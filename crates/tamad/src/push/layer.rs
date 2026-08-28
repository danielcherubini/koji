//! The tamad-side [`tracing::Layer`] for structured log push
//! (plan-195 task 6, stage 2a).
//!
//! [`PushLogLayer`] encodes each event into a [`PushEvent`] and
//! `try_send`s it over the bounded channel into the
//! [`crate::push::runtime::LogPushRuntime`]. Field encoding reuses the
//! proxy side's [`FieldValueVisitor`] (plan-195 task 2) so the JSON doc
//! shape is identical on both sides:
//! `{"message": <event text>, "target": <module>, ...named fields}`.
//!
//! * `source` is always [`super::TAMAD_SOURCE`]; `level` is the real
//!   tracing level (0..4 = TRACE..ERROR).
//! * `ts` is capture time, unix ms.
//! * `on_event` NEVER blocks — channel full ⇒ the in-flight (newest)
//!   event is dropped and the channel stays bounded
//!   ([`super::EVENT_CHANNEL_CAP`]) even when the runtime is inactive.

use crate::push::{now_unix_ms, PushEvent, TAMAD_SOURCE};
use serde_json::Value;
use tama_core::logstore::layer::FieldValueVisitor;
use tokio::sync::mpsc;
use tracing::Level as TracingLevel;
use tracing::Subscriber;
use tracing_subscriber::Layer;

/// Tracing layer feeding the `StreamLogs` push channel.
/// `Clone` is cheap — every clone shares the same channel.
#[derive(Clone)]
pub struct PushLogLayer {
    tx: mpsc::Sender<PushEvent>,
}

/// Build a layer pushing events to the `PushEvent` channel endpoint `tx`.
pub fn build_layer(tx: mpsc::Sender<PushEvent>) -> PushLogLayer {
    PushLogLayer { tx }
}

impl<S> Layer<S> for PushLogLayer
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
        fields.insert("message".to_string(), Value::String(message));
        fields.insert(
            "target".to_string(),
            Value::String(event.metadata().target().to_string()),
        );

        let level = match *event.metadata().level() {
            TracingLevel::TRACE => 0,
            TracingLevel::DEBUG => 1,
            TracingLevel::INFO => 2,
            TracingLevel::WARN => 3,
            TracingLevel::ERROR => 4,
        };

        // Never block the hot path: channel full ⇒ drop this (newest)
        // event. A `Closed` error means the runtime is gone — nothing to
        // do either way.
        let _ = self.tx.try_send(PushEvent {
            ts: now_unix_ms(),
            level,
            source: TAMAD_SOURCE.to_string(),
            message: Value::Object(fields).to_string(),
        });
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    fn harness(
        cap: usize,
    ) -> (
        mpsc::Sender<PushEvent>,
        mpsc::Receiver<PushEvent>,
        PushLogLayer,
    ) {
        let (tx, rx) = mpsc::channel(cap);
        (tx.clone(), rx, build_layer(tx))
    }

    /// An `info!` with named fields flattens into the same JSON doc shape
    /// as the proxy side: message/target first-class, fields as values.
    #[test]
    fn test_layer_encodes_document_shape() {
        let (_tx, mut rx, layer) = harness(16);
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(model = "mini", depth = 12, "hello push");
        });

        let evt = rx.try_recv().expect("event enqueued");
        assert!(evt.ts > 0, "capture-time unix ms");
        assert_eq!(evt.level, 2, "INFO maps to 2");
        assert_eq!(evt.source, TAMAD_SOURCE);

        let doc: serde_json::Value =
            serde_json::from_str(&evt.message).expect("message is a JSON doc");
        assert_eq!(doc.get("message"), Some(&Value::from("hello push")));
        assert_eq!(
            doc.get("target").and_then(|v| v.as_str()).map(str::len),
            Some(module_path!().len())
        );
        assert_eq!(doc.get("model"), Some(&Value::from("mini")));
        assert_eq!(doc.get("depth"), Some(&Value::from(12)));
        assert_eq!(doc.as_object().expect("object doc").len(), 4);
    }

    /// All five tracing levels map to 0..4.
    #[test]
    fn test_layer_level_mapping() {
        let (_tx, mut rx, layer) = harness(16);
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::trace!("t");
            tracing::debug!("d");
            tracing::info!("i");
            tracing::warn!("w");
            tracing::error!("e");
        });
        let levels: Vec<i32> = (0..5)
            .map(|_| rx.try_recv().expect("event").level)
            .collect();
        assert_eq!(levels, vec![0, 1, 2, 3, 4]);
    }

    /// The channel stays bounded with bounded memory: after many more
    /// events than its capacity, recv.len() ≤ capacity — even with NO
    /// runtime draining it (the acceptance criterion for running without
    /// a proxy).
    #[test]
    fn test_channel_stays_bounded_without_runtime() {
        let cap = super::super::EVENT_CHANNEL_CAP;
        assert_eq!(cap, 1024, "capacity matches the proxy-side channel");
        let (_tx, rx, layer) = harness(cap);
        let subscriber = Registry::default().with(layer);

        let n = cap * 5;
        tracing::subscriber::with_default(subscriber, || {
            for i in 0..n {
                tracing::info!(i = i, "flood");
            }
        });

        assert!(
            rx.len() <= cap,
            "channel must stay ≤ {cap} (got {})",
            rx.len()
        );
        // It filled up (nothing drained it) — the overflow dropped newest.
        assert_eq!(rx.len(), cap, "channel is full; older events kept");
    }
}
