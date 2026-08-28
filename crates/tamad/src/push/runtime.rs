//! The `LogPushRuntime`: the tamad's `StreamLogs` server-side feed
//! (plan-195 task 6, stage 2a).
//!
//! ## Direction (reads as "push", but the tamad is the gRPC *server*)
//!
//! The tamad NEVER dials: no tonic client, no endpoint, no reconnect
//! loop (the proxy dials; the backoff / UNIMPLEMENTED-stop machinery
//! is task 7's, on the proxy side). The runtime is the BUFFER OWNER:
//! it holds the two bounded rings ([`super::ring`]) behind the feed
//! channel ([`super::EVENT_CHANNEL_CAP`]) and, when the proxy opens a
//! `stream_logs` RPC, [`LogPushRuntime::stream`] yields the ordered
//! sequire: `StreamInit{instance_id, start_seq_by_source}`, then the
//! NEWEST [`MAX_HANDSHAKE_REPLAY`] lines of the replay ring
//! (oldest→newest within that tail — see `MAX_HANDSHAKE_REPLAY`),
//! then live frames as the absorb loop
//! records them — until the client disconnects (drop the returned
//! receiver; the unregistered stream is pruned lazily on the next
//! publish).
//!
//! Nothing is reset on connect: `instance_id` is per-boot
//! (`Uuid::new_v4()` at construction) and per-source seq counters
//! keep counting, so every (re)dial replays the ring under the same
//! identity — the proxy (task 7) dedups on `(instance_id, seq)`.
//!
//! ## Capability gate
//!
//! The register response's `supports_stream_logs` (old proxy ⇒ field
//! absent ⇒ `false`) feeds the absorb loop's flag
//! ([`CapabilityRx`]). While the flag is off the feed channel simply
//! CYCLES (bounded; producers `try_send`-drop newest) and the rings
//! are never written — bounded memory, NO spurious drop markers, and
//! nothing assumes a live consumer. When the flag turns on (register
//! succeeded against a v2 proxy), the rings start filling from seq 1.
//!
//! ## Ordering / fan-out
//!
//! One core lock wraps the rings AND the open-stream registry so a
//! connecting stream snapshots the rings and registers its live
//! channel atomically — a published frame can never land both in a
//! handshake snapshot and in the fan-out. Per-stream channels are
//! bounded ([`super::EVENT_CHANNEL_CAP`]); a full one drops its
//! NEWEST frame (try_send) and stays registered — it never blocks the
//! absorb loop, so the tamad never stalls on a slow client.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use tama_core::tamad::tamad_service::{
    stream_log_message::Kind, LoggedLine, StreamInit, StreamLogMessage,
};

use super::ring::PushRing;
use super::{now_unix_ms, PushEvent, EVENT_CHANNEL_CAP};

/// Upper bound on the replay lines carried in a `stream_logs`
/// handshake (init + replay ≤ cap+1 frames total). A full ring
/// re-pipes until the proxy catches up; 10_000 is safe for the gRPC
/// buffer.
///
/// The cap keeps the **TAIL of the window**: the NEWEST 10_000
/// entries of the 25_000-entry ring, still emitted oldest→newest
/// within that tail (never the head — the ring's own overflow policy
/// is drop-OLDEST, so the tail is also the most valuable chunk). The
/// head is normally harmless to the proxy: it dedups on
/// `(instance_id, seq)`, so lines it already received on a live
/// stream would be dups anyway, and seqs are per-source monotonic —
/// omitting old head entries leaves the proxy's dedupe state
/// unaffected. One honest hole: lines that entered the ring while no
/// proxy was connected, on a ring quiet enough that the 25k FIFO
/// never evicts them further, stay unreplayed — a proxy dialing only
/// after the ring crossed the cap never sees them. That loss is
/// bounded (≤ ring cap − handshake cap lines) and matches the ring's
/// own drop-oldest policy.
pub const MAX_HANDSHAKE_REPLAY: usize = 10_000;

/// Peer capability flag (see the module docs): the absorb loop polls
/// it on every frame.
pub type CapabilityRx = watch::Receiver<bool>;

/// A capability gate fixed at `v` (no register / tests).
#[cfg(test)]
pub fn fixed_capability(v: bool) -> CapabilityRx {
    let (_tx, rx) = watch::channel(v);
    rx
}

/// The rings + open-stream registry, behind ONE lock (see the module
/// docs for the ordering argument).
struct Core {
    ring: PushRing,
    /// One bounded live channel per connected `stream_logs` stream.
    streams: Vec<mpsc::Sender<StreamLogMessage>>,
}

impl Core {
    /// Publish one ordered batch (zero or more drop markers + a line)
    /// to every registered stream.
    fn publish(&mut self, frames: &[LoggedLine]) {
        for frame in frames {
            let msg = StreamLogMessage {
                kind: Some(Kind::Line(frame.clone())),
            };
            // A CLOSED channel (client gone) is pruned; a FULL one
            // (slow client) stays — its newest frames are dropped,
            // never block.
            self.streams.retain(|tx| match tx.try_send(msg.clone()) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            });
        }
    }
}

/// The tamad's `StreamLogs` runtime (plan-195 task 6). Construct with
/// [`LogPushRuntime::spawn`] (with an absorb loop) or
/// [`LogPushRuntime::dormant`] (no loop — test servers / e2e
/// fixtures).
#[derive(Clone)]
pub struct LogPushRuntime {
    /// Per-boot push identity (NOT the pid; stable across every
    /// (re)connect and proxy restart for the life of the process).
    instance_id: Uuid,
    core: Arc<Mutex<Core>>,
    /// Absorb-loop stop signal (process exit).
    cancel_tx: watch::Sender<bool>,
}

impl LogPushRuntime {
    /// Construct WITHOUT an absorb loop: nothing fills the ring, but
    /// [`stream`](Self::stream) still serves (init + whatever the
    /// ring holds + no live frames). Used by local test servers and
    /// e2e fixtures that need a `stream_logs` implementation
    /// obligation without a live feed.
    #[cfg(test)]
    pub fn dormant() -> Arc<Self> {
        let (cancel_tx, _) = watch::channel(false);
        Arc::new(Self {
            instance_id: Uuid::new_v4(),
            core: Arc::new(Mutex::new(Core {
                ring: PushRing::new(),
                streams: Vec::new(),
            })),
            cancel_tx,
        })
    }

    /// Construct the runtime and start its absorb loop, which drains
    /// `feed_rx` into the rings (gated on `capability`) and publishes
    /// each ordered frame to every connected stream.
    pub fn spawn(feed_rx: mpsc::Receiver<PushEvent>, capability: CapabilityRx) -> Arc<Self> {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let rt = Arc::new(Self {
            instance_id: Uuid::new_v4(),
            core: Arc::new(Mutex::new(Core {
                ring: PushRing::new(),
                streams: Vec::new(),
            })),
            cancel_tx,
        });
        let core = Arc::clone(&rt.core);
        tokio::spawn(absorb_loop(core, feed_rx, capability, cancel_rx));
        rt
    }

    /// Per-boot push identity (stable across reconnects / proxy
    /// restarts; task 7 keys replay + dedup on this).
    #[cfg(test)]
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    /// Open a `stream_logs` stream. Under the core lock, snapshots
    /// the `StreamInit` + the NEWEST [`MAX_HANDSHAKE_REPLAY`] replay
    /// lines — the tail of the ring, still oldest→newest within that
    /// window (see `MAX_HANDSHAKE_REPLAY` for the cap rationale and
    /// its dedupe interaction) — and registers the
    /// bounded live channel — so the resulting sequire is exactly
    /// `init, replay…, live…` with no duplicated or skipped frame
    /// across the boundary. Drop the returned `Receiver` to
    /// disconnect (pruned lazily on the next publish).
    pub fn stream(&self) -> (Vec<StreamLogMessage>, mpsc::Receiver<StreamLogMessage>) {
        let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        let handshake: Vec<StreamLogMessage> = {
            let mut core = self.core.lock().unwrap();
            let start_seq: HashMap<String, i64> =
                core.ring.start_seq_by_source().into_iter().collect();
            // Register the live channel BEFORE releasing the lock:
            // the snapshot and the registration are atomic, so a
            // frame recorded after either belongs only to the
            // fan-out, and one recorded before either belongs only to
            // the snapshot.
            let mut out = vec![StreamLogMessage {
                kind: Some(Kind::Init(StreamInit {
                    instance_id: self.instance_id.to_string(),
                    start_seq_by_source: start_seq,
                })),
            }];
            core.streams.push(tx);
            // Keep the NEWEST `MAX_HANDSHAKE_REPLAY` lines: reverse,
            // take the tail, re-reverse to restore oldest→newest for
            // the wire. The ring FIFO is oldest→newest, so a plain
            // `take` would (wrongly) keep the head and drop the
            // newest lines of a full ring.
            let tail: Vec<&LoggedLine> = core
                .ring
                .replay()
                .iter()
                .rev()
                .take(MAX_HANDSHAKE_REPLAY)
                .collect();
            for line in tail.iter().rev() {
                out.push(StreamLogMessage {
                    kind: Some(Kind::Line((*line).clone())),
                });
            }
            out
        };
        (handshake, rx)
    }

    /// Stop the absorb loop (process exit).
    pub fn shutdown(&self) {
        let _ = self.cancel_tx.send(true);
    }
}

/// Drain the feed into the rings (capability-gated) and publish each
/// ordered frame to every connected stream.
async fn absorb_loop(
    core: Arc<Mutex<Core>>,
    mut feed_rx: mpsc::Receiver<PushEvent>,
    mut capability: CapabilityRx,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = cancel.changed() => return,
            maybe = feed_rx.recv() => {
                let Some(evt) = maybe else {
                    return; // all producers dropped
                };
                if !*capability.borrow_and_update() {
                    // Capability off: CYCLE the channel. Bounded
                    // memory; producers try_send-drop newest; the
                    // rings stay untouched (no spurious drop marks,
                    // no live consumer assumed).
                    continue;
                }
                let mut core = core.lock().unwrap();
                let frames = core.ring.push(evt, now_unix_ms());
                core.publish(&frames);
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn event(source: &str, msg: &str) -> PushEvent {
        PushEvent {
            ts: now_unix_ms(),
            level: 2,
            source: source.to_string(),
            message: format!(r#"{{"message":"{msg}"}}"#),
        }
    }

    /// Poll `cond` at 5 ms steps until `timeout`; returns the final
    /// observation.
    async fn wait_for(mut cond: impl FnMut() -> bool, timeout_ms: u64) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            if cond() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        cond()
    }

    /// Drain N events into the ring and wait until they were absorbed.
    async fn settle(rt: &LogPushRuntime, feed_tx: &mpsc::Sender<PushEvent>, n: usize) {
        for i in 0..n {
            feed_tx
                .try_send(event("tamad", &format!("line-{i}")))
                .unwrap();
        }
        assert!(
            wait_for(|| rt.core.lock().unwrap().ring.replay_len() == n, 2_000).await,
            "ring absorbed {n} events"
        );
    }

    /// `instance_id` is per-boot: stable across every stream/dial of
    /// THE SAME runtime; distinct between runtimes (boots). Testable
    /// without any network — this is the property the proxy (task 7)
    /// keys replay + dedup on.
    #[test]
    fn test_instance_id_per_boot() {
        let a = LogPushRuntime::dormant();
        let b = LogPushRuntime::dormant();
        let a1 = a.instance_id().to_string();
        // A fresh dial on the same runtime sees the same id:
        let (h, _rx) = a.stream();
        let id_at_connect = match &h[0].kind {
            Some(Kind::Init(i)) => i.instance_id.clone(),
            other => panic!("first frame is not an init: {other:?}"),
        };
        assert_eq!(a1, a.instance_id().to_string());
        assert_eq!(a1, id_at_connect, "same runtime ⇒ same per-boot id on wire");
        assert_ne!(
            a1,
            b.instance_id().to_string(),
            "different boot ⇒ different id"
        );
    }

    /// Bounded without a consumer: gate OFF, no proxy dial. Feed 2×
    /// cap events: the channel clips at the cap (try_send drops
    /// newest), the rings stay EMPTY, and a stream opened mid-flood
    /// carries init only (empty replay, empty start_seq).
    #[tokio::test]
    async fn test_bounded_without_consumer_gate_off() {
        let (feed_tx, feed_rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        let rt = LogPushRuntime::spawn(feed_rx, fixed_capability(false));

        for _ in 0..(2 * EVENT_CHANNEL_CAP) {
            let _ = feed_tx.try_send(event("tamad", "x"));
        }
        assert!(
            feed_tx.try_send(event("tamad", "x")).is_err(),
            "channel full ⇒ try_send drops newest"
        );
        // Let (buggy) absorb round-trips happen; settle that the ring
        // stays empty.
        assert!(
            !wait_for(|| rt.core.lock().unwrap().ring.replay_len() > 0, 200).await,
            "gate off ⇒ the ring is never written"
        );
        let (handshake, mut rx) = rt.stream();
        assert_eq!(handshake.len(), 1, "init only — nothing replayed");
        let init = match &handshake[0].kind {
            Some(Kind::Init(i)) => i,
            other => panic!("first frame is not an init: {other:?}"),
        };
        assert_eq!(init.instance_id, rt.instance_id().to_string());
        assert!(init.start_seq_by_source.is_empty());
        let r = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(r.is_err(), "no live frame while the gate is off");
        rt.shutdown();
    }

    /// Handshake replay frame cap (plan task 6): even a FULL ring
    /// (25k entries) never puts more than
    /// [`MAX_HANDSHAKE_REPLAY`] lines in the init handshake — safety
    /// for the gRPC buffer; the remainder is simply not re-served
    /// this dial. The cap keeps the TAIL of the window — the
    /// NEWEST 10k of the surviving 25k (not the oldest 10k) —
    /// emitted oldest→newest within that tail.
    #[tokio::test]
    async fn test_handshake_replay_frame() {
        let (feed_tx, feed_rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        let rt = LogPushRuntime::spawn(feed_rx, fixed_capability(true));
        // Overfill the 25k-entry ring (pace against the 1024-deep
        // feed channel — the absorb loop clips it to the ring).
        for i in 0..(super::super::ring::REPLAY_MAX_ENTRIES + 50) {
            let evt = event("tamad", &format!("x-{i}"));
            while feed_tx.try_send(evt.clone()).is_err() {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        }
        // Identify every line by its seq stamp in the message:
        // the 25 050 events are x-0 .. x-25049; the 25k ring holds
        // x-50 .. x-25049, tail-of-window is x-15050 .. x-25049.
        // Wait for the newest to be absorbed (not just "ring is
        // full", which is already true mid-push through the
        // 50-event over-fill).
        assert!(
            wait_for(
                || {
                    let core = rt.core.lock().unwrap();
                    let ring = core.ring.replay();
                    ring.len() == super::super::ring::REPLAY_MAX_ENTRIES
                        && ring.back().is_some_and(|l| l.message.contains("x-25049"))
                },
                15_000
            )
            .await,
            "ring full at 25k entries, newest absorbed"
        );

        let (handshake, _rx) = rt.stream();
        assert_eq!(
            handshake.len(),
            1 + MAX_HANDSHAKE_REPLAY,
            "init + frame-capped replay"
        );
        // The surviving frames are EXACTLY the newest 10k of the
        // surviving 25k, emitted oldest→newest within the tail, with
        // strictly ascending seq (single source; markers may take
        // seqs between them — never break ordering).
        let got: Vec<(i64, String)> = handshake[1..]
            .iter()
            .map(|m| match m.kind.as_ref().unwrap() {
                Kind::Line(l) => (l.seq, l.message.clone()),
                other => panic!("replay frame not a line: {other:?}"),
            })
            .collect();
        // The window must be EXACTLY the newest 10k of the surviving
        // 25k, emitted oldest→newest within the tail:
        let expected_msgs: Vec<String> = (15_050..25_050)
            .map(|i| format!(r#"{{"message":"x-{i}"}}"#))
            .collect();
        let got_msgs: Vec<String> = got.iter().map(|(_, m)| m.clone()).collect();
        assert_eq!(
            got_msgs, expected_msgs,
            "window is the newest 10k of the ring (x-15050 .. x-25049), oldest→newest"
        );
        // The oldest lines the ring held (x-50 … x-15049) are NOT
        // re-served on this dial:
        for absent in ["x-50", "x-10000", "x-15049"] {
            assert!(
                !got_msgs
                    .iter()
                    .any(|m| m == &format!(r#"{{"message":"{absent}"}}"#)),
                "{absent} (older than the window) must not be replayed"
            );
        }
        // seq strictly ascending within the window (single source;
        // markers may take seqs between lines — never break order):
        let seqs: Vec<i64> = got.iter().map(|(s, _)| *s).collect();
        for w in seqs.windows(2) {
            assert!(w[0] < w[1], "seq ascending within the window");
        }
        rt.shutdown();
    }

    /// Connect → `StreamInit` FIRST (instance_id + start_seq), then
    /// the ring replayed oldest→newest with continuing per-source
    /// seqs, then live appends arrive on the live half in order.
    #[tokio::test]
    async fn test_init_then_replay_then_live() {
        let (feed_tx, feed_rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        let rt = LogPushRuntime::spawn(feed_rx, fixed_capability(true));
        settle(&rt, &feed_tx, 3).await;

        let (handshake, mut rx) = rt.stream();
        assert_eq!(handshake.len(), 4, "1 init + 3 replayed lines");
        // Init first, with the per-boot id and per-source starts.
        let init = match &handshake[0].kind {
            Some(Kind::Init(i)) => i,
            other => panic!("first frame is not an init: {other:?}"),
        };
        assert_eq!(init.instance_id, rt.instance_id().to_string());
        assert_eq!(init.start_seq_by_source.get("tamad"), Some(&1));
        // Replay oldest→newest, continuing per-source seqs.
        let replay: Vec<(i64, &str)> = handshake[1..]
            .iter()
            .map(|m| match m.kind.as_ref().unwrap() {
                Kind::Line(l) => (l.seq, l.message.as_str()),
                other => panic!("replay frame is not a line: {other:?}"),
            })
            .collect();
        assert_eq!(
            replay,
            [
                (1, r#"{"message":"line-0"}"#),
                (2, r#"{"message":"line-1"}"#),
                (3, r#"{"message":"line-2"}"#)
            ]
        );

        // Live: a new event recorded AFTER connect arrives on the
        // live half with the next per-source seq (never replayed
        // twice).
        feed_tx.try_send(event("tamad", "live-1")).unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("live frame within 2s")
            .expect("stream still open");
        let Some(Kind::Line(l)) = got.kind else {
            panic!("live frame must be a line: {got:?}");
        };
        assert_eq!(l.seq, 4, "live takes the next per-source seq");
        assert!(l.message.contains("live-1"));
        rt.shutdown();
    }

    /// Reconnect semantics: a second stream of the SAME runtime sees
    /// the SAME `instance_id`, and its replay now includes the lines
    /// recorded during the first stream's life — nothing was reset by
    /// the first connect/disconnect.
    #[tokio::test]
    async fn test_reconnect_same_instance_replay_grows() {
        let (feed_tx, feed_rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        let rt = LogPushRuntime::spawn(feed_rx, fixed_capability(true));
        // One "tamad" line, one "model:m" line.
        feed_tx.try_send(event("tamad", "a")).unwrap();
        assert!(wait_for(|| rt.core.lock().unwrap().ring.replay_len() == 1, 2_000).await);
        feed_tx
            .try_send(PushEvent {
                ts: now_unix_ms(),
                level: -1,
                source: super::super::model_source("m"),
                message: r#"{"message":"b"}"#.to_string(),
            })
            .unwrap();
        assert!(wait_for(|| rt.core.lock().unwrap().ring.replay_len() == 2, 2_000).await);

        let (h1, rx1) = rt.stream();
        drop(rx1); // the first proxy "disconnects"

        let (h2, _rx2) = rt.stream();
        let mut ids = Vec::new();
        for h in [&h1, &h2] {
            ids.push(match &h[0].kind {
                Some(Kind::Init(i)) => i.instance_id.clone(),
                other => panic!("init first: {other:?}"),
            });
        }
        assert_eq!(
            ids[0], ids[1],
            "per-boot: both inits carry the same instance_id"
        );
        assert_eq!(ids[0], rt.instance_id().to_string());
        let init2 = match &h2[0].kind {
            Some(Kind::Init(i)) => i,
            other => panic!("not init: {other:?}"),
        };
        assert_eq!(init2.start_seq_by_source.get("tamad"), Some(&1));
        assert_eq!(init2.start_seq_by_source.get("model:m"), Some(&1));
        assert_eq!(h2.len(), 1 + 2, "init + 2 replayed lines (global FIFO)");
        rt.shutdown();
    }
}
