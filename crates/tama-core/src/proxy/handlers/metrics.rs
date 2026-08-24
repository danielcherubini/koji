//! Prometheus metrics formatting helpers.
//!
//! Converts Tama's internal proxy metrics, system metrics, and backend
//! (llama.cpp) metrics into Prometheus exposition format for Grafana ingestion.

use crate::gpu::SystemMetrics;
use crate::proxy::types::ProxyMetrics;
use std::sync::atomic::Ordering::Relaxed;

/// Format Tama's proxy metrics as Prometheus exposition format.
///
/// Returns a string with `# HELP`, `# TYPE`, and value lines for each metric.
/// All metrics are typed as `gauge`.
pub fn format_tama_metrics(metrics: &ProxyMetrics, active_models: usize) -> String {
    let mut out = String::new();

    let total = metrics.total_requests.load(Relaxed);
    let successful = metrics.successful_requests.load(Relaxed);
    let failed = metrics.failed_requests.load(Relaxed);
    // plan-193 T4/T5c: `models_loaded` is the live row ready count (the
    // in-memory AtomicU64 counter is gone), passed in as `active_models`.
    let loaded = active_models as u64;

    push_gauge(
        &mut out,
        "tama:total_requests",
        "Total number of requests proxied.",
        total,
    );
    push_gauge(
        &mut out,
        "tama:successful_requests",
        "Number of successful (2xx) requests.",
        successful,
    );
    push_gauge(
        &mut out,
        "tama:failed_requests",
        "Number of failed (non-2xx) requests.",
        failed,
    );
    push_gauge(
        &mut out,
        "tama:models_loaded",
        "Current number of loaded (ready) models (rows.ready_count).",
        loaded,
    );
    push_gauge(
        &mut out,
        "tama:models_unloaded",
        "Unloaded-model counter (kept for wire compatibility; no counter remains).",
        0,
    );
    push_gauge(
        &mut out,
        "tama:active_models",
        "Current number of active (ready) models.",
        active_models as u64,
    );

    out
}

/// Format Tama's system metrics (CPU, RAM, GPU, VRAM) as Prometheus exposition format.
pub fn format_system_metrics(sys: &SystemMetrics) -> String {
    let mut out = String::new();

    push_gauge_f32(
        &mut out,
        "tama:cpu_usage_pct",
        "CPU utilization percentage (0.0-100.0).",
        sys.cpu_usage_pct,
    );
    push_gauge(
        &mut out,
        "tama:ram_used_mib",
        "RAM currently in use (MiB).",
        sys.ram_used_mib,
    );
    push_gauge(
        &mut out,
        "tama:ram_total_mib",
        "Total RAM (MiB).",
        sys.ram_total_mib,
    );

    if let Some(pct) = sys.gpu_utilization_pct {
        push_gauge(
            &mut out,
            "tama:gpu_utilization_pct",
            "GPU utilization percentage (0-100).",
            pct as u64,
        );
    }

    if let Some(ref vram) = sys.vram {
        push_gauge(
            &mut out,
            "tama:vram_used_mib",
            "VRAM currently in use (MiB).",
            vram.used_mib,
        );
        push_gauge(
            &mut out,
            "tama:vram_total_mib",
            "Total VRAM (MiB).",
            vram.total_mib,
        );
    }

    if let Some(ref net) = sys.network {
        push_gauge_f64(
            &mut out,
            "tama:net_rx_mibps",
            "Network download throughput (MiB/s).",
            net.download_mibps,
        );
        push_gauge_f64(
            &mut out,
            "tama:net_tx_mibps",
            "Network upload throughput (MiB/s).",
            net.upload_mibps,
        );
    }

    out
}

/// Push a gauge metric line (u64 value) to the output buffer.
fn push_gauge(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    out.push_str(help);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push_str(" gauge\n");
    out.push_str(name);
    out.push(' ');
    out.push_str(&value.to_string());
    out.push('\n');
}

/// Push a gauge metric line (f32 value) to the output buffer.
fn push_gauge_f32(out: &mut String, name: &str, help: &str, value: f32) {
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    out.push_str(help);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push_str(" gauge\n");
    out.push_str(name);
    out.push(' ');
    // Format with enough precision, removing trailing zeros
    let formatted = format_value(value);
    out.push_str(&formatted);
    out.push('\n');
}

/// Push a gauge metric line (f64 value) to the output buffer.
fn push_gauge_f64(out: &mut String, name: &str, help: &str, value: f64) {
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    out.push_str(help);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push_str(" gauge\n");
    out.push_str(name);
    out.push(' ');
    // Format with enough precision, removing trailing zeros
    let formatted = format_value_f64(value);
    out.push_str(&formatted);
    out.push('\n');
}

/// Format an f64 value, removing unnecessary trailing zeros.
fn format_value_f64(value: f64) -> String {
    if value == 0.0 {
        return "0.0".to_string();
    }
    if value.abs() < 1.0 {
        // Use 3 decimal places for small values to preserve precision
        format!("{:.3}", value)
    } else if value.fract() == 0.0 {
        format!("{:.1}", value)
    } else {
        format!("{:.2}", value)
    }
}

/// Format an f32 value, removing unnecessary trailing zeros.
fn format_value(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{:.1}", value)
    } else {
        format!("{:.*}", 2, value)
    }
}

/// Inject a `{server="<name>"}` label into a single Prometheus metric line.
///
/// - Metric data lines get the server label injected
/// - Comment lines (`# HELP`, `# TYPE`) and blank lines pass through unchanged
///
/// Uses simple string parsing (no regex):
/// 1. Find first `{` and first ` ` in the line
/// 2. If `{` comes first: inject `,server="<name>"` before `}`
/// 3. If ` ` comes first or no `{`: inject `{server="<name>"}` before the space
pub fn inject_backend_label(line: &str, backend_name: &str) -> String {
    // Comment lines and blank lines pass through unchanged
    if line.is_empty() || line.starts_with('#') {
        return line.to_string();
    }

    let escaped_name = escape_prometheus_label(backend_name);

    // Find the first space (separates metric name from value)
    // and first `{` (start of label block)
    let first_space = line.find(' ');
    let first_brace = line.find('{');

    match (first_space, first_brace) {
        // Has labels: name{existing="label"} value → name{existing="label",server="name"} value
        (Some(space_idx), Some(brace_idx)) if brace_idx < space_idx => {
            // Find the closing brace
            if let Some(close_brace) = line[brace_idx..].find('}') {
                let close_abs = brace_idx + close_brace;
                let mut result = String::with_capacity(line.len() + 20);
                result.push_str(&line[..close_abs]);
                result.push_str(",server=\"");
                result.push_str(&escaped_name);
                result.push('"');
                result.push_str(&line[close_abs..]);
                result
            } else {
                // Malformed — no closing brace, pass through
                line.to_string()
            }
        }
        // No labels: name value → name{server="name"} value
        _ => {
            if let Some(space_idx) = first_space {
                let mut result = String::with_capacity(line.len() + 20);
                result.push_str(&line[..space_idx]);
                result.push_str("{server=\"");
                result.push_str(&escaped_name);
                result.push_str("\"}");
                result.push_str(&line[space_idx..]);
                result
            } else {
                // No space found — just a metric name with no value, pass through
                line.to_string()
            }
        }
    }
}

/// Format all lines from a backend's `/metrics` response, injecting server labels.
pub fn format_backend_metrics(lines: &[&str], backend_name: &str) -> String {
    lines
        .iter()
        .map(|line| inject_backend_label(line, backend_name))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Escape a string for use as a Prometheus label value.
///
/// Escapes `\` → `\\`, `"` → `\"`, and newline → `\n`.
fn escape_prometheus_label(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn make_proxy_metrics() -> ProxyMetrics {
        ProxyMetrics {
            total_requests: AtomicU64::new(98),
            successful_requests: AtomicU64::new(90),
            failed_requests: AtomicU64::new(4),
        }
    }

    // ── inject_backend_label tests ──────────────────────────────────────

    #[test]
    fn test_inject_backend_label_with_existing_labels() {
        let line = "llamacpp:prompt_tokens_total{backend=\"llama\"} 32479";
        let result = inject_backend_label(line, "my-model");
        assert!(
            result.contains("backend=\"llama\""),
            "existing labels should be preserved: {}",
            result
        );
        assert!(
            result.contains(",server=\"my-model\""),
            "server label should be injected: {}",
            result
        );
        assert!(
            result.ends_with(" 32479"),
            "value should be preserved: {}",
            result
        );
    }

    #[test]
    fn test_inject_backend_label_without_labels() {
        let line = "llamacpp:n_decode_total 581";
        let result = inject_backend_label(line, "my-model");
        assert_eq!(
            result, "llamacpp:n_decode_total{server=\"my-model\"} 581",
            "server label should be injected before value"
        );
    }

    #[test]
    fn test_inject_backend_label_help_line_unchanged() {
        let line = "# HELP llamacpp:prompt_tokens_total Number of prompt tokens processed.";
        let result = inject_backend_label(line, "my-model");
        assert_eq!(result, line, "HELP line should pass through unchanged");
    }

    #[test]
    fn test_inject_backend_label_type_line_unchanged() {
        let line = "# TYPE llamacpp:prompt_tokens_total counter";
        let result = inject_backend_label(line, "my-model");
        assert_eq!(result, line, "TYPE line should pass through unchanged");
    }

    #[test]
    fn test_inject_backend_label_empty_line_unchanged() {
        let line = "";
        let result = inject_backend_label(line, "my-model");
        assert_eq!(result, "", "empty line should pass through unchanged");
    }

    #[test]
    fn test_inject_backend_label_escapes_special_chars() {
        let line = "metric_name 42";
        let result = inject_backend_label(line, "model\\with\"quotes");
        assert_eq!(
            result, "metric_name{server=\"model\\\\with\\\"quotes\"} 42",
            "special chars should be escaped"
        );
    }

    // ── format_tama_metrics tests ──────────────────────────────────────

    #[test]
    fn test_format_tama_metrics_all_fields_present() {
        let metrics = make_proxy_metrics();
        let output = format_tama_metrics(&metrics, 3);

        // Check all 6 metrics are present
        assert!(output.contains("tama:total_requests"));
        assert!(output.contains("tama:successful_requests"));
        assert!(output.contains("tama:failed_requests"));
        assert!(output.contains("tama:models_loaded"));
        assert!(output.contains("tama:models_unloaded"));
        assert!(output.contains("tama:active_models"));

        // Check HELP and TYPE lines exist for each
        for metric in [
            "tama:total_requests",
            "tama:successful_requests",
            "tama:failed_requests",
            "tama:models_loaded",
            "tama:models_unloaded",
            "tama:active_models",
        ] {
            assert!(
                output.contains(&format!("# HELP {}", metric)),
                "missing HELP for {}",
                metric
            );
            assert!(
                output.contains(&format!("# TYPE {} gauge", metric)),
                "missing TYPE for {}",
                metric
            );
        }
    }

    #[test]
    fn test_format_tama_metrics_correct_values() {
        let metrics = make_proxy_metrics();
        let output = format_tama_metrics(&metrics, 3);

        assert!(output.contains("tama:total_requests 98"));
        assert!(output.contains("tama:successful_requests 90"));
        assert!(output.contains("tama:failed_requests 4"));
        // models_loaded = the live ready count (the active_models arg);
        // models_unloaded no longer has a counter (constant 0).
        assert!(output.contains("tama:models_loaded 3"));
        assert!(output.contains("tama:models_unloaded 0"));
        assert!(output.contains("tama:active_models 3"));
    }

    #[test]
    fn test_format_tama_metrics_zero_values() {
        let metrics = ProxyMetrics::default();
        let output = format_tama_metrics(&metrics, 0);

        assert!(output.contains("tama:total_requests 0"));
        assert!(output.contains("tama:active_models 0"));
    }

    // ── format_backend_metrics tests ───────────────────────────────────

    #[test]
    fn test_format_backend_metrics_multi_line() {
        let lines = vec![
            "# HELP llamacpp:prompt_tokens_total Number of prompt tokens processed.",
            "# TYPE llamacpp:prompt_tokens_total counter",
            "llamacpp:prompt_tokens_total 32479",
            "llamacpp:n_decode_total{backend=\"a\"} 581",
        ];
        let output = format_backend_metrics(&lines, "test-server");

        // Comment lines unchanged
        assert!(output
            .contains("# HELP llamacpp:prompt_tokens_total Number of prompt tokens processed."));
        assert!(output.contains("# TYPE llamacpp:prompt_tokens_total counter"));

        // Data lines have server label
        assert!(output.contains("llamacpp:prompt_tokens_total{server=\"test-server\"} 32479"));
        assert!(
            output.contains("llamacpp:n_decode_total{backend=\"a\",server=\"test-server\"} 581")
        );
    }
}
