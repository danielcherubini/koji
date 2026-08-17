//! Shared Postgres test harness for the tama workspace.
//!
//! All test binaries on the machine share ONE `postgres:16` instance:
//! - `TAMA_TEST_PG_DSN` (e.g. `postgres://tama:tama@127.0.0.1:5433/tama`)
//!   connects directly to an externally-managed Postgres — no container.
//! - Otherwise, connect to the shared container at 127.0.0.1:`TAMA_TEST_PG_PORT`
//!   (default 5433); if nothing is there, start a container named/labelled
//!   `tama-test-pg` on that fixed port. The container is intentionally NOT
//!   removed when the binary exits (other binaries share it) — clean up with
//!   `make docker-clean`.
//!
//! Per-test isolation is via private schemas ([`SchemaGuard`]): each test
//! gets a fresh schema with the caller's migrations applied to it. The
//! schema-name prefix and the [`sqlx::migrate::Migrator`] are parameterized
//! per crate via [`Harness`] so in-src and integration tests keep distinct
//! schema namespaces.
//!
//! This crate is a **dev-dependency only** — it must never be a regular
//! dependency of a crate that is compiled for WASM.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool, Row};
use testcontainers::runners::SyncRunner;
use testcontainers::{Container, GenericImage, ImageExt};

/// Pinned container image tag.
const POSTGRES_TAG: &str = "16";
/// Credentials of the shared test server (container or external).
const PG_USER: &str = "tama";
const PG_PASSWORD: &str = "tama";
const PG_DB: &str = "tama";
/// Default host port of the shared container (override: `TAMA_TEST_PG_PORT`).
const DEFAULT_SHARED_PORT: u16 = 5433;
/// Name/label of the shared container (used by `make docker-clean`).
const SHARED_CONTAINER_NAME: &str = "tama-test-pg";
/// Literal SQL for the startup sweep of leaked `tama_%` schemas
/// (a compile-time string, satisfying sqlx's `SqlSafeStr` bound).
const SWEEP_LIST_SQL: &str = "SELECT nspname FROM pg_namespace WHERE nspname LIKE 'tama_%'";

/// Monotonic counter for unique schema names.
static SCHEMA_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Background runtime used for best-effort schema cleanup on drop.
///
/// Built on a plain thread because constructing a multi-threaded runtime
/// from within a running runtime panics.
fn background_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("failed to build background cleanup runtime");
            tx.send(rt).expect("init channel closed");
        });
        rx.recv().expect("init thread died")
    })
}

/// Where to get Postgres: a direct DSN or the shared container port.
#[derive(Debug)]
enum PgTarget {
    /// Connect directly to this DSN (from `TAMA_TEST_PG_DSN`).
    Direct(String),
    /// Use the shared container on 127.0.0.1 at this port.
    Shared { port: u16 },
}

/// Resolve the Postgres target from env vars (pure; unit-tested).
fn resolve_target(dsn_env: Option<&str>, port_env: Option<&str>) -> PgTarget {
    match dsn_env {
        Some(dsn) if !dsn.trim().is_empty() => PgTarget::Direct(dsn.trim().to_string()),
        _ => PgTarget::Shared {
            port: port_env
                .and_then(|p| p.trim().parse().ok())
                .unwrap_or(DEFAULT_SHARED_PORT),
        },
    }
}

/// DSN of the shared container (user/password/db are all `tama`).
fn shared_dsn(port: u16) -> String {
    format!("postgresql://{PG_USER}:{PG_PASSWORD}@127.0.0.1:{port}/{PG_DB}?sslmode=disable")
}

/// Short-timeout connection probe (returns false instead of erroring).
fn probe(rt: &tokio::runtime::Runtime, url: &str) -> bool {
    rt.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::PgConnection::connect(url),
        )
        .await
        .is_ok_and(|r| r.is_ok())
    })
}

/// Parse the `HostPort` field of a `docker inspect` output (pure; unit-tested).
fn parse_host_port(raw: &str) -> Option<u16> {
    raw.trim().parse().ok()
}

/// Classification of a `docker inspect` probe of the shared container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerProbe {
    /// No container with that name (or docker unavailable) — start fresh.
    NotFound,
    /// Exists, running, and mapped to the expected port — reuse it.
    Reuse,
    /// Exists but stopped, or mapped to a stale port — safe to remove.
    Remove,
}

/// Parse the combined inspect output (`{{.State.Running}}|<HostPort>`),
/// where the port half may be `<no value>` or empty when the container has
/// no port mapping (pure; unit-tested).
fn parse_inspect_line(raw: &str) -> Option<(bool, Option<u16>)> {
    let (running, port) = raw.trim().split_once('|')?;
    let running = running.trim().parse::<bool>().ok()?;
    Some((running, parse_host_port(port)))
}

/// Classify a probe result against the expected shared port (pure;
/// unit-tested). A STOPPED container's `docker inspect` still reports its
/// mapped port, so the running flag decides: only a *running* container on
/// the expected port is reusable; anything else is safe to remove.
fn classify_probe(running: bool, mapped_port: Option<u16>, expected_port: u16) -> ContainerProbe {
    if running && mapped_port == Some(expected_port) {
        ContainerProbe::Reuse
    } else {
        ContainerProbe::Remove
    }
}

/// Probe the named container: `NotFound` when it is absent or docker is
/// unavailable, otherwise `Reuse`/`Remove` for the given expected port.
fn probe_container(name: &str, expected_port: u16) -> ContainerProbe {
    let out = std::process::Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.State.Running}}|{{(index .NetworkSettings.Ports \"5432/tcp\").[0].HostPort}}",
            name,
        ])
        .output()
        .ok();
    let Some(out) = out else {
        return ContainerProbe::NotFound;
    };
    if !out.status.success() {
        return ContainerProbe::NotFound;
    }
    let Ok(stdout) = String::from_utf8(out.stdout) else {
        return ContainerProbe::Remove;
    };
    match parse_inspect_line(&stdout) {
        Some((running, port)) => classify_probe(running, port, expected_port),
        // Inspect succeeded but the output is unparseable: the container
        // exists — remove it and start fresh (pre-fix recovery behavior).
        None => ContainerProbe::Remove,
    }
}

/// Start the shared container on a fixed host port.
///
/// The returned handle is kept in the process-global static and never
/// dropped, so the container survives this binary's exit (other test
/// binaries share it). `make docker-clean` removes it.
///
/// Race-safe cleanup: a container that is running and already mapped to our
/// port is left alone (it belongs to a sibling binary, possibly still
/// initializing) and an error is returned so the caller waits for it to
/// become reachable. Only a container that is absent or mapped to a
/// different (stale) port is removed first.
fn start_shared_container(port: u16) -> Result<Container<GenericImage>, String> {
    match probe_container(SHARED_CONTAINER_NAME, port) {
        ContainerProbe::Reuse => {
            return Err(format!(
                "container {SHARED_CONTAINER_NAME} is already running on port {port}"
            ))
        }
        _ => {
            // Absent, stopped, or mapped to a different port — safe to remove.
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", SHARED_CONTAINER_NAME])
                .status();
        }
    }

    GenericImage::new("postgres", POSTGRES_TAG)
        .with_container_name(SHARED_CONTAINER_NAME)
        .with_label("tama-test", "true")
        .with_mapped_port(port, 5432.into())
        .with_env_var("POSTGRES_USER", PG_USER)
        .with_env_var("POSTGRES_PASSWORD", PG_PASSWORD)
        .with_env_var("POSTGRES_DB", PG_DB)
        // Keep the shared container's memory small (32GB host).
        .with_cmd([
            "postgres",
            "-c",
            "shared_buffers=64MB",
            "-c",
            "work_mem=4MB",
            "-c",
            "maintenance_work_mem=16MB",
        ])
        .start()
        .map_err(|e| e.to_string())
}

/// Drop leaked `tama_%` test schemas (best-effort, never fails the run).
///
/// Called only on the first connection to a container that THIS process
/// just created, so no other test binary can be mid-test inside it.
async fn sweep_leaked_schemas(conn: &mut PgConnection) {
    let rows = match sqlx::query(SWEEP_LIST_SQL).fetch_all(&mut *conn).await {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("schema sweep: failed to list schemas: {e}");
            return;
        }
    };
    for row in rows {
        let name: String = match row.try_get("nspname") {
            Ok(name) => name,
            Err(e) => {
                eprintln!("schema sweep: failed to read schema name: {e}");
                continue;
            }
        };
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {name} CASCADE")))
            .execute(&mut *conn)
            .await;
    }
}

/// Shared state: optional container (only if this binary started it),
/// pool, base URL, and the runtime that owns the pool (sqlx 0.9 pools are
/// bound to the runtime that created them, so it must be kept alive for the
/// process).
type SharedState = (
    Option<Container<GenericImage>>,
    PgPool,
    String,
    tokio::runtime::Runtime,
);

/// Resolve the shared Postgres and return its state (blocking; runs once).
fn init_shared() -> SharedState {
    // testcontainers' sync runner drives its async client with `block_on`,
    // so the container must be started on a plain thread, never from within
    // the tests' async runtime.
    let (tx, rx) = std::sync::mpsc::channel::<Result<SharedState, String>>();
    std::thread::spawn(move || {
        let result = (|| -> Result<SharedState, String> {
            // Long-lived runtime: owns the shared pool's background tasks
            // for the whole process.
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;

            let (url, container) = match resolve_target(
                std::env::var("TAMA_TEST_PG_DSN").ok().as_deref(),
                std::env::var("TAMA_TEST_PG_PORT").ok().as_deref(),
            ) {
                PgTarget::Direct(dsn) => (dsn, None),
                PgTarget::Shared { port } => {
                    let dsn = shared_dsn(port);
                    // Another binary may have already started the shared
                    // container on this port — reuse it if reachable.
                    if probe(&rt, &dsn) {
                        (dsn, None)
                    } else {
                        match start_shared_container(port) {
                            Ok(c) => (dsn, Some(c)),
                            // Start failed (another binary is starting or
                            // already started the container): wait for the
                            // other binary's server to become reachable.
                            Err(e) => {
                                let deadline =
                                    std::time::Instant::now() + std::time::Duration::from_secs(60);
                                loop {
                                    if probe(&rt, &dsn) {
                                        break (dsn, None);
                                    }
                                    if std::time::Instant::now() >= deadline {
                                        return Err(format!(
                                            "failed to start shared postgres container on port {port} and no server became reachable: {e}"
                                        ));
                                    }
                                    std::thread::sleep(std::time::Duration::from_millis(500));
                                }
                            }
                        }
                    }
                }
            };

            // The entrypoint restarts Postgres while initializing the DB, so
            // poll until a real connection succeeds.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            loop {
                match rt.block_on(sqlx::PgConnection::connect(&url)) {
                    Ok(mut conn) => {
                        // Fresh container: sweep leaked schemas from
                        // previously killed test runs (safe only because
                        // we just created it).
                        if container.is_some() {
                            rt.block_on(async {
                                sweep_leaked_schemas(&mut conn).await;
                            });
                        }
                        drop(conn);
                        break;
                    }
                    Err(e) if std::time::Instant::now() < deadline => {
                        eprintln!("waiting for postgres to be ready: {e}");
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                    Err(e) => return Err(format!("postgres never became ready: {e}")),
                }
            }

            let pool = rt.block_on(async {
                PgPoolOptions::new()
                    .max_connections(4)
                    .connect_lazy(&url)
                    .expect("valid pool config")
            });

            Ok((container, pool, url, rt))
        })();
        tx.send(result).expect("init channel closed");
    });
    rx.recv()
        .expect("init thread died")
        .expect("failed to reach shared postgres:16 test server")
}

/// The shared pool + URL + owning runtime (started once).
fn shared() -> &'static SharedState {
    static INIT: OnceLock<SharedState> = OnceLock::new();
    INIT.get_or_init(init_shared)
}

/// Returns a pool connected to the shared `postgres:16` test container.
///
/// The container (and its pool) are started once per process and reused by
/// all callers.
pub async fn test_pool() -> PgPool {
    shared().1.clone()
}

/// Host and port of the container's Postgres service.
///
/// For building a `DatabaseConfig` that points at the shared container
/// (plan-190 pool startup tests).
pub fn container_host_port() -> (String, u16) {
    let url = url::Url::parse(&shared().2).expect("valid test container URL");
    (
        url.host_str().unwrap_or("localhost").to_string(),
        url.port().unwrap_or(5432),
    )
}

/// A lazily-created pool for tests that must hold a pool but never touch
/// the database. `connect_lazy` does not dial, so construction is safe
/// without a running server (port 1 is virtually guaranteed closed).
pub fn test_dummy_pool() -> std::sync::Arc<PgPool> {
    std::sync::Arc::new(
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect_lazy("postgres://tama:tama@127.0.0.1:1/tama")
            .expect("valid dummy pool config"),
    )
}

/// Per-crate harness configuration: the schema-name prefix and the
/// migrations to apply into each fresh test schema.
///
/// Cheap (two references) — construct it as a `static` in each crate.
pub struct Harness {
    schema_prefix: &'static str,
    migrator: &'static sqlx::migrate::Migrator,
}

impl Harness {
    /// Create a harness with the given schema prefix and migrator.
    pub const fn new(
        schema_prefix: &'static str,
        migrator: &'static sqlx::migrate::Migrator,
    ) -> Self {
        Self {
            schema_prefix,
            migrator,
        }
    }

    /// Create an isolated test schema, scope a pool to it via
    /// `search_path`, and run the harness's migrations against it.
    pub async fn with_schema(&self) -> SchemaGuard {
        let (base, url) = (&shared().1, &shared().2);
        let pid = std::process::id();
        let n = SCHEMA_COUNTER.fetch_add(1, Ordering::SeqCst);
        let schema = format!("{}{pid}_{n:04}", self.schema_prefix);

        {
            let mut conn = base
                .acquire()
                .await
                .expect("acquire shared pool connection");
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
                .execute(&mut *conn)
                .await
                .expect("failed to create test schema");
        }

        let scoped_url = format!("{url}&options=-c%20search_path={schema}");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_lazy(&scoped_url)
            .expect("valid pool config");

        // Apply the migrations into this schema.
        self.migrator
            .run(&pool)
            .await
            .expect("failed to run Postgres migrations in test schema");

        SchemaGuard {
            schema,
            pool,
            finished: false,
        }
    }
}

/// A Postgres pool scoped to a private test schema with migrations applied.
///
/// Running `finish()` (or dropping the guard) drops the schema CASCADE.
pub struct SchemaGuard {
    pub schema: String,
    pub pool: PgPool,
    finished: bool,
}

impl SchemaGuard {
    /// Drop the test schema (CASCADE) and close the schema-scoped pool.
    pub async fn finish(mut self) {
        self.finished = true;
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.pool)
        .await;
        self.pool.close().await;
    }
}

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // Best-effort cleanup on the background runtime (schema names are
        // unique per test, so async cleanup cannot collide with new tests).
        let schema = std::mem::take(&mut self.schema);
        let pool = self.pool.clone();
        background_runtime().spawn(async move {
            let _ = sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
                .execute(&pool)
                .await;
            pool.close().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_target_prefers_dsn_env() {
        let target = resolve_target(Some("postgres://u:p@h:1/d"), Some("5434"));
        assert!(
            matches!(&target, PgTarget::Direct(d) if d == "postgres://u:p@h:1/d"),
            "DSN env must win over port env, got {target:?}"
        );
    }

    #[test]
    fn test_resolve_target_ignores_blank_dsn() {
        assert!(matches!(
            resolve_target(Some("  "), None),
            PgTarget::Shared { port: 5433 }
        ));
    }

    #[test]
    fn test_resolve_target_port_env() {
        assert!(matches!(
            resolve_target(None, Some("5434")),
            PgTarget::Shared { port: 5434 }
        ));
    }

    #[test]
    fn test_resolve_target_default_port() {
        assert!(matches!(
            resolve_target(None, None),
            PgTarget::Shared { port: 5433 }
        ));
        assert!(matches!(
            resolve_target(None, Some("not-a-port")),
            PgTarget::Shared { port: 5433 }
        ));
    }

    #[test]
    fn test_shared_dsn_format() {
        assert_eq!(
            shared_dsn(5433),
            "postgresql://tama:tama@127.0.0.1:5433/tama?sslmode=disable"
        );
    }

    #[test]
    fn test_parse_host_port() {
        assert_eq!(parse_host_port("5433\n"), Some(5433));
        assert_eq!(parse_host_port(" 5434 "), Some(5434));
        assert_eq!(parse_host_port("0"), Some(0));
        assert_eq!(parse_host_port(""), None);
        assert_eq!(parse_host_port("<nil>"), None);
    }

    #[test]
    fn test_parse_inspect_line_running_with_port() {
        assert_eq!(parse_inspect_line("true|5433\n"), Some((true, Some(5433))));
    }

    #[test]
    fn test_parse_inspect_line_stopped_still_reports_port() {
        // A stopped container's inspect output still includes the mapped
        // port — this is exactly the regression case (F2).
        assert_eq!(
            parse_inspect_line("false|5433\n"),
            Some((false, Some(5433)))
        );
    }

    #[test]
    fn test_parse_inspect_line_no_value_port() {
        assert_eq!(parse_inspect_line("true|<no value>\n"), Some((true, None)));
    }

    #[test]
    fn test_parse_inspect_line_garbage() {
        assert_eq!(parse_inspect_line(""), None);
        assert_eq!(parse_inspect_line("true\n"), None);
        assert_eq!(parse_inspect_line("notabool|5433\n"), None);
    }

    #[test]
    fn test_classify_probe_running_expected_port_reuse() {
        assert_eq!(
            classify_probe(true, Some(5433), 5433),
            ContainerProbe::Reuse
        );
    }

    #[test]
    fn test_classify_probe_stopped_removable() {
        // Stopped but on the right port: must be treated as removable, not
        // "already running" (F2 regression).
        assert_eq!(
            classify_probe(false, Some(5433), 5433),
            ContainerProbe::Remove
        );
        assert_eq!(classify_probe(false, None, 5433), ContainerProbe::Remove);
    }

    #[test]
    fn test_classify_probe_stale_port_removable() {
        assert_eq!(
            classify_probe(true, Some(5434), 5433),
            ContainerProbe::Remove
        );
        assert_eq!(classify_probe(true, None, 5433), ContainerProbe::Remove);
    }
}
