//! Replay dedupe for the tamad `StreamLogs` ingest (plan-195 task 7).
//!
//! PURE — no IO, no async-runtime dependency: a plain in-memory
//! [`DedupState`] guarded by ONE mutex on the ingest side (the mutex lives
//! in `crates/tama-core/src/tamad/stream_logs.rs`, never here).
//!
//! ## Rules
//!
//! `on_message(tamad, source, instance_id, seq)`:
//!
//! 1. First contact for `(tamad, source)` → [`Decision::Fresh`]; the id
//!    becomes `current`, `seq` the watermark.
//! 2. `instance_id == current`: `seq <= last` → [`Decision::Duplicate`];
//!    `seq > last` → [`Decision::Fresh`] (watermark advances to `seq`).
//! 3. `instance_id == expected` (announced by the latest `StreamInit`) AND
//!    `!= current` → [`Decision::NewInstance`]: `current = (instance_id,
//!    seq)`; the previous current id moves into `seen_olds`.
//! 4. `instance_id` in none of `{current, expected, seen_olds}` →
//!    [`Decision::OldInstanceReplay`]: accept THIS message only (do NOT
//!    update `last`); add the id to `seen_olds` so later lines of the same
//!    old instance hit rule 5.
//! 5. `instance_id` in `seen_olds` → [`Decision::Duplicate`].
//!
//! On connection-lost: keep ALL state (no reset) — late replays are
//! handled by rules 4/5. Memory: `seen_olds` grows at the tamad reboot
//! rate — single digits for years, no cap needed.
//!
//! NOTE: `StreamInit` ALWAYS precedes the lines of a (re)connected stream,
//! so `expected` is populated before any line is judged — this keeps a
//! genuine new boot (rule 3) from being misread as a late replay (rule 4).

use std::collections::{HashMap, HashSet};

/// The disposition of one ingested log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// New line — enqueue it.
    Fresh,
    /// Already seen (replayed `(instance_id, seq)`, or a line from a
    /// demoted old instance) — skip.
    Duplicate,
    /// An init-announced newer boot instance — enqueue, and the watermark
    /// moves to `(instance_id, seq)`.
    NewInstance,
    /// Late lines from an unannounced old instance — accept this one line
    /// only.
    OldInstanceReplay,
}

/// Instance/watermark state for one `(tamad, source)` pair.
#[derive(Debug, Clone)]
pub struct InstanceState {
    /// `(current instance_id, last accepted seq of that instance)`.
    pub(crate) current: (String, i64),
    /// Instance ids demoted when a newer (init-announced) instance took
    /// over; any further lines from them are duplicates.
    pub(crate) seen_olds: HashSet<String>,
}

/// In-memory replay dedupe keyed on `(tamad, source) →
/// { instance_id → last_seq }`.
#[derive(Debug, Clone, Default)]
pub struct DedupState {
    /// current-lines: `tamad_id → source → instance watermark state`.
    pub(crate) current_lines: HashMap<String, HashMap<String, InstanceState>>,
    /// expected: `tamad_id → source → instance_id from the latest
    /// StreamInit`.
    pub(crate) expected: HashMap<String, HashMap<String, String>>,
}

impl DedupState {
    /// New, empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// A `StreamInit` for `(tamad, source)` announced `instance_id` as the
    /// expected instance from now on. This is an `insert`, not an
    /// `or_insert`: the LATEST init wins (a late/replayed init must
    /// overwrite the recorded expectation).
    pub fn on_init(&mut self, tamad: &str, source: &str, instance_id: &str) {
        let per_source = self.expected.entry(tamad.to_string()).or_default();
        per_source.insert(source.to_string(), instance_id.to_string());
    }

    /// Judge one ingested line; see the module docs for the rules.
    pub fn on_message(
        &mut self,
        tamad: &str,
        source: &str,
        instance_id: &str,
        seq: i64,
    ) -> Decision {
        // Rule 1: first contact for (tamad, source) — record id as
        // current, seq as last, and accept the line.
        let state = match self
            .current_lines
            .entry(tamad.to_string())
            .or_default()
            .entry(source.to_string())
        {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(InstanceState {
                    current: (instance_id.to_string(), seq),
                    seen_olds: HashSet::new(),
                });
                return Decision::Fresh;
            }
            std::collections::hash_map::Entry::Occupied(slot) => slot.into_mut(),
        };

        // Rule 2: the instance is the current one — the watermark decides.
        if instance_id == state.current.0 {
            if seq > state.current.1 {
                state.current.1 = seq;
                return Decision::Fresh;
            }
            return Decision::Duplicate;
        }

        // Rule 3: the latest StreamInit announced this instance (and it is
        // not the current one) — a genuine new boot.
        let expected = self
            .expected
            .get(tamad)
            .and_then(|per_source| per_source.get(source));
        if expected.is_some_and(|e| e.as_str() == instance_id) {
            state.seen_olds.insert(state.current.0.clone());
            state.current = (instance_id.to_string(), seq);
            return Decision::NewInstance;
        }

        // Rule 5: a demoted old instance repeats itself.
        if state.seen_olds.contains(instance_id) {
            return Decision::Duplicate;
        }

        // Rule 4: an unannounced, never-seen instance — accept this single
        // line only (do NOT update `last`), and demote it so its later
        // lines become duplicates.
        state.seen_olds.insert(instance_id.to_string());
        Decision::OldInstanceReplay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 1: first contact for (tamad, source) → Fresh; later higher
    /// seq on the same instance → Fresh, lower/equal → Duplicate (rule 2).
    #[test]
    fn test_first_contact_and_same_instance_watermark() {
        let mut d = DedupState::new();
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 1),
            Decision::Fresh,
            "rule 1"
        );
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 2),
            Decision::Fresh,
            "rule 2: higher seq"
        );
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 2),
            Decision::Duplicate,
            "rule 2: equal seq is a replay"
        );
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 1),
            Decision::Duplicate,
            "rule 2: lower seq (late replay from the ring)"
        );
    }

    /// Rule 2: a line below the watermark seen out of order is a dup;
    /// the watermark tracks the highest accepted seq.
    #[test]
    fn test_watermark_out_of_order_arrivals() {
        let mut d = DedupState::new();
        assert_eq!(d.on_message("h1", "tamad", "a", 1), Decision::Fresh);
        assert_eq!(d.on_message("h1", "tamad", "a", 3), Decision::Fresh);
        assert_eq!(
            d.on_message("h1", "tamad", "a", 2),
            Decision::Duplicate,
            "2 < last(3)"
        );
        assert_eq!(d.on_message("h1", "tamad", "a", 4), Decision::Fresh);
    }

    /// Rule 3: the latest `StreamInit` announcement → NewInstance; the
    /// previous current id is demoted into seen_olds, so its late lines
    /// become Duplicates (rule 5).
    #[test]
    fn test_new_instance_announced() {
        let mut d = DedupState::new();
        d.on_init("h1", "tamad", "boot-a");
        assert_eq!(d.on_message("h1", "tamad", "boot-a", 5), Decision::Fresh);
        // Tamad reboots; the (re)connected stream re-announces init B
        // before any of its lines.
        d.on_init("h1", "tamad", "boot-b");
        assert_eq!(
            d.on_message("h1", "tamad", "boot-b", 1),
            Decision::NewInstance,
            "rule 3: expected != current"
        );
        assert_eq!(
            d.on_message("h1", "tamad", "boot-b", 2),
            Decision::Fresh,
            "B is now current; higher seq"
        );
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 6),
            Decision::Duplicate,
            "rule 5: A was demoted when B took over"
        );
    }

    /// An init that re-announces the already-current instance (the normal
    /// reconnect) is NOT a new instance — lines are judged by the
    /// watermark (rule 2).
    #[test]
    fn test_init_of_current_instance_is_not_new_instance() {
        let mut d = DedupState::new();
        d.on_init("h1", "tamad", "boot-a");
        assert_eq!(d.on_message("h1", "tamad", "boot-a", 1), Decision::Fresh);
        d.on_init("h1", "tamad", "boot-a");
        assert_eq!(d.on_message("h1", "tamad", "boot-a", 2), Decision::Fresh);
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 2),
            Decision::Duplicate
        );
    }

    /// A `StreamInit` that announced BEFORE first contact does not change
    /// the first-contact decision (rule 1).
    #[test]
    fn test_init_before_first_contact() {
        let mut d = DedupState::new();
        d.on_init("h1", "tamad", "boot-a");
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 1),
            Decision::Fresh,
            "rule 1 regardless of `expected`"
        );
        assert_eq!(d.on_message("h1", "tamad", "boot-a", 2), Decision::Fresh);
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 2),
            Decision::Duplicate
        );
    }

    /// Rule 4: a line from an unannounced, never-seen instance is
    /// accepted ONCE (OldInstanceReplay) WITHOUT touching the current
    /// watermark; the next line from that same id is a Duplicate.
    #[test]
    fn test_old_instance_late_replay_accepted_once() {
        let mut d = DedupState::new();
        d.on_init("h1", "tamad", "boot-b");
        assert_eq!(d.on_message("h1", "tamad", "boot-b", 1), Decision::Fresh);
        // A stale line of the previous boot slips in, unannounced.
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 99),
            Decision::OldInstanceReplay,
            "rule 4: first sighting of an old id"
        );
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 100),
            Decision::Duplicate,
            "rule 5: a second line of the demoted id"
        );
        // The demotion must not have touched B's watermark.
        assert_eq!(
            d.on_message("h1", "tamad", "boot-b", 1),
            Decision::Duplicate,
            "a B line up to the current watermark stays a dup"
        );
        assert_eq!(d.on_message("h1", "tamad", "boot-b", 2), Decision::Fresh);
    }

    /// Multiple reboots: every boot after the first is demoted in order;
    /// only the latest instance is current and only its lines beyond the
    /// watermark are fresh.
    #[test]
    fn test_multiple_reboots() {
        let mut d = DedupState::new();
        for (i, boot) in ["r1", "r2", "r3"].into_iter().enumerate() {
            d.on_init("h1", "model:m", boot);
            let first = d.on_message("h1", "model:m", boot, 1);
            assert_eq!(
                first,
                if i == 0 {
                    Decision::Fresh
                } else {
                    Decision::NewInstance
                },
                "boot {boot}"
            );
            assert_eq!(d.on_message("h1", "model:m", boot, 2), Decision::Fresh);
        }
        // None of the demoted boots contributes fresh lines anymore.
        assert_eq!(d.on_message("h1", "model:m", "r1", 3), Decision::Duplicate);
        assert_eq!(d.on_message("h1", "model:m", "r2", 3), Decision::Duplicate);
        assert_eq!(d.on_message("h1", "model:m", "r3", 3), Decision::Fresh);
    }

    /// Connection-lost continuation: NO state is reset on a reconnect —
    /// the re-announced init of the same boot followed by a full ring
    /// replay produces all-Duplicates, then genuinely new lines are
    /// fresh.
    #[test]
    fn test_reconnect_replay_after_connection_lost() {
        let mut d = DedupState::new();
        d.on_init("h1", "model:m", "boot-a");
        d.on_init("h1", "tamad", "boot-a");
        assert_eq!(d.on_message("h1", "model:m", "boot-a", 1), Decision::Fresh);
        assert_eq!(d.on_message("h1", "tamad", "boot-a", 1), Decision::Fresh);
        assert_eq!(d.on_message("h1", "tamad", "boot-a", 2), Decision::Fresh);

        // Stream dropped; the (re)connected stream re-announces boot-a
        // and replays the ring from its oldest surviving line.
        d.on_init("h1", "model:m", "boot-a");
        d.on_init("h1", "tamad", "boot-a");
        assert_eq!(
            d.on_message("h1", "model:m", "boot-a", 1),
            Decision::Duplicate,
            "surviving replay re-judged by the watermark"
        );
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 2),
            Decision::Duplicate,
            "replayed tamad line"
        );
        assert_eq!(
            d.on_message("h1", "tamad", "boot-a", 3),
            Decision::Fresh,
            "genuinely new line after the replay"
        );
    }

    /// State is per (tamad, source): another host or another source never
    /// shares a watermark.
    #[test]
    fn test_per_tamad_per_source_isolation() {
        let mut d = DedupState::new();
        assert_eq!(d.on_message("h1", "tamad", "a", 1), Decision::Fresh);
        assert_eq!(
            d.on_message("h2", "tamad", "a", 1),
            Decision::Fresh,
            "other host"
        );
        assert_eq!(
            d.on_message("h1", "model:m", "a", 1),
            Decision::Fresh,
            "same id on a different source"
        );
        // Advance h2 only; h1's watermark is untouched.
        assert_eq!(d.on_message("h2", "tamad", "a", 9), Decision::Fresh);
        assert_eq!(d.on_message("h1", "tamad", "a", 2), Decision::Fresh);
    }
}
