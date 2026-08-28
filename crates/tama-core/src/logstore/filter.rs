//! Shared runtime log-filter builder (plan-195 task 2).
//!
//! Home of `build_log_filter`, moved (behavior-preserving) out of
//! `crates/tama/src/main.rs` so proxy startup, the `tama admin` CLI and (in
//! plan-195 task 6) tamad startup share ONE merge point for the durable
//! `log_level`, the new durable `log_directives` config field and the
//! `RUST_LOG` environment variable.
//!
//! ## Merge semantics (the "target-only" rule — behavior of the old
//! `main.rs` body, preserved byte-for-byte for `RUST_LOG`)
//!
//! 1. The config `log_level` is the sole floor/default of the filter:
//!    a bare level entry (no target before `=` — e.g. `RUST_LOG="info"`)
//!    is **not** a directive and is skipped, so it can't override the
//!    configured floor.
//! 2. `RUST_LOG` entries that DO contain `=` (target-specific
//!    directives) are added in order. The function reads the env var
//!    itself — callers never pass env values.
//! 3. The config `log_directives` (the `directives` argument; every
//!    current caller passes `""` — the durable field arrives in task 3)
//!    are merged with the same target-only rule, added AFTER the env
//!    directives.
//!
//! Same-target precedence is last-addition-wins: `EnvFilter::add_directive`
//! keeps the last added directive matching a target (the building block is
//! pinned in `test_envfilter_add_directive_replaces_last_matching_target`
//! in this tracing-subscriber version, 0.3.23) — so config directives win
//! over env directives for the same target.
//!
//! Every directive-looking entry (contains `=`) is validated: a
//! directive-looking string that fails to parse is an [`LogFilterError`]
//! (the pre-move `main.rs` silently skipped these — the explicit error is
//! the small behavior change called out in the plan).

use crate::config::LogLevel;
use tracing::Subscriber;
use tracing_subscriber::filter::Directive;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::reload;
use tracing_subscriber::EnvFilter;

/// The env var carrying extra target directives. Read internally by
/// [`build_log_filter`] and [`apply_reload`]; callers never pass its
/// value.
const RUST_LOG_ENV: &str = "RUST_LOG";

/// A directive-looking (contains `=`) log directive failed to parse, or
/// a filter reload against a live subscriber failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFilterError(String);

impl LogFilterError {
    fn invalid(directive: &str, reason: &str) -> Self {
        Self(format!("invalid log directive {directive:?}: {reason}"))
    }

    fn reload(message: &str) -> Self {
        Self(format!("log filter reload failed: {message}"))
    }
}

impl std::fmt::Display for LogFilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LogFilterError {}

/// Splits a comma-separated directive string, applying the target-only
/// rule and validating every directive-looking entry.
fn parse_target_directives(raw: &str) -> Result<Vec<Directive>, LogFilterError> {
    let mut directives = Vec::new();
    for item in raw.split(',') {
        let item = item.trim();
        // Only entries with a target (contain '=') are directives. Bare
        // levels like "warn" or "info" are left out — bare levels would
        // set the default and override the configured floor — we want
        // the floor to stay authoritative.
        if item.is_empty() || !item.contains('=') {
            continue;
        }
        match item.parse::<Directive>() {
            Ok(directive) => directives.push(directive),
            Err(e) => return Err(LogFilterError::invalid(item, &e.to_string())),
        }
    }
    Ok(directives)
}

/// Count of directive-looking entries in `raw` (same target-only rule).
fn target_directive_count(raw: &str) -> usize {
    raw.split(',')
        .filter(|item| {
            let item = item.trim();
            !item.is_empty() && item.contains('=')
        })
        .count()
}

/// Builds the runtime [`EnvFilter`] from the DB/durable config: the
/// `level` is the floor/default; `directives` (RUST_LOG-syntax string)
/// adds target directives; `RUST_LOG` env vars are merged with the same
/// target-only rule — env directives first, config `directives` after, so
/// config wins for the same target (last-addition-wins).
///
/// The function reads the `RUST_LOG` env var itself; callers never pass
/// it.
pub fn build_log_filter(level: &LogLevel, directives: &str) -> Result<EnvFilter, LogFilterError> {
    // The configured config level is the base floor/default.
    let mut filter = tracing_subscriber::EnvFilter::new(level.as_str());

    // Merge around the inner `=` of RUST_LOG entries as target directives
    // (real lines: user sets RUST_LOG="tama_core=debug,tama=info" or
    // RUST_LOG=my_crate=trace).
    if let Ok(rust_log) = std::env::var(RUST_LOG_ENV) {
        for directive in parse_target_directives(&rust_log)? {
            filter = filter.add_directive(directive);
        }
    }

    // Config-supplied directives are merged last so they win over env
    // directives for the same target (tracing-subscriber keeps the last
    // addition per target).
    for directive in parse_target_directives(directives)? {
        filter = filter.add_directive(directive);
    }

    Ok(filter)
}

/// Swaps the loaded filter of a live subscriber through a
/// `reload::Handle`. Returns the number of directives in the replacement
/// filter (one base level directive plus every merged env + config
/// directive).
pub fn apply_reload<S>(
    handle: &reload::Handle<EnvFilter, S>,
    level: &LogLevel,
    directives: &str,
) -> Result<usize, LogFilterError>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let new_filter = build_log_filter(level, directives)?;
    handle
        .reload(new_filter)
        .map_err(|_| LogFilterError::reload("reloading the active subscriber failed"))?;

    Ok(1 + target_directive_count(directives) + env_directive_count())
}

/// Count of directive-looking entries from the current `RUST_LOG` env var
/// (same target-only rule as [`parse_target_directives`]).
fn env_directive_count() -> usize {
    std::env::var(RUST_LOG_ENV)
        .map(|v| target_directive_count(&v))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LogLevel;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
    use tracing_subscriber::Registry;

    /// Serializes tests that touch RUST_LOG (libtest runs tests in
    /// parallel threads in one process; nextest isolates them in
    /// processes, but the mutex keeps both runners correct).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn set_rust_log(value: Option<&str>) {
        match value {
            Some(v) => std::env::set_var("RUST_LOG", v),
            None => std::env::remove_var("RUST_LOG"),
        }
    }

    /// Flags every event reaching it (installed as a registry layer
    /// alongside the filter under test) and counts arrivals.
    struct Flag {
        count: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl<S> Layer<S> for Flag
    where
        S: Subscriber,
    {
        fn on_event(&self, _event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Whether the probe event fired by `probe_body` reaches the flag
    /// layer when dispatched through a real subscriber (registry + flag
    /// layer + filter under test). The probe body is a small function
    /// wrapping a real `tracing!` macro call (macro targets are const,
    /// so one fn per target/level pair).
    fn probe(filter: &EnvFilter, probe_body: fn()) -> bool {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let flag = Flag {
            count: count.clone(),
        };
        let subscriber = Registry::default().with(flag).with(filter.clone());
        tracing::subscriber::with_default(subscriber, probe_body);
        count.load(Ordering::SeqCst) == 1
    }

    // ── Probe events (one per target+level pair) ─────────────

    fn ev_probe_target_debug() {
        tracing::debug!(target: "probe_target", "probe");
    }
    fn ev_probe_target_info() {
        tracing::info!(target: "probe_target", "probe");
    }
    fn ev_probe_target_warn() {
        tracing::warn!(target: "probe_target", "probe");
    }
    fn ev_probe_target_error() {
        tracing::error!(target: "probe_target", "probe");
    }
    fn ev_other_target_debug() {
        tracing::debug!(target: "other_target", "probe");
    }
    fn ev_probe_a_debug() {
        tracing::debug!(target: "probe_a", "probe");
    }
    fn ev_probe_b_debug() {
        tracing::debug!(target: "probe_b", "probe");
    }
    fn ev_probe_c_debug() {
        tracing::debug!(target: "probe_c", "probe");
    }
    fn ev_probe_c_info() {
        tracing::info!(target: "probe_c", "probe");
    }
    fn ev_probe_c_error() {
        tracing::error!(target: "probe_c", "probe");
    }
    fn ev_probe_d_error() {
        tracing::error!(target: "probe_d", "probe");
    }
    fn ev_a_error() {
        tracing::error!(target: "a", "probe");
    }
    fn ev_a_warn() {
        tracing::warn!(target: "a", "probe");
    }

    const CS_DEBUG_PROBE_TARGET: fn() = ev_probe_target_debug;
    const CS_INFO_PROBE_TARGET: fn() = ev_probe_target_info;
    const CS_WARN_PROBE_TARGET: fn() = ev_probe_target_warn;
    const CS_ERROR_PROBE_TARGET: fn() = ev_probe_target_error;
    const CS_DEBUG_OTHER_TARGET: fn() = ev_other_target_debug;
    const CS_DEBUG_PROBE_A: fn() = ev_probe_a_debug;
    const CS_DEBUG_PROBE_B: fn() = ev_probe_b_debug;
    const CS_DEBUG_PROBE_C: fn() = ev_probe_c_debug;
    const CS_INFO_PROBE_C: fn() = ev_probe_c_info;
    const CS_ERROR_PROBE_C: fn() = ev_probe_c_error;
    const CS_ERROR_PROBE_D: fn() = ev_probe_d_error;
    const CS_ERROR_A: fn() = ev_a_error;
    const CS_WARN_A: fn() = ev_a_warn;

    /// The config `log_level` is the authoritative floor of the filter.
    #[test]
    fn test_config_level_is_floor_default() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_rust_log(None);

        let filter = build_log_filter(&LogLevel::Info, "").expect("valid filter");
        assert!(
            !probe(&filter, CS_DEBUG_PROBE_TARGET),
            "info floor must gate DEBUG"
        );
        assert!(
            probe(&filter, CS_INFO_PROBE_TARGET),
            "info floor must enable INFO"
        );

        let filter = build_log_filter(&LogLevel::Error, "").expect("valid filter");
        assert!(
            !probe(&filter, CS_WARN_PROBE_TARGET),
            "error floor must gate WARN"
        );
        assert!(
            probe(&filter, CS_ERROR_PROBE_TARGET),
            "error floor must enable ERROR"
        );
    }

    /// RUST_LOG target directives merge on top of the floor, scoped to
    /// their targets (preserving the pre-move `main.rs` behavior).
    #[test]
    fn test_rust_log_target_directive_merges_over_floor() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_rust_log(Some("probe_target=debug"));

        let filter = build_log_filter(&LogLevel::Warn, "").expect("valid filter");
        assert!(
            probe(&filter, CS_DEBUG_PROBE_TARGET),
            "directive must open DEBUG for that target"
        );
        assert!(
            !probe(&filter, CS_DEBUG_OTHER_TARGET),
            "directive is target-specific; other targets keep the floor"
        );
    }

    /// Multiple RUST_LOG directives merge individually (trim + split).
    #[test]
    fn test_rust_log_multiple_target_directives() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_rust_log(Some("probe_a=debug, probe_b=debug"));

        let filter = build_log_filter(&LogLevel::Warn, "").expect("valid filter");
        assert!(probe(&filter, CS_DEBUG_PROBE_A), "first directive");
        assert!(probe(&filter, CS_DEBUG_PROBE_B), "second directive");
        assert!(!probe(&filter, CS_DEBUG_PROBE_C), "no third target");
    }

    /// Bare-level RUST_LOG entries are NOT directives (pre-move rule) —
    /// they can't raise the floor.
    #[test]
    fn test_bare_rust_log_entry_is_not_a_directive() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_rust_log(Some("info"));

        let filter = build_log_filter(&LogLevel::Error, "").expect("valid filter");
        assert!(
            !probe(&filter, CS_INFO_PROBE_TARGET),
            "bare 'info' must not raise an error floor"
        );
        assert!(
            probe(&filter, CS_ERROR_PROBE_TARGET),
            "the error floor still enables ERROR"
        );
    }

    /// Directive-looking strings that fail to parse are an error (both
    /// sources); non-directive-looking strings are skipped, per rule.
    #[test]
    fn test_invalid_directive_is_err() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_rust_log(None);

        assert!(
            build_log_filter(&LogLevel::Info, "").is_ok(),
            "empty directives"
        );
        assert!(
            build_log_filter(&LogLevel::Info, "not a directive at all").is_ok(),
            "no '=' means 'not a directive', skipped"
        );
        assert!(
            build_log_filter(&LogLevel::Info, "probe_target=not-a-level:-").is_err(),
            "directive-looking string that fails to parse must be an error"
        );

        set_rust_log(Some("probe_target=not-a-level:-"));
        assert!(
            build_log_filter(&LogLevel::Info, "").is_err(),
            "invalid env directive must be an error, not skipped"
        );
        set_rust_log(None);
    }

    /// The building block the config-over-env precedence rests on: adding
    /// `a=warn` and then `a=error` leaves target `a` enabled at ERROR
    /// but not WARN — the LAST added directive matching a target
    /// replaces the earlier one (the behavior the merge chain relies on
    /// in this tracing-subscriber version; the plan's fallback is the
    /// joined-directive-string construction, if this ever regresses).
    #[test]
    fn test_envfilter_add_directive_replaces_last_matching_target() {
        let mut filter = EnvFilter::new("info");
        filter = filter.add_directive("a=warn".parse::<Directive>().expect("parse a=warn"));
        filter = filter.add_directive("a=error".parse::<Directive>().expect("parse a=error"));

        assert!(
            probe(&filter, CS_ERROR_A),
            "last added directive (error) is in force"
        );
        assert!(
            !probe(&filter, CS_WARN_A),
            "a=error replaced a=warn for target a"
        );
    }

    /// Config directives win over env directives for the SAME target
    /// (env added first, config after; last-match-wins), and the merge
    /// is comma-split + trimmed for the config source too.
    #[test]
    fn test_config_directive_wins_over_env_for_same_target() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_rust_log(Some("probe_target=debug"));

        let filter = build_log_filter(&LogLevel::Info, "probe_target=error").expect("valid filter");
        assert!(
            !probe(&filter, CS_DEBUG_PROBE_TARGET),
            "config directive (error) must replace the env directive (debug)"
        );
        assert!(
            !probe(&filter, CS_WARN_PROBE_TARGET),
            "an explicit target directive is authoritative for that target (floor not consulted)"
        );
        assert!(
            probe(&filter, CS_ERROR_PROBE_TARGET),
            "the config error directive is in effect"
        );

        // Multi-directive config source, split and trimmed.
        let filter = build_log_filter(&LogLevel::Info, " probe_c=error, probe_d=error ")
            .expect("valid filter");
        assert!(probe(&filter, CS_ERROR_PROBE_C), "first config directive");
        assert!(probe(&filter, CS_ERROR_PROBE_D), "second config directive");
        assert!(
            !probe(&filter, CS_DEBUG_PROBE_C),
            "no debug opened for probe_c"
        );
        assert!(
            !probe(&filter, CS_INFO_PROBE_C),
            "the target directive is authoritative for probe_c (floor not consulted)"
        );
    }

    /// `apply_reload` swaps the live filter and reports the directive
    /// count of the replacement (base level + env + config directives).
    #[test]
    fn test_apply_reload_swaps_filter_and_reports_directive_count() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_rust_log(None);

        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let flag = Flag {
            count: count.clone(),
        };
        let (filter, handle) =
            reload::Layer::new(build_log_filter(&LogLevel::Info, "").expect("initial filter"));
        // The reload layer wraps the filter; dispatching happens against the
        // same subscriber so a live swap is observable via on_event counts.
        let subscriber = Registry::default().with(flag).with(filter);
        tracing::subscriber::with_default(subscriber, || {
            // Gated by the info floor: nothing reaches the flag layer.
            tracing::debug!(target: "probe_target", "post-reload probe");
            let n = apply_reload(&handle, &LogLevel::Debug, "probe_target=trace")
                .expect("apply_reload against a live subscriber");
            assert_eq!(n, 2, "base level directive + 1 config directive");
            // Enabled now: the reloaded debug floor lets the probe through.
            tracing::debug!(target: "probe_target", "post-reload probe");
        });
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "only the post-reload debug probe fires through the live filter"
        );
    }

    /// `apply_reload` also counts env-derived directives.
    #[test]
    fn test_apply_reload_counts_env_directives() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_rust_log(Some("env_directive_target=debug"));

        let (filter, handle) =
            reload::Layer::new(build_log_filter(&LogLevel::Info, "").expect("initial filter"));
        let subscriber = Registry::default().with(filter);
        tracing::subscriber::with_default(subscriber, || {
            let n = apply_reload(&handle, &LogLevel::Warn, "").expect("apply_reload");
            assert_eq!(n, 2, "base level directive + 1 env directive");
        });
    }

    /// `apply_reload` rejects invalid directives, and the live filter
    /// keeps serving its earlier state (no partial swap possible: the
    /// new filter is built before the handle is touched).
    #[test]
    fn test_apply_reload_rejects_invalid_and_keeps_current() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_rust_log(None);

        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let flag = Flag {
            count: count.clone(),
        };
        let (filter, handle) =
            reload::Layer::new(build_log_filter(&LogLevel::Error, "").expect("initial filter"));
        let subscriber = Registry::default().with(flag).with(filter);
        tracing::subscriber::with_default(subscriber, || {
            apply_reload(&handle, &LogLevel::Debug, "oops=!!>")
                .expect_err("invalid directive must be rejected");
            // Still gated by the (unchanged) error floor — never dispatched
            // (if it were, the flag layer would have counted it).
            tracing::warn!(target: "probe_target", "post-failed-reload probe");
        });
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "the failed reload must leave the error floor in force"
        );
    }
}
