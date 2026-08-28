//! The two bounded buffers backing the `StreamLogs` push (plan-195
//! task 6, stage 2a).
//!
//! There are exactly two deques, both written through from **both**
//! feeds (the tamad's own tracing events and the per-model engine
//! tails) on every [`PushRing::push`]:
//!
//! * **in-flight** — [`INFLIGHT_MAX_ENTRIES`] entries or
//!   [`INFLIGHT_MAX_BYTES`] *estimated* bytes: the recent-activity
//!   window (a subset of the replay ring). It is never reset on
//!   connect — a `stream_logs` connection resets nothing.
//! * **replay** — [`REPLAY_MAX_ENTRIES`] entries or [`REPLAY_MAX_BYTES`]
//!   bytes, a GLOBAL FIFO across all sources; what a `stream_logs`
//!   connect replays oldest→newest. On overflow it also drops the
//!   OLDEST and emits a marker under the same throttle. The ONE-shot
//!   handshake frame caps that replay at the ring's TAIL — its
//!   newest [`runtime::MAX_HANDSHAKE_REPLAY`] entries, still emitted
//!   oldest→newest within that tail (see
//!   [`runtime::MAX_HANDSHAKE_REPLAY`] for the cap and its keep-
//!   tail rationale; in short, the proxy's `(instance_id, seq)`
//!   dedupe makes head-entries normally dups anyway, and a quiet
//!   ring's head is the one bounded loss).
//!
//! `seq` is monotonic **per source, per boot** (fresh `0` each boot;
//! `next_seq` hands out `1,2,3,…`). Drop markers take the NEXT seq at
//! emission time and are always emitted *before* the line that triggered
//! the drop, so the stream stays in order and seq increases in-stream.
//!
//! ## Drop markers
//!
//! A drop marker is a [`LoggedLine`] with `dropped: true`,
//! `dropped_count` = lines discarded in that window,
//! `dropped_since_ts` = the earliest dropped line's `ts`, `level: -1` and
//! a JSON `message`. At most ONE marker is emitted per source within
//! [`MARKER_THROTTLE_MS`] (same 5 s throttle as the proxy side) — pending
//! drops accumulate and are flushed together when the throttle clears.
//! When a single window discards ≥ [`DROPPED_WARN_BYTES`], a `warn!` is
//! emitted naming the source.
//!
//! Markers are part of the *live* stream for the triggering connection;
//! they are not stored in the deques (which keep only real lines), and
//! `start_seq_by_source` therefore reports the first surviving real line
//! per source (or `0`).
//!
//! Per-line byte estimate is [`est_bytes`]: `message.len() + 64`.

use std::collections::{BTreeMap, HashMap, VecDeque};

use tama_core::tamad::tamad_service::LoggedLine;

use crate::push::PushEvent;

/// In-flight window: max entries.
pub const INFLIGHT_MAX_ENTRIES: usize = 2048;
/// In-flight window: max estimated bytes (1 MiB).
pub const INFLIGHT_MAX_BYTES: usize = 1024 * 1024;
/// Replay ring: max entries.
pub const REPLAY_MAX_ENTRIES: usize = 25_000;
/// Replay ring: max estimated bytes (10 MiB).
pub const REPLAY_MAX_BYTES: usize = 10 * 1024 * 1024;
/// Drop-marker throttle: at most one marker per source per window (5 s).
pub const MARKER_THROTTLE_MS: i64 = 5_000;
/// Warn threshold: bytes discarded within a single marker window (4 MiB).
pub const DROPPED_WARN_BYTES: usize = 4 * 1024 * 1024;
/// Additive per-line estimate overhead.
const LINE_OVERHEAD: usize = 64;

/// Estimated on-the-wire size of a line (message + fixed overhead).
pub fn est_bytes(line: &LoggedLine) -> usize {
    line.message.len() + LINE_OVERHEAD
}

/// Accumulated, not-yet-flushed drops for one source within the current
/// throttle window.
#[derive(Debug, Default)]
struct PendingDrop {
    count: i64,
    first_ts: i64,
    bytes: usize,
}

impl PendingDrop {
    fn record(&mut self, est: usize, ts: i64) {
        self.count += 1;
        self.bytes += est;
        // First drop of this window anchors `first_ts`; later drops pull
        // it earlier so it reflects the earliest discarded line.
        if self.count == 1 {
            self.first_ts = ts;
        } else {
            self.first_ts = self.first_ts.min(ts);
        }
    }
}

/// The two bounded buffers + per-source seq counters + drop/markers.
///
/// Call [`PushRing::push`] for every captured event; it returns the
/// ordered frames to emit on the *live* stream now (zero or more drop
/// markers followed by the line). Implementation is synchronous and
/// lock-free; the runtime owns it behind a lock.
#[derive(Debug, Default)]
pub struct PushRing {
    in_flight: VecDeque<LoggedLine>,
    in_flight_bytes: usize,
    replay: VecDeque<LoggedLine>,
    replay_bytes: usize,
    /// per-source monotonic seq counter (starts at 0 per boot).
    seqs: HashMap<String, i64>,
    /// per-source pending drops awaiting their 5 s throttle window.
    pending: BTreeMap<String, PendingDrop>,
    /// per-source ts of the last emitted drop marker.
    last_marker_ts: HashMap<String, i64>,
}

impl PushRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Assign and return the next per-source `seq` (monotonic per boot).
    pub fn next_seq(&mut self, source: &str) -> i64 {
        let v = self.seqs.entry(source.to_string()).or_insert(0);
        *v += 1;
        *v
    }

    /// The live-push window, oldest → newest.
    #[cfg(test)]
    pub fn in_flight(&self) -> &VecDeque<LoggedLine> {
        &self.in_flight
    }

    /// The reconnect replay ring, oldest → newest (global FIFO).
    pub fn replay(&self) -> &VecDeque<LoggedLine> {
        &self.replay
    }

    #[cfg(test)]
    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    #[cfg(test)]
    pub fn replay_len(&self) -> usize {
        self.replay.len()
    }

    /// `source → first surviving seq in the replay ring` (0 when the
    /// source has no surviving real line). Oldest-first scan of the FIFO
    /// ring makes first-seen == minimum seq for that source.
    pub fn start_seq_by_source(&self) -> BTreeMap<String, i64> {
        let mut out: BTreeMap<String, i64> = BTreeMap::new();
        for line in &self.replay {
            out.entry(line.source.clone()).or_insert(line.seq);
        }
        out
    }

    /// Record one captured event. Returns the ordered frames to emit on
    /// the live stream now: drop marker(s) (if any became due) followed
    /// by the new line. Both deques are written through; overflow drops
    /// the OLDEST.
    pub fn push(&mut self, evt: PushEvent, now: i64) -> Vec<LoggedLine> {
        let source = evt.source.clone();
        let bytes = evt.message.len() + LINE_OVERHEAD;

        // Make room in the in-flight window (drop oldest).
        while (self.in_flight.len() + 1 > INFLIGHT_MAX_ENTRIES
            || self.in_flight_bytes + bytes > INFLIGHT_MAX_BYTES)
            && !self.in_flight.is_empty()
        {
            if let Some(old) = self.in_flight.pop_front() {
                self.in_flight_bytes = self.in_flight_bytes.saturating_sub(est_bytes(&old));
                self.accumulate_drop(&old);
            }
        }
        // Make room in the replay ring (drop oldest).
        while (self.replay.len() + 1 > REPLAY_MAX_ENTRIES
            || self.replay_bytes + bytes > REPLAY_MAX_BYTES)
            && !self.replay.is_empty()
        {
            if let Some(old) = self.replay.pop_front() {
                self.replay_bytes = self.replay_bytes.saturating_sub(est_bytes(&old));
                self.accumulate_drop(&old);
            }
        }

        // Flush any drop markers whose 5 s window has cleared. These take
        // their (next) seqs BEFORE the line, so in-stream seq order holds.
        let mut out = self.flush_markers(now);

        let line = evt.into_line(self.next_seq(&source));
        self.in_flight.push_back(line.clone());
        self.in_flight_bytes += bytes;
        self.replay.push_back(line.clone());
        self.replay_bytes += bytes;
        out.push(line);
        out
    }

    fn accumulate_drop(&mut self, old: &LoggedLine) {
        self.pending
            .entry(old.source.clone())
            .or_default()
            .record(est_bytes(old), old.ts);
    }

    /// Emit due drop markers (throttled per source). Each marker is built
    /// BEFORE the current line's seq is assigned so its `seq` is lower.
    fn flush_markers(&mut self, now: i64) -> Vec<LoggedLine> {
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(src, p)| {
                p.count > 0
                    && now - self.last_marker_ts.get(*src).copied().unwrap_or(0)
                        >= MARKER_THROTTLE_MS
            })
            .map(|(src, _)| src.clone())
            .collect(); // BTreeMap ⇒ deterministic (source-order)

        let mut out = Vec::new();
        for src in due {
            let Some(p) = self.pending.remove(&src) else {
                continue;
            };
            let mseq = self.next_seq(&src);
            let marker = LoggedLine {
                ts: now,
                level: -1,
                source: src.clone(),
                message: serde_json::json!({
                    "message": format!("{} log lines dropped", p.count)
                })
                .to_string(),
                seq: mseq,
                dropped: true,
                dropped_count: p.count,
                dropped_since_ts: p.first_ts,
            };
            if p.bytes >= DROPPED_WARN_BYTES {
                tracing::warn!(
                    source = %src,
                    dropped_count = p.count,
                    dropped_bytes = p.bytes,
                    "log push discarded over 4 MiB within one marker window"
                );
            }
            self.last_marker_ts.insert(src, now);
            out.push(marker);
        }
        out
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic event with a stable, short message.
    fn evt(source: &str, ts: i64, msg: &str) -> PushEvent {
        PushEvent {
            ts,
            level: 2,
            source: source.to_string(),
            message: format!(r#"{{"message":"{msg}"}}"#),
        }
    }

    fn source_of(line: &LoggedLine) -> &str {
        &line.source
    }

    /// seq is monotonic per source, per boot: independent counters.
    #[test]
    fn test_seq_numbering_across_sources() {
        let mut ring = PushRing::new();
        let now = 1_000;

        let a1 = ring.push(evt("tamad", now, "a1"), now).pop().unwrap();
        let b1 = ring.push(evt("model:m", now, "b1"), now).pop().unwrap();
        let a2 = ring.push(evt("tamad", now, "a2"), now).pop().unwrap();

        assert_eq!(source_of(&a1), "tamad");
        assert_eq!(a1.seq, 1);
        assert_eq!(source_of(&b1), "model:m");
        assert_eq!(b1.seq, 1, "second source also starts at 1");
        assert_eq!(source_of(&a2), "tamad");
        assert_eq!(a2.seq, 2, "tamad advances independently");
    }

    /// When the in-flight window (2 048 entries) is full, the OLDEST line
    /// is dropped and a `dropped` marker is emitted BEFORE the new line,
    /// with the dropped line's ts in `dropped_since_ts`.
    #[test]
    fn test_in_flight_drop_oldest_and_marker_order() {
        let mut ring = PushRing::new();
        let now = 1_000_000;

        // Fill to capacity (no drops yet).
        for i in 0..INFLIGHT_MAX_ENTRIES {
            let _ = ring.push(evt("tamad", now + i as i64, "x"), now);
        }
        assert_eq!(ring.in_flight_len(), INFLIGHT_MAX_ENTRIES);
        assert!(
            ring.in_flight().iter().all(|l| !l.dropped),
            "no markers after filling exactly to cap"
        );

        // One more event → overflow: oldest dropped, marker emitted first.
        let frames = ring.push(evt("tamad", now + INFLIGHT_MAX_ENTRIES as i64, "last"), now);
        assert_eq!(frames.len(), 2, "marker + new line");
        let (marker, line) = (&frames[0], &frames[1]);
        assert!(marker.dropped, "first frame is a drop marker");
        assert_eq!(marker.dropped_count, 1);
        assert_eq!(marker.dropped_since_ts, now, "oldest (now+0) was dropped");
        assert_eq!(marker.level, -1);
        assert!(line.seq > marker.seq, "marker seq precedes the line");
        assert!(!line.dropped);
        assert_eq!(ring.in_flight_len(), INFLIGHT_MAX_ENTRIES, "still bounded");

        // The oldest surviving line is the second one we pushed (now+1).
        let front = ring.in_flight().front().unwrap();
        assert!(!front.dropped);
    }

    /// Within the 5 s throttle window drops ACCUMULATE and are flushed as
    /// ONE marker; a later flush (after the window) reports the combined
    /// count.
    #[test]
    fn test_marker_throttle_accumulates_pending_drops() {
        let mut ring = PushRing::new();
        let t0 = 2_000_000;

        for _ in 0..INFLIGHT_MAX_ENTRIES {
            let _ = ring.push(evt("tamad", t0, "x"), t0);
        }
        // Drop #1 (first drop of the boot of this source ⇒ immediate
        // flush since no prior marker ts).
        let f1 = ring.push(evt("tamad", t0, "y"), t0);
        assert_eq!(f1.len(), 2);
        assert!(f1[0].dropped);
        assert_eq!(f1[0].dropped_count, 1);

        // Drop #2 within the same 5 s window (t0 + 1 s) ⇒ NOT flushed.
        let f2 = ring.push(evt("tamad", t0, "z"), t0 + 1_000);
        assert_eq!(f2.len(), 1, "within throttle ⇒ no marker yet");
        assert!(!f2[0].dropped);

        // Drop #3 after the 5 s window has cleared (t0 + 6 s) ⇒ the two
        // pending drops (the one from step 2 folded in) flush together.
        let f3 = ring.push(evt("tamad", t0, "w"), t0 + 6_000);
        assert_eq!(f3.len(), 2);
        let (m, l) = (&f3[0], &f3[1]);
        assert!(m.dropped);
        assert_eq!(m.dropped_count, 2, "pending drops combined");
        assert_eq!(m.dropped_since_ts, t0, "earliest dropped ts kept");
        assert!(!l.dropped);
    }

    /// The replay ring is a GLOBAL FIFO: lines from several sources remain
    /// in strict arrival (push) order, and `start_seq_by_source` reports
    /// the first surviving seq per source.
    #[test]
    fn test_replay_global_fifo_order() {
        let mut ring = PushRing::new();
        let t0 = 3_000_000;
        let push_order = ["tamad", "model:a", "tamad", "model:b", "model:a"];

        let mut got = Vec::new();
        for (i, src) in push_order.iter().enumerate() {
            let frames = ring.push(evt(src, t0 + i as i64, "x"), t0);
            got.push(frames.last().unwrap().source.clone());
        }
        assert_eq!(
            got,
            push_order.into_iter().map(String::from).collect::<Vec<_>>()
        );

        let seqs = ring.start_seq_by_source();
        // Per-source seq (plan): each source counts from its own
        // per-boot counter — several sources can both report first-
        // surviving seq `1`.
        assert_eq!(seqs.get("tamad"), Some(&1));
        assert_eq!(seqs.get("model:a"), Some(&1));
        assert_eq!(seqs.get("model:b"), Some(&1));
        assert_eq!(seqs.len(), 3);

        // The per-source counters advanced independently: tamad's
        // second line took seq 2, model:a's second line took seq 2,
        // while model:b's (last) line is still seq 1.
        let mut dids: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
        for l in ring.replay().iter() {
            // overwriting in order ⇒ the LAST line per source wins.
            dids.insert(&l.source, l.seq);
        }
        assert_eq!(dids.get("tamad"), Some(&2));
        assert_eq!(dids.get("model:a"), Some(&2));
        assert_eq!(dids.get("model:b"), Some(&1));
    }

    /// The replay ring is the BYTES-bounded overflow path: a source whose
    /// 2 MiB lines can't all fit (10 MiB cap) drops the OLDEST. Drop
    /// Markers are emitted in-stream (consuming that source's seq), so the
    /// oldest *surviving* line's seq is what `start_seq_by_source` reports.
    #[test]
    fn test_replay_byte_cap_drop_oldest() {
        let ring = PushRing::new();
        let seqs = ring.start_seq_by_source();
        assert!(seqs.is_empty(), "empty ring ⇒ no sources");
        drop(ring);

        let mut ring = PushRing::new();
        let t0 = 4_000_000;
        let big = "y".repeat(2 * 1024 * 1024); // 2 MiB each
        for i in 0..6 {
            let _ = ring.push(
                PushEvent {
                    ts: t0 + i,
                    level: 2,
                    source: "model:big".to_string(),
                    message: big.clone(),
                },
                t0,
            );
        }
        // 10 MiB / 2 MiB ⇒ at most 4 lines fit (5 would exceed the cap).
        let lines: Vec<&LoggedLine> = ring.replay().iter().collect();
        assert_eq!(lines.len(), 4, "replay is byte-bounded to 4 lines");
        // Drop the oldest ⇒ the first surviving line is NOT seq 1.
        let min_surviving: i64 = lines.iter().map(|l| l.seq).min().unwrap();
        assert!(
            min_surviving > 1,
            "oldest line was dropped (got {min_surviving})"
        );
        // The ring reports the oldest surviving seq for the source.
        let seqs = ring.start_seq_by_source();
        assert_eq!(seqs.get("model:big"), Some(&min_surviving));
    }

    /// A source whose lines have ALL aged out of the replay ring (the
    /// FIFO entry cap evicted its head while another source kept
    /// writing) is absent from `start_seq_by_source` — i.e. a 0-start
    /// for any new consumer of that source.
    #[test]
    fn test_start_seq_zero_when_source_aged_out() {
        let mut ring = PushRing::new();
        let t0 = 5_000_000i64;
        ring.push(evt("model:a", t0, "a1"), t0);
        for i in 0..(REPLAY_MAX_ENTRIES as i64 + 100) {
            ring.push(evt("model:b", t0 + i, "x"), t0 + i);
        }
        assert!(
            ring.replay().iter().all(|l| l.source == "model:b"),
            "source A aged out of the FIFO completely"
        );
        let seqs = ring.start_seq_by_source();
        assert!(!seqs.contains_key("model:a"));
        assert_eq!(
            seqs.get("model:a").copied().unwrap_or(0),
            0,
            "no surviving line ⇒ a new consumer starts from 0"
        );
        assert_eq!(seqs.len(), 1);
    }
}
