//! In-crate (in-file `#[cfg(test)]`) Postgres test harness for the `tama` crate.
//!
//! Mirrors `tama-core::testing::postgres` for in-src unit tests that need the
//! model domain in Postgres (plan-190 Task 5). Integration tests under
//! `tests/` should keep using `tests/common/mod.rs`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgPool};
use testcontainers::runners::SyncRunner;
use testcontainers::{Container, GenericImage, ImageExt};

/// Pinned container image tag (kept in sync with tests/common/mod.rs).
const POSTGRES_TAG: &str = "16";
const POSTGRES_USER: &str = "postgres";
const POSTGRES_PASSWORD: &str = "postgres";
const POSTGRES_DB: &str = "postgres";

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

type SharedState = (
    Container<GenericImage>,
    PgPool,
    String,
    tokio::runtime::Runtime,
);

/// Start the shared container and return its state (blocking; runs once).
fn init_shared() -> SharedState {
    let (tx, rx) = std::sync::mpsc::channel::<Result<SharedState, String>>();
    std::thread::spawn(move || {
        let result = (|| -> Result<SharedState, String> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;

            let container = GenericImage::new("postgres", POSTGRES_TAG)
                .with_exposed_port(5432.into())
                .with_env_var("POSTGRES_USER", POSTGRES_USER)
                .with_env_var("POSTGRES_PASSWORD", POSTGRES_PASSWORD)
                .with_env_var("POSTGRES_DB", POSTGRES_DB)
                .start()
                .map_err(|e| e.to_string())?;
            let port = container
                .get_host_port_ipv4(5432)
                .map_err(|e| e.to_string())?;
            let url = format!(
                "postgresql://{}:{}@localhost:{}/{}?sslmode=disable",
                POSTGRES_USER, POSTGRES_PASSWORD, port, POSTGRES_DB
            );

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
        .expect("failed to start postgres:16 test container")
}

/// The shared container + pool + URL + owning runtime (started once).
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
    let n = SCHEMA_COUNTER.fetch_add(1, Ordering::SeqCst);
    let schema = format!("tama_web_{n:04}");

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
