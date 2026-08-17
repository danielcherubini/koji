//! In-crate (in-file `#[cfg(test)]`) Postgres test harness for the `tama` crate.
//!
//! Mirrors `tama-core::testing::postgres` for in-src unit tests that need the
//! model domain in Postgres (plan-190 Task 5). Integration tests under
//! `tests/` should keep using `tests/common/mod.rs`.
//!
//! All test binaries on the machine share ONE `postgres:16` instance:
//! `TAMA_TEST_PG_DSN` connects to an externally-managed Postgres directly;
//! otherwise the shared `tama-test-pg` container on 127.0.0.1:`TAMA_TEST_PG_PORT`
//! (default 5433) is reused or started. The container is intentionally NOT
//! removed when a binary exits (other binaries share it) — clean up with
//! `make docker-clean`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgPool};
use testcontainers::runners::SyncRunner;
use testcontainers::{Container, GenericImage, ImageExt};

/// Pinned container image tag (kept in sync with tests/common/mod.rs).
const POSTGRES_TAG: &str = "16";
/// Credentials of the shared test server (container or external).
const PG_USER: &str = "tama";
const PG_PASSWORD: &str = "tama";
const PG_DB: &str = "tama";
/// Default host port of the shared container (override: `TAMA_TEST_PG_PORT`).
const DEFAULT_SHARED_PORT: u16 = 5433;
/// Name/label of the shared container (used by `make docker-clean`).
const SHARED_CONTAINER_NAME: &str = "tama-test-pg";

/// Monotonic counter for unique schema names.
static SCHEMA_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Background runtime used for best-effort schema cleanup on drop.
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

/// Start the shared container on a fixed host port.
///
/// The returned handle is kept in the process-global static and never
/// dropped, so the container survives this binary's exit (other test
/// binaries share it). `make docker-clean` removes it.
fn start_shared_container(port: u16) -> Result<Container<GenericImage>, String> {
    // Best-effort: remove a stale (stopped) container holding the name.
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", SHARED_CONTAINER_NAME])
        .status();

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
    let (tx, rx) = std::sync::mpsc::channel::<Result<SharedState, String>>();
    std::thread::spawn(move || {
        let result = (|| -> Result<SharedState, String> {
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
                            // Start failed (raced another binary for the port
                            // or the name): wait for the other binary's
                            // server to become reachable.
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
                    Ok(conn) => {
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

/// A Postgres pool scoped to a private test schema with migrations applied.
pub struct SchemaGuard {
    pub schema: String,
    pub pool: PgPool,
    finished: bool,
}

/// Create an isolated test schema, scope a pool to it via `search_path`,
/// and run the embedded Postgres migrations against it.
pub async fn with_schema() -> SchemaGuard {
    let (base, url) = (&shared().1, &shared().2);
    let pid = std::process::id();
    let n = SCHEMA_COUNTER.fetch_add(1, Ordering::SeqCst);
    let schema = format!("tama_web_{pid}_{n:04}");

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

    tama_core::db::postgres::run_migrations(&pool)
        .await
        .expect("failed to run Postgres migrations in test schema");

    SchemaGuard {
        schema,
        pool,
        finished: false,
    }
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
}
