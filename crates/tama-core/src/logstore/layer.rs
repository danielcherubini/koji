//! The tracing layer side of structured logging (plan-195 task 2).
//!
//! [`LogStoreLayer`] encodes each event into a [`LogRecord`] and sends it
//! over the bounded channel to the writer task
//! ([`super::writer`]). The layer IS the hot path: exactly ONE
//! `try_send` per event. Channel full → the policy is DROP NEWEST (this,
//! in-flight, event) and bump the shared drop counter. `on_event` never
//! blocks — a slow disk must not stall request-time code.
//!
//! The drop counter is shared with the writer (both take it as a
//! parameter — the writer needs it to know how many events were dropped
//! and to enqueue the synthetic "dropped N events" marker after its 5 s
//! window; see [`super::writer`]).
//!
//! ## `on_event` encoding
//!
//! - `ts` — capture time, in unix milliseconds
//!   (`SystemTime::now()`), the source of truth built for display.
//! - `level` — the mapping from the event level to the
//!   [`LogstoreLevel`] domain.
//! - `source` — the layer's configured source (default
//!   `Source::proxy()`; each binary sets it at construction; task 6 has
//!   tamad setting its own).
//! - `msg` — a single JSON object: `{"message": <event's message text>,
//!   "target": <metadata target>}`, plus all named fields the event
//!   carried as first-class keys. Named fields only — in v1, NO span
//!   fields (span policy deferred).
//!
//! Field encoding is the job of [`FieldValueVisitor`] (one visitor; the
//! event is recorded into it via `event.record(&mut visitor)`):
//! primitives (`i64`/`u64`/`f64`/`bool`), strings and bytes become JSON
//! values; an error value becomes its Display text; anything
//! unrepresentable (a Debug-wrapped value, etc.) becomes JSON of its
//! Debug-formatted string. A field recorded as a value whose Debug text
//! is plain JSON (a `?value` capture of a `serde_json::Value` — this
//! tracing version has no by-ref value downcast API, so "as-is" is
//! rebuilt from the Debug text at that boundary) is kept as a JSON
//! value, not a pre-serialized JSON string of a struct: the best-practice
//! rule (ADR-0013 / Q2) is that structs never flow through as JSON blob
//! strings — only scalar Debug text or a restored JSON value is
//! emitted.
//!
//! Conversion failures can never be panics on the hot path: no
//! `unwrap`/`expect` anywhere in the layer — a field that fails to
//! convert is SKIPPED (the rest of the record is kept). The only
//! special key: fields named `message` are merged into the event's
//! message text (tracing's own message field is `"message"`).
//!
//! The synthetic drop markers the writer enqueues over the same channel
//! are indistinguishable rows downstream.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::logstore::types::{LogRecord, LogstoreLevel, Source};
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::Level as TracingLevel;
use tracing::Subscriber;
use tracing_subscriber::Layer;

/// Encodes the named fields carried by an event into a flat JSON map
/// (plus the message text), as documented on the module.
///
/// Used directly in unit tests and by the layer's `on_event`.
#[derive(Default)]
pub struct FieldValueVisitor {
    /// Fields other than the message, keyed by field name.
    inner: Map<String, Value>,
    /// Accumulated text of the message field.
    message: String,
}

impl FieldValueVisitor {
    /// New, empty visitor.
    pub fn new() -> Self {
        Self {
            inner: Map::new(),
            message: String::new(),
        }
    }

    /// Take out the encoded parts: `(message text, fields)`.
    pub fn into_record_parts(self) -> (String, Map<String, Value>) {
        (self.message, self.inner)
    }
}

impl Visit for FieldValueVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // tracing's own message field is named "message".
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
            return;
        }
        // This tracing-core version has no by-ref value downcast API, so
        // "as is" is reconstructed from the formatted text: if the text
        // is plain JSON (a `%value` capture of `serde_json::Value` — the
        // protocol's intended path for pre-encoded value payloads), it
        // goes through as a JSON value; otherwise the Debug form survives
        // as a string. Structs never flow as pre-serialized JSON blobs
        // (ADR-0013 / Q2 best practice).
        let text = format!("{value:?}");
        let value = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
        self.inner.insert(field.name().to_owned(), value);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
            return;
        }
        self.inner
            .insert(field.name().to_owned(), Value::String(value.to_owned()));
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        if field.name() == "message" {
            self.message = String::from_utf8_lossy(value).into_owned();
            return;
        }
        self.inner.insert(
            field.name().to_owned(),
            Value::String(String::from_utf8_lossy(value).into_owned()),
        );
    }

    fn record_error(&mut self, field: &Field, value: &dyn std::error::Error) {
        if field.name() == "message" {
            self.message = format!("{value}");
            return;
        }
        self.inner
            .insert(field.name().to_owned(), Value::String(format!("{value}")));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.inner
            .insert(field.name().to_owned(), Value::from(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.inner
            .insert(field.name().to_owned(), Value::from(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        // JSON has no non-finite numbers; `Value::from(f64)` would map
        // NaN/±inf to null — keep those readable as Debug strings.
        let value: Value = if value.is_finite() {
            Value::from(value)
        } else {
            Value::String(format!("{value:?}"))
        };
        self.inner.insert(field.name().to_owned(), value);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.inner
            .insert(field.name().to_owned(), Value::from(value));
    }
}

/// Appends each event as a [`LogRecord`] over the bounded channel to the
/// writer task (see the module docs for the encoding and the
/// drop-newest channel policy). `Clone` is cheap — every clone shares
/// the channel and the drop counter.
#[derive(Clone)]
pub struct LogStoreLayer {
    tx: mpsc::Sender<LogRecord>,
    source: Source,
    /// Shared with the writer; bumped on channel-full drop.
    dropped: Arc<AtomicU64>,
}

/// Builds a layer writing to the channel endpoint `tx`, stamped with
/// `source`. `dropped` is shared with the writer that drains the other
/// end of the channel (see [`super::writer`]).
pub fn build_layer(
    tx: mpsc::Sender<LogRecord>,
    source: Source,
    dropped: Arc<AtomicU64>,
) -> LogStoreLayer {
    LogStoreLayer {
        tx,
        source,
        dropped,
    }
}

impl<S> Layer<S> for LogStoreLayer
where
    S: Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = FieldValueVisitor::new();
        event.record(&mut visitor);

        let (message, mut fields) = visitor.into_record_parts();
        fields.insert("message".to_owned(), Value::String(message));
        fields.insert(
            "target".to_owned(),
            Value::String(event.metadata().target().to_owned()),
        );

        let metadata = event.metadata();
        let level = match *metadata.level() {
            TracingLevel::TRACE => LogstoreLevel::TRACE,
            TracingLevel::DEBUG => LogstoreLevel::DEBUG,
            TracingLevel::INFO => LogstoreLevel::INFO,
            TracingLevel::WARN => LogstoreLevel::WARN,
            TracingLevel::ERROR => LogstoreLevel::ERROR,
        };
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let record = LogRecord {
            ts,
            level,
            source: self.source.clone(),
            msg: Value::Object(fields),
        };

        // Never block on the hot path: channel full ⇒ drop this event
        // (drop-newest policy) and tell the writer task about it.
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(record) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
        // A `Closed` error means the writer task is already gone (store
        // shutting down) — nothing to do.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logstore::types::LogstoreLevel;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    struct EmptyError;

    impl std::fmt::Display for EmptyError {
        fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // Display comes out empty — to_string() == "".
            Ok(())
        }
    }

    impl std::fmt::Debug for EmptyError {
        fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            Ok(())
        }
    }

    impl std::error::Error for EmptyError {}

    fn visitor() -> (
        mpsc::Sender<LogRecord>,
        tokio::sync::mpsc::Receiver<LogRecord>,
        Arc<AtomicU64>,
    ) {
        let (tx, rx) = mpsc::channel(16);
        (tx, rx, Arc::new(AtomicU64::new(0)))
    }

    /// `info!(gpu = "a", n = 1, "m")` flattens into
    /// `{"message":"m","target":<module>,"gpu":"a","n":1}` — primitives
    /// as JSON values, message and target first-class keys.
    #[test]
    fn test_flattens_named_fields_as_first_class_keys() {
        let (tx, mut rx, dropped) = visitor();
        let layer = build_layer(tx, Source::proxy(), dropped);
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(gpu = "a", n = 1, "m");
        });

        let record = rx.try_recv().expect("record enqueued");
        assert!(record.ts > 0, "ts is a capture-time unix ms");
        assert_eq!(record.level, LogstoreLevel::INFO);
        assert_eq!(
            record.source.as_str(),
            "proxy",
            "the configured source is stamped"
        );
        assert_eq!(record.msg.get("message"), Some(&json!("m")));
        assert_eq!(record.msg.get("target"), Some(&json!(module_path!())));
        assert_eq!(record.msg.get("gpu"), Some(&json!("a")));
        assert_eq!(record.msg.get("n"), Some(&json!(1)));
        assert_eq!(
            record.msg.as_object().expect("msg is a JSON object").len(),
            4,
            "message, target, gpu, n — nothing else"
        );
    }

    /// Level mapping and per-binary source.
    #[test]
    fn test_level_and_source_mapping() {
        let (tx, mut rx, dropped) = visitor();
        let layer = build_layer(tx, Source::backend("llama-cpp"), dropped);
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("oom watch");
            tracing::error!("hard fail");
        });

        let warn = rx.try_recv().expect("warn enqueued");
        let error = rx.try_recv().expect("error enqueued");
        assert_eq!(warn.level, LogstoreLevel::WARN);
        assert_eq!(error.level, LogstoreLevel::ERROR);
        assert_eq!(warn.source.as_str(), "backend:llama-cpp");
        assert_eq!(error.source.as_str(), "backend:llama-cpp");
        assert_eq!(warn.msg.get("message"), Some(&json!("oom watch")));
    }

    /// Anything unrepresentable becomes its Debug-formatted JSON string
    /// (`?value`); a non-finite f64 stays readable as a string.
    #[test]
    fn test_unrepresentable_field_becomes_debug_string() {
        #[derive(Debug)]
        struct Deep {
            x: u8,
        }

        let (tx, mut rx, dropped) = visitor();
        let layer = build_layer(tx, Source::proxy(), dropped);
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let d = Deep { x: 7 };
            let _ = d.x; // only the Debug text matters here
            tracing::debug!(?d, "watch bytes");
            tracing::debug!(f = f64::NAN, "nan not json");
        });

        let deep = rx.try_recv().expect("records with ?-value");
        assert_eq!(
            deep.msg.get("d"),
            Some(&json!("Deep { x: 7 }")),
            "struct appears as a Debug string, never as a JSON blob"
        );
        let nan = rx.try_recv().expect("nan record");
        assert_eq!(nan.msg.get("f"), Some(&json!("NaN")));
    }

    /// A field that formats to an empty error: the record survives, and
    /// the field is present as the empty string (no unwrap/expect on
    /// the hot path).
    #[test]
    fn test_error_field_formatting_to_empty_still_records() {
        let (tx, mut rx, dropped) = visitor();
        let layer = build_layer(tx, Source::proxy(), dropped);
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let empty: &dyn std::error::Error = &EmptyError;
            tracing::info!(empty, "boom");
        });

        let record = rx.try_recv().expect("record despite empty-error field");
        assert_eq!(
            record.msg.get("empty"),
            Some(&json!("")),
            "empty Display text kept"
        );
        assert_eq!(record.msg.get("message"), Some(&json!("boom")));
    }

    /// A `%value` capture of a `serde_json::Value` stays a JSON value
    /// (its Display text IS compact JSON — the visitor rebuilds it),
    /// never a pre-serialized JSON string of a struct.
    #[test]
    fn test_value_field_stays_json() {
        let (tx, mut rx, dropped) = visitor();
        let layer = build_layer(tx, Source::proxy(), dropped);
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let payload = json!({"a": 1, "b": [2, 3]});
            tracing::info!(%payload, "restored");
        });

        let record = rx.try_recv().expect("record");
        let value = record.msg.get("payload").expect("payload field");
        assert_eq!(
            value,
            &json!({ "a": 1, "b": [2, 3] }),
            "value field preserved as a JSON value"
        );
        assert!(
            value.is_object(),
            "not a string — reconstruct the JSON value, do not pre-serialize"
        );
    }

    /// Drop-newest semantics: with a full channel `on_event` drops the
    /// in-flight event (the newest one) and bumping the shared counter,
    /// and the old record remains in the channel. It never blocks.
    #[test]
    fn test_drop_newest_when_channel_full() {
        let (tx, mut rx) = mpsc::channel(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let old = LogRecord {
            ts: 1,
            level: LogstoreLevel::INFO,
            source: Source::proxy(),
            msg: json!({ "message": "old" }),
        };
        tx.try_send(old).expect("capacity for one");

        let layer = build_layer(tx, Source::proxy(), dropped.clone());
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("newest");
        });

        assert_eq!(
            dropped.load(Ordering::SeqCst),
            1,
            "channel full → the newest (in-flight) event is dropped and counted"
        );
        let kept = rx.try_recv().expect("the old record stays in the channel");
        assert_eq!(kept.msg.get("message"), Some(&json!("old")));
        assert!(rx.try_recv().is_err(), "nothing else was enqueued");
    }
}
