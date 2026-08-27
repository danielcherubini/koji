//! vLLM spec-decoding metrics scraping: Prometheus text parsing, windowed
//! diffing of cumulative counters, and endpoint→/metrics URL rewriting.
//!
//! Pure and dependency-free (`std` + `url`). The HTTP fetching and
//! per-tick budgeting live in [`crate::stats::StatsCollector`]; this module
//! owns the logic that decides what an observation means.
//!
//! The three cumulative counters vLLM exposes:
//!   `vllm:spec_decode_num_drafts_total`
//!   `vllm:spec_decode_num_draft_tokens_total`
//!   `vllm:spec_decode_num_accepted_tokens_total`
//!
//! each tagged with per-engine `model_name`/`engine` labels. With draft
//! tokens enabled (the default), the window acceptance rate is
//! `accepted_tokens ÷ draft_tokens`; with plain speculative tokens, the
//! 1/draft 1:1 relationship makes the same division read as accept/drafts
//! anyway.

use url::Url;

/// How often each endpoint is re-scraped (vLLM logs its spec summary every
/// 10s, so 10s gives one observation per backend log line).
pub const SCRAPE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
/// An observation older than one minute reads as inactive — renames,
/// restarts, or a wedged scrape silence the indicator instead of showing a
/// stale rate forever.
pub const STALE_MS: i64 = 60_000;
/// Per-endpoint scrape timeout; a wedged /metrics must not stall the tick.
pub const PER_SCRAPE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// Cumulative scrape budget per tick: the tick must never linger, or the
/// proxy's 5s `LIVE_FRAME_MAX_AGE` freshness gate blanks every model on the
/// host. Skipped models simply retry next tick.
pub const TICK_SCRAPE_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

/// Cumulative spec-decoding counters summed across all label sets.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpecCounters {
    /// `vllm:spec_decode_num_drafts_total`
    pub drafts: f64,
    /// `vllm:spec_decode_num_draft_tokens_total`
    pub draft_tokens: f64,
    /// `vllm:spec_decode_num_accepted_tokens_total`
    pub accepted_tokens: f64,
}

/// Parse the spec-decoding counters out of a Prometheus text exposition.
///
/// Skips blank lines, `#` comment lines (HELP/TYPE/EOF), and any line
/// whose value token is not a finite `f64` — while keeping the rest of
/// the body. Label values containing spaces are parsed (the name is
/// recovered as the prefix before the first `{`), and a trailing
/// Prometheus timestamp is ignored whenever a finite value precedes
/// it. Sums the value across
/// ALL label sets for each exact counter name — names are matched
/// exactly, so a metric with a longer suffix (e.g. `..._drafts_total_extra`)
/// is never counted. Returns `None` only when none of the three counter
/// names is present at all.
pub fn parse_spec_metrics(body: &str) -> Option<SpecCounters> {
    let mut seen = [false; 3];
    let mut c = SpecCounters::default();
    for line in body.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = split_metric_line(line) else {
            continue;
        };
        let idx = match name.as_str() {
            "vllm:spec_decode_num_drafts_total" => 0,
            "vllm:spec_decode_num_draft_tokens_total" => 1,
            "vllm:spec_decode_num_accepted_tokens_total" => 2,
            _ => continue,
        };
        seen[idx] = true;
        match idx {
            0 => c.drafts += value,
            1 => c.draft_tokens += value,
            _ => c.accepted_tokens += value,
        }
    }
    if seen.iter().all(|s| !*s) {
        None
    } else {
        Some(c)
    }
}

/// Split a `name{...maybe labels...} <value>` line into its exact metric
/// name and value.
///
/// Guarantees, over the whitespace-token sequence `t[0..n]` (`n >= 2`,
/// else `None`):
///
/// - **Spaced label values are accepted.** A quoted label value containing
///   a space (`name{model_name="foo bar"} 5`) splits the line into extra
///   tokens, but the name is the single-space rejoin of the tokens
///   before the value token, truncated at the FIRST `{` — so the exact,
///   space-free counter name always lives entirely in `t[0]` and
///   rejoining can never mix label text into it.
/// - **Trailing timestamps are ignored, never read as the value.** When
///   the last token is a plain integer and the second-to-last parses to
///   a finite `f64`, the second-to-last is the value (the last token is
///   treated as an ignorable trailing Prometheus timestamp).
/// - **Non-finite values are rejected.** Any value token that parses to
///   NaN / ±Inf disqualifies the line, since it would otherwise flow
///   into an observation and surface as a "NaN%" card.
///
/// Degenerate case (legal Prometheus has at most one token after the
/// value): on a 4-token line whose last token is a plain integer and
/// second-to-last a finite value, the second-to-last token is accepted
/// as the value — `name 5.0 1700000000000 22` yields `1700000000000`.
/// Real expositions never produce this; the exact-name match in the
/// caller bounds the blast radius of such malformed input.
///
/// Returns `None` for any line that does not yield a finite value.
fn split_metric_line(line: &str) -> Option<(String, f64)> {
    let tokens: Vec<&str> = line.split_ascii_whitespace().collect();
    let n = tokens.len();
    if n < 2 {
        return None;
    }
    // Pick the value token: a plain-integer last token after a finite
    // value is a trailing timestamp — use the token before it instead.
    // Otherwise the value is the last token.
    let value_idx = if n >= 3
        && is_plain_int(tokens[n - 1])
        && tokens[n - 2]
            .parse::<f64>()
            .ok()
            .is_some_and(|v: f64| v.is_finite())
    {
        n - 2
    } else {
        n - 1
    };
    let value: f64 = tokens[value_idx].parse().ok()?;
    // NaN / ±Inf must not reach an observation — reject the line.
    if !value.is_finite() {
        return None;
    }
    // The tokens before the value form `metric{...labels...}` (a label
    // value may contain spaces, so rejoin with single spaces), then
    // truncate at the first `{` where the label set begins.
    let rejoined = tokens[..value_idx].join(" ");
    let name = match rejoined.find('{') {
        Some(i) => rejoined[..i].to_string(),
        None => rejoined,
    };
    Some((name, value))
}

/// Whether `tok` is a plain integer (`-?[0-9]+`) — the shape of a
/// trailing Prometheus timestamp.
fn is_plain_int(tok: &str) -> bool {
    let digits = tok.strip_prefix('-').unwrap_or(tok);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Diff two successive cumulative counter sets for a windowed acceptance
/// rate.
///
/// - `prev` is `None` (first scrape) → `None` (no delta to compute)
/// - any counter in `cur` lower than `prev` → `None` (engine restart /
///   counter reset — never emit a bogus rate)
/// - `Δdraft_tokens > 0` → `Some((100.0 * Δaccepted / Δdraft_tokens, true))`
/// - otherwise → `None` (no spec traffic in this window; the caller keeps
///   the last observation until it goes stale)
pub fn observe(prev: Option<SpecCounters>, cur: SpecCounters) -> Option<(f64, bool)> {
    let prev = prev?;
    if cur.drafts < prev.drafts
        || cur.draft_tokens < prev.draft_tokens
        || cur.accepted_tokens < prev.accepted_tokens
    {
        return None;
    }
    let d_draft_tokens = cur.draft_tokens - prev.draft_tokens;
    if d_draft_tokens <= 0.0 {
        return None;
    }
    let pct = 100.0 * (cur.accepted_tokens - prev.accepted_tokens) / d_draft_tokens;
    Some((pct, true))
}

/// Rewrite an engine endpoint URL to its `/metrics` path:
/// `"http://127.0.0.1:8000/v1"` → `Some("http://127.0.0.1:8000/metrics")`.
/// A bare `http://host:9000` (no path) works as-is; `https` is preserved;
/// any non-http(s) scheme (e.g. `grpc://`) → `None`.
pub fn metrics_url_for(endpoint_url: &str) -> Option<String> {
    let parsed = Url::parse(endpoint_url).ok()?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return None,
    }
    let mut u = parsed;
    u.set_path("/metrics");
    u.set_query(None);
    u.set_fragment(None);
    Some(u.to_string())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic vLLM /metrics body: comments, unrelated metrics, labels
    /// with `=` and spaces in values, and the three spec counters each in
    /// two label sets. Values must sum across label sets.
    const VLLM_BODY: &str = r#"# HELP vllm:prompt_tokens_total Total number of prompt tokens processed
# TYPE vllm:prompt_tokens_total counter
vllm:prompt_tokens_total{model_name="Qwen3-30B-A3B",engine="0"} 12345.0
vllm:num_requests_running{model_name="Qwen3-30B-A3B",engine="0"} 1.0
# HELP vllm:spec_decode_num_drafts_total Total number of spec decoding iterations
# TYPE vllm:spec_decode_num_drafts_total counter
vllm:spec_decode_num_drafts_total{model_name="Qwen3-30B-A3B",engine="0"} 57.0
vllm:spec_decode_num_drafts_total{model_name="Qwen3-30B-A3B",engine="1"} 58.0
vllm:spec_decode_num_draft_tokens_total{model_name="Qwen3-30B-A3B",engine="0"} 185.5
vllm:spec_decode_num_draft_tokens_total{model_name="Qwen3-30B-A3B",engine="1"} 186.0
vllm:spec_decode_num_accepted_tokens_total{model_name="Qwen3-30B-A3B",engine="0"} 82.0
vllm:spec_decode_num_accepted_tokens_total{model_name="Qwen3-30B-A3B",engine="1"} 84.0
vllm:kv_cache_usage_perc{model_name="Qwen3-30B-A3B",engine="0"} 0.42
http_requests_total{method="GET",uri="/v1/chat/completions",ok="true"} 99.0

# EOF
"#;

    #[test]
    fn test_parse_vllm_two_label_sets_sum() {
        let c = parse_spec_metrics(VLLM_BODY).expect("three counters present");
        assert_eq!(
            c,
            SpecCounters {
                drafts: 115.0,
                draft_tokens: 371.5,
                accepted_tokens: 166.0,
            }
        );
    }

    /// A body without any of the three names → `None`, even with a
    /// tokenizing label.
    #[test]
    fn test_parse_missing_counters_none() {
        let body = "\
vllm:prompt_tokens_total{model_name=\"x\",engine=\"0\"} 1.0\n\
llamacpp_duration_s{status_stage=\"0\",lifespan_stage=\"0\",vram_stage=\"3\"} 2.5\n\
process_cpu_seconds_total 42.0\n";
        assert_eq!(parse_spec_metrics(body), None);
    }

    /// The rest of the body still parses when a line's trailing value token
    /// is unparseable — that one line is skipped, the others are kept.
    #[test]
    fn test_parse_unparseable_value_line_skipped() {
        let body = "\
vllm:spec_decode_num_drafts_total{model_name=\"x\",engine=\"0\"} 10.0\nbroken_metric{a=\"b\"} not_a_number\nvllm:spec_decode_num_draft_tokens_total{model_name=\"x\",engine=\"0\"} 30.0\nvllm:spec_decode_num_accepted_tokens_total{model_name=\"x\",engine=\"0\"} 15.0\n";
        let c = parse_spec_metrics(body).expect("two valid lines remain");
        assert_eq!(
            c,
            SpecCounters {
                drafts: 10.0,
                draft_tokens: 30.0,
                accepted_tokens: 15.0,
            }
        );
    }

    /// Exact-name guard: a longer-suffixed counter must NOT match.
    #[test]
    fn test_parse_exact_name_guard() {
        let body = "vllm:spec_decode_num_drafts_total_extra{model_name=\"x\",engine=\"0\"} 7.0\n";
        assert_eq!(parse_spec_metrics(body), None);
    }

    /// A label set whose quoted value contains a space is a legal
    /// Prometheus form (plan Task 2) and MUST be parsed: the name is the
    /// prefix before the first `{`, the value is the last finite token.
    /// Values are summed across all label sets for the counter.
    #[test]
    fn test_parse_labels_with_spaces() {
        let body = "\
            vllm:spec_decode_num_drafts_total{model_name=\"Instruct Model\",engine=\"0\"} 3.0\n\
            vllm:spec_decode_num_drafts_total{model_name=\"Instruct Model\",engine=\"1\"} 4.0\n\
            vllm:spec_decode_num_draft_tokens_total{model_name=\"a b\",engine=\"0\"} 100\n\
            vllm:spec_decode_num_accepted_tokens_total{model_name=\"a b\",engine=\"0\"} 60\n";
        let c = parse_spec_metrics(body).expect("spaced label lines parse");
        assert_eq!(
            c,
            SpecCounters {
                drafts: 7.0,
                draft_tokens: 100.0,
                accepted_tokens: 60.0,
            }
        );
    }

    /// A trailing Prometheus timestamp must never be read as the counter
    /// value. When a finite value precedes a trailing plain-integer token,
    /// the value is used and the timestamp ignored — including when the
    /// label set itself carries a spaced quoted value.
    #[test]
    fn test_parse_trailing_timestamp_ignored() {
        // No label set: timestamp ignored, real value used.
        let c = parse_spec_metrics("vllm:spec_decode_num_drafts_total 57.0 1700000000000")
            .expect("value precedes the timestamp");
        assert_eq!(c.drafts, 57.0, "timestamp must not be the value");

        // Labeled form (realistic vLLM output): same rule.
        let c = parse_spec_metrics(
            r#"vllm:spec_decode_num_drafts_total{model_name="x",engine="0"} 5.0 1700000000000"#,
        )
        .expect("labeled line parses");
        assert_eq!(c.drafts, 5.0, "timestamp must not be the value");

        // Spaced label value plus timestamp: value used, timestamp ignored.
        let c = parse_spec_metrics(
            r#"vllm:spec_decode_num_drafts_total{model_name="a b",engine="0"} 5.0 1700000000000"#,
        )
        .expect("spaced label + timestamp parses");
        assert_eq!(c.drafts, 5.0, "timestamp must not be summed into the value");

        // Mixed: each line counts at its genuine value.
        let mixed = r#"vllm:spec_decode_num_drafts_total{model_name="x",engine="0"} 4.0
vllm:spec_decode_num_drafts_total{model_name="x",engine="1"} 100.0 1700000000000"#;
        let c = parse_spec_metrics(mixed).expect("both lines count at their values");
        assert_eq!(c.drafts, 104.0, "timestamp must not be summed in");
    }

    /// A non-finite value token (NaN / ±Inf) is rejected: a `NaN`
    /// counter must not flow into an observation (it would surface as a
    /// "NaN%" card downstream).
    #[test]
    fn test_parse_non_finite_value_skipped() {
        let nan = r#"vllm:spec_decode_num_drafts_total{model_name="x",engine="0"} NaN"#;
        assert_eq!(parse_spec_metrics(nan), None);
        let inf = r#"vllm:spec_decode_num_drafts_total{model_name="x",engine="0"} inf"#;
        assert_eq!(parse_spec_metrics(inf), None);

        // Mixed: the NaN line is dropped, the other counters survive.
        let mixed = r#"vllm:spec_decode_num_drafts_total{model_name="x",engine="0"} 4.0
vllm:spec_decode_num_drafts_total{model_name="x",engine="1"} NaN
vllm:spec_decode_num_draft_tokens_total{model_name="x",engine="0"} 8.0
vllm:spec_decode_num_accepted_tokens_total{model_name="x",engine="0"} 2.0"#;
        let c = parse_spec_metrics(mixed).expect("remaining counters survive");
        assert_eq!(c.drafts, 4.0);
        assert!(c.drafts.is_finite());
    }

    #[test]
    fn test_observe_first_scrape_none() {
        let cur = SpecCounters {
            drafts: 115.0,
            draft_tokens: 371.0,
            accepted_tokens: 165.0,
        };
        assert_eq!(observe(None, cur), None);
    }

    /// Real-log vector: window counters "Accepted: 165, Drafted: 371" ⇒
    /// "Avg Draft acceptance rate: 44.5%".
    #[test]
    fn test_observe_real_log_window() {
        let prev = SpecCounters::default();
        let cur = SpecCounters {
            drafts: 115.0,
            draft_tokens: 371.0,
            accepted_tokens: 165.0,
        };
        let Some((pct, active)) = observe(Some(prev), cur) else {
            panic!("expected a rate");
        };
        assert!(active);
        assert!((pct - 44.47).abs() < 0.01, "got {pct}");
    }

    /// Intermediate window: (200-133)/(450-300) = 67/150 ≈ 44.667.
    #[test]
    fn test_observe_intermediate_window() {
        let prev = SpecCounters {
            drafts: 100.0,
            draft_tokens: 300.0,
            accepted_tokens: 133.0,
        };
        let cur = SpecCounters {
            drafts: 140.0,
            draft_tokens: 450.0,
            accepted_tokens: 200.0,
        };
        let Some((pct, _)) = observe(Some(prev), cur) else {
            panic!("expected a rate");
        };
        assert!((pct - 44.6666667).abs() < 0.001, "got {pct}");
    }

    /// Any counter lower than prev → engine restart / reset → `None`.
    #[test]
    fn test_observe_counter_reset_none() {
        let base_prev = SpecCounters {
            drafts: 100.0,
            draft_tokens: 300.0,
            accepted_tokens: 133.0,
        };
        let reset_draft = SpecCounters {
            drafts: 90.0,
            draft_tokens: 350.0,
            accepted_tokens: 140.0,
        };
        let reset_tokens = SpecCounters {
            drafts: 110.0,
            draft_tokens: 250.0,
            accepted_tokens: 140.0,
        };
        let reset_accepted = SpecCounters {
            drafts: 110.0,
            draft_tokens: 350.0,
            accepted_tokens: 100.0,
        };
        assert_eq!(observe(Some(base_prev), reset_draft), None);
        assert_eq!(observe(Some(base_prev), reset_tokens), None);
        assert_eq!(observe(Some(base_prev), reset_accepted), None);
    }

    /// Δdraft_tokens == 0 → no spec traffic in the window → `None`.
    #[test]
    fn test_observe_no_traffic_none() {
        let c = SpecCounters {
            drafts: 50.0,
            draft_tokens: 150.0,
            accepted_tokens: 70.0,
        };
        assert_eq!(observe(Some(c), c), None);
    }

    #[test]
    fn test_metrics_url_with_path() {
        assert_eq!(
            metrics_url_for("http://127.0.0.1:8000/v1"),
            Some("http://127.0.0.1:8000/metrics".to_string())
        );
    }

    #[test]
    fn test_metrics_url_bare_host() {
        assert_eq!(
            metrics_url_for("http://host:9000"),
            Some("http://host:9000/metrics".to_string())
        );
    }

    #[test]
    fn test_metrics_url_https_preserved() {
        assert_eq!(
            metrics_url_for("https://inhost:8000/v1"),
            Some("https://inhost:8000/metrics".to_string())
        );
    }

    #[test]
    fn test_metrics_url_non_http_scheme() {
        assert_eq!(metrics_url_for("grpc://x"), None);
    }
}
