//! Structured log store (SQLite).
//!
//! Persistence layer for Tama's structured logging feature.
//!
//! An embedded SQLite database that stores log entries as "one JSON document
//! per row + indexed label columns" (the Loki model: labels indexed, payload
//! not indexed). See ADR-0013 (`docs/adr/0013-log-store-sqlite.md`).
//!
//! ## Modules
//!
//! - [`db`] — the SQLite store (schema, FTS5, batch insert, query,
//!   prune). Pure persistence; knows nothing about tracing/axum/tamad.
//! - [`types`] — the log level domain, `source` labels, query shape,
//!   result rows.
//! - [`layer`] — the `tracing` layer: one `try_send` per event into a
//!   bounded channel (drop-newest, drop counter shared with the writer).
//! - [`writer`] — the single writer task: drains the channel into the
//!   store in batches, journaled-style degradation + ring, dropped-event
//!   markers, `watch` status broadcast. THE writer owns the store's sole
//!   connection (worker-guard rule — see `writer::spawn_log_writer`).
//! - [`filter`] — shared `EnvFilter` builder (config `log_level` floor +
//!   `log_directives` + `RUST_LOG` merged in exactly one place; shared
//!   by proxy, `tama admin`, and — task 6 — tamad startup).
//!
//! ## Concurrency model
//!
//! [`db::LogStore`] is `Send` (it holds one `rusqlite::Connection`).
//! The tracing writer is the sole writer; a read endpoint opens its own
//! separate connection (task 4). WAL journal mode keeps readers usable
//! during write bursts.

pub mod db;
pub mod dedupe;
pub mod filter;
pub mod layer;
pub mod types;
pub mod writer;

pub use db::LogStore;
pub use dedupe::{Decision, DedupState};
pub use filter::{apply_reload, build_log_filter, LogFilterError};
pub use layer::{build_layer, FieldValueVisitor, LogStoreLayer};
pub use types::{
    LevelCount, LogEntry, LogQuery, LogRecord, LogstoreLevel, PruneBounds, QueryOrder, Source,
    SourceInfo,
};
pub use writer::{spawn_log_writer, spawn_log_writer_with_config, LogStoreStatus, WriterConfig};

#[cfg(test)]
mod e2e_tests {
    //! Cross-file end-to-end: layer → channel → writer → on-disk store
    //! (the full plan-195 task 2 pipeline, multiplexed in memory).

    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;
    use std::time::Duration;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    /// A `tracing::info!` through a real layer+registry is captured,
    /// batched, and stored as a queryable row with the documented msg
    /// shape — and cancel returns the writer on a healthy status.
    #[tokio::test]
    async fn test_layer_channel_writer_store_end_to_end() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("e2e.db");
        let store = LogStore::open(&path).expect("open store");

        let (tx, rx) = tokio::sync::mpsc::channel::<LogRecord>(64);
        let dropped = Arc::new(AtomicU64::new(0));
        let (status_tx, _status_rx) = tokio::sync::watch::channel(LogStoreStatus::ok());
        let token = tokio_util::sync::CancellationToken::new();
        let handle = spawn_log_writer(store, rx, dropped.clone(), status_tx, token.clone());

        let layer = build_layer(tx, Source::proxy(), dropped);
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(run_id = 7, "e2e hello");
        });

        // One collect tick + margin (real time — this is the only real-
        // time sleep in the suite; e2e keeps the production cadence).
        tokio::time::sleep(Duration::from_millis(400)).await;

        let path = path.clone();
        let check = LogStore::open(&path).expect("check connection");
        let (entries, next) = check.query(&LogQuery::default()).expect("query");
        assert!(
            next.is_none(),
            "small end-to-end result: window end reached"
        );
        assert_eq!(entries.len(), 1, "exactly the emit event is stored");
        let entry = &entries[0];
        assert_eq!(entry.source.as_str(), "proxy", "layer source");
        assert_eq!(entry.level, LogstoreLevel::INFO);
        assert!(entry.ts > 0, "capture-time ts");
        assert_eq!(entry.msg.get("message"), Some(&json!("e2e hello")));
        assert_eq!(entry.msg.get("target"), Some(&json!(module_path!())));
        assert_eq!(entry.msg.get("run_id"), Some(&json!(7)));

        // Cancel: the writer drains (nothing left) and joins on a
        // healthy final status; the WorkerGuard rule is the caller's
        // job — this test holds the handle to the end.
        token.cancel();
        let final_status = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("writer joins after cancel")
            .expect("writer completes cleanly");
        assert!(!final_status.degraded);
        assert_eq!(final_status.channel_len, 0);
        assert_eq!(final_status.ring_len, 0);
    }
}
