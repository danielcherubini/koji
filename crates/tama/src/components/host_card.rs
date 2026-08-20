//! Host card component — one card per registered tamad on the dashboard
//! Hosts section (plan-191 Task 9).
//!
//! Tamad cards show name, online status, CPU, RAM, per-GPU
//! VRAM/utilization/temperature from the SSE `hosts[]` stream, and the
//! models actively running on the node (host-centric grouping — the old
//! standalone "Active Models" section was merged into these cards).

use leptos::prelude::*;

use crate::components::active_model_row::ActiveModelRow;
use crate::pages::dashboard::{
    format_bytes_gib, format_bytes_gib_rounded, host_gpus_to_device_stats, HostGpu,
    ModelStateSnapshot,
};

/// Format a GPU VRAM label as `used / total GiB`, e.g. `28.6 GiB / 32 GiB`.
///
/// Both helpers already include the `GiB` unit, so this must not append
/// another one. Returns `"—"` when the total is unknown (zero).
pub fn vram_label(used_bytes: u64, total_bytes: u64) -> String {
    if total_bytes == 0 {
        return "—".to_string();
    }
    format!(
        "{} / {}",
        format_bytes_gib(used_bytes),
        format_bytes_gib_rounded(total_bytes)
    )
}

/// Strip a leading vendor/system prefix from a raw GPU name so only the
/// device name remains, e.g. `"Advanced Micro Devices, Inc. [AMD/ATI] Radeon
/// Pro W7900"` -> `"Radeon Pro W7900"`, and drop a trailing PCI id suffix
/// like `" [10de:2685]"` when one remains. No-op for names that already
/// look clean (e.g. `"Radeon Pro W7900"`).
///
/// When the name is vendor-only (no device name after the label), cleaning
/// would leave nothing usable — an empty string, a bare bracketed tag like
/// `[AMD/ATI]`, or a corporate fragment like `Corporation` — so the raw
/// (trimmed) string is returned instead.
pub fn clean_gpu_name(raw: &str) -> String {
    let mut name = raw.trim();
    // System prefixes run up to and including "], " (e.g. "... [AMD/ATI] ").
    if let Some(pos) = name.find("], ") {
        name = &name[pos + 3..];
    }
    // Remaining known vendor labels (longest first so the specific ones win).
    const VENDOR_PREFIXES: &[&str] = &[
        "NVIDIA Corporation ",
        "Advanced Micro Devices, Inc. ",
        "Advanced Micro Devices ",
        "Intel(R)(R) ",
        "Intel(R) ",
        "Intel ",
        "NVIDIA ",
    ];
    for prefix in VENDOR_PREFIXES {
        if let Some(stripped) = name.strip_prefix(prefix) {
            name = stripped;
            break;
        }
    }
    // Drop a leading system tag like "[AMD/ATI] " left behind after the
    // vendor label (only when the whole tag begins the name).
    if let Some(end) = name.find("] ") {
        let tag = &name[..end];
        if tag.starts_with('[') {
            name = &name[end + 2..];
        }
    }
    // Drop a trailing PCI identifier, e.g. "[10de:2685]".
    if let Some(open) = name.rfind('[') {
        let suffix = &name[open..];
        let inner = suffix.strip_prefix('[').and_then(|s| s.strip_suffix(']'));
        if inner.is_some_and(is_pci_id) {
            name = name[..open].trim_end();
        }
    }
    let cleaned = name.trim().to_string();
    // Vendor-only names leave nothing usable behind (empty string, a bare
    // bracketed tag, or a corporate fragment) — fall back to the raw string
    // so the row still shows something meaningful.
    if cleaned.is_empty() || is_bare_tag(&cleaned) || VENDOR_FRAGMENTS.contains(&cleaned.as_str()) {
        raw.trim().to_string()
    } else {
        cleaned
    }
}

/// Words that appear in vendor labels (e.g. `NVIDIA Corporation`,
/// `Advanced Micro Devices, Inc.`) but are never part of a device name —
/// a cleaned-up result made of one of these is a vendor fragment, not a
/// device.
const VENDOR_FRAGMENTS: &[&str] = &["Corporation", "Inc.", "Inc", "Corp.", "Corp"];

/// True when the whole string is a single bracketed system tag such as
/// `[AMD/ATI]` (not a device name).
fn is_bare_tag(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0] == b'[' && b[b.len() - 1] == b']'
}

/// Returns true for PCI ids like `10de:2685` (hex digits, colon, hex digits).
fn is_pci_id(inner: &str) -> bool {
    let Some((vendor, device)) = inner.split_once(':') else {
        return false;
    };
    !vendor.is_empty()
        && !device.is_empty()
        && vendor.len() <= 4
        && device.len() <= 4
        && vendor.bytes().all(|b| b.is_ascii_hexdigit())
        && device.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Properties for a host metric row (GPU VRAM, RAM): the usage percent
/// clamped to 0–100 and the bottom-line label `used / total GiB (N%)`.
///
/// A zero (unknown) total renders the same `"—"` placeholder the GPU row
/// shows for unknown VRAM, with a 0% bar. The GPU row uses both values;
/// the host card's RAM row uses the percent only (its caption was dropped
/// so the CPU/RAM groups render as identical two-row metrics).
pub fn metric_line(used_bytes: u64, total_bytes: u64) -> (f64, String) {
    if total_bytes == 0 {
        return (0.0, vram_label(0, 0));
    }
    let pct = (used_bytes as f64 / total_bytes as f64 * 100.0).clamp(0.0, 100.0);
    (
        pct,
        format!("{} ({pct:.0}%)", vram_label(used_bytes, total_bytes)),
    )
}

/// CSS `style` value for a usage bar fill: `width: N%` with the percent
/// rounded and clamped to 0–100 so corrupt telemetry can never overflow
/// the track or go negative.
///
/// Shared by every bar in a host card — CPU, RAM, GPU utilization, and
/// GPU VRAM — so they are all driven by one implementation.
pub fn bar_width_style(pct: f64) -> String {
    format!("width: {:.0}%", pct.clamp(0.0, 100.0))
}

/// The rendered values for one GPU telemetry row (see [`HostGpuRow`]).
///
/// The row is three lines: (1) title left + temperature right,
/// (2) `util` label + utilization bar + right-aligned percent text,
/// (3) VRAM label + bar. Building the strings here keeps the markup in
/// `view!` declarative while the formatting logic stays unit-testable.
#[derive(Debug, Clone)]
pub struct GpuRowLines {
    /// Line 1 (left): `GPU {i} · {cleaned name}`.
    pub title: String,
    /// Line 1 (right): temperature, e.g. `52°C`.
    pub temp: String,
    /// Line 2: fill style for the utilization bar, e.g. `width: 31%`.
    pub util_bar: String,
    /// Line 2 (right): utilization percent text, e.g. `31%`.
    pub util_pct: String,
    /// Line 3 (left): VRAM label, e.g. `28.6 GiB / 32 GiB (89%)`.
    pub vram_label: String,
    /// Line 3: fill style for the VRAM bar, e.g. `width: 89%`.
    pub vram_bar: String,
}

/// Build the display values for one GPU telemetry row (see
/// [`GpuRowLines`]). Utilization is clamped to 0–100 for BOTH the bar
/// fill and the percent text; VRAM is shared with [`metric_line`] so an
/// unknown total renders `"—"` with a 0% bar.
pub fn gpu_row_lines(gpu: &HostGpu) -> GpuRowLines {
    let util = gpu.utilization_percent.clamp(0.0, 100.0);
    let used = gpu.vram_used_bytes.max(0) as u64;
    let total = gpu.vram_total_bytes.max(0) as u64;
    let (vram_pct, vram_label) = metric_line(used, total);
    GpuRowLines {
        title: format!("GPU {} · {}", gpu.index, clean_gpu_name(&gpu.name)),
        temp: format!("{:.0}°C", gpu.temperature_c),
        util_bar: bar_width_style(util),
        util_pct: format!("{util:.0}%"),
        vram_label,
        vram_bar: bar_width_style(vram_pct),
    }
}

/// One row of per-GPU telemetry for a tamad host card. Three lines:
/// (1) GPU index + cleaned name with the temperature right-aligned,
/// (2) a `util` label + utilization bar + percent text, (3) the VRAM
/// label with its usage bar. All bars share the `.host-gpu-row__util-bar`
/// track so they render at identical height across GPU blocks.
#[component]
pub fn HostGpuRow(gpu: HostGpu) -> impl IntoView {
    let GpuRowLines {
        title,
        temp,
        util_bar,
        util_pct,
        vram_label,
        vram_bar,
    } = gpu_row_lines(&gpu);
    view! {
        <div class="host-gpu-row">
            <div class="host-gpu-row__top">
                <span class="host-gpu-row__name">{title}</span>
                <span class="host-gpu-row__value">{temp}</span>
            </div>
            <div class="host-gpu-row__util-line">
                <span class="host-gpu-row__metric-label">"util"</span>
                <div class="host-gpu-row__util-bar">
                    <div class="progress-bar-fill" style=util_bar/>
                </div>
                <span class="host-gpu-row__metric-value">{util_pct}</span>
            </div>
            <div class="host-gpu-row__bottom">
                <span class="host-gpu-row__vram">{vram_label}</span>
                <div class="host-gpu-row__util-bar">
                    <div class="progress-bar-fill" style=vram_bar/>
                </div>
            </div>
        </div>
    }
}

/// One host card metric group (CPU or RAM): a column with exactly two
/// rows — (a) a label row with the metric name left and a plain
/// percentage value right, (b) a bar row. CPU and RAM reuse one component
/// so the two `.host-metrics` groups are guaranteed identical in
/// structure and height: their labels and bars sit on the same lines.
/// (GPU rows keep their own three-line structure — [`HostGpuRow`].)
#[component]
pub fn HostMetricGroup(
    /// Metric name, left of the label row (e.g. `CPU`).
    name: String,
    /// Plain percentage value, right of the label row (e.g. `17%`).
    value: String,
    /// Fill style for the bar (e.g. `width: 17%`, see [`bar_width_style`]).
    bar: String,
) -> impl IntoView {
    view! {
        <div class="host-gpu-row">
            <div class="host-gpu-row__top">
                <span class="host-gpu-row__name">{name}</span>
                <span class="host-gpu-row__value">{value}</span>
            </div>
            <div class="host-gpu-row__bottom">
                <div class="host-gpu-row__util-bar">
                    <div class="progress-bar-fill" style=bar/>
                </div>
            </div>
        </div>
    }
}

/// HostCard — one card per tamad.
///
/// Shows CPU + RAM on one compact line, one row per GPU, and — below the
/// GPU section — the models actively running on this node (same row markup
/// as the Hosts section's "Unassigned" group, via [`ActiveModelRow`]).
#[component]
pub fn HostCard(
    /// Host display name (tamad name).
    name: String,
    /// Whether the host is currently reachable (tamad stats stream open).
    online: bool,
    /// CPU usage % (None hides the CPU metric).
    cpu_percent: Option<f64>,
    /// RAM used/total in bytes (None hides the RAM metric).
    memory: Option<(u64, u64)>,
    /// GPUs (tamad hosts only).
    gpus: Vec<HostGpu>,
    /// Models actively running on this node; empty hides the models section.
    #[prop(default = Vec::new())]
    running_models: Vec<ModelStateSnapshot>,
    /// Dispatched with the model id when a row's Unload button is clicked.
    on_unload: Callback<String>,
    /// Shared unload-in-progress flag forwarded to each model row.
    unload_busy: Signal<bool>,
) -> impl IntoView {
    view! {
        <div class="card host-card">
            <div class="host-card__head">
                <div class="host-card__title">{name}</div>
                {if online {
                    view! { <span class="badge badge-success">"● online"</span> }.into_any()
                } else {
                    view! { <span class="badge badge-danger">"● offline"</span> }.into_any()
                }}
            </div>
            {if online {
                view! {
                    <div class="host-card__stats">
                        <div class="host-metrics">
                            {if let Some(cpu) = cpu_percent {
                                view! {
                                    <HostMetricGroup
                                        name="CPU".to_string()
                                        value=format!("{cpu:.1}%")
                                        bar=bar_width_style(cpu)
                                    />
                                }
                                .into_any()
                            } else {
                                view! { <div/> }.into_any()
                            }}
                            {if let Some((used, total)) = memory {
                                let ram_pct = metric_line(used, total).0;
                                let value = if total > 0 {
                                    format!("{ram_pct:.0}%")
                                } else {
                                    "—".to_string()
                                };
                                // Same group component as CPU: the old
                                // `used / total GiB (N%)` caption was
                                // dropped — the % value already carries it.
                                view! {
                                    <HostMetricGroup
                                        name="RAM".to_string()
                                        value=value
                                        bar=bar_width_style(ram_pct)
                                    />
                                }
                                .into_any()
                            } else {
                                view! { <div/> }.into_any()
                            }}
                        </div>
                    </div>
                    {if !gpus.is_empty() {
                        view! {
                            <div class="host-card__gpus">
                                <div class="host-card__gpus-title">
                                    {"GPU"}
                                    <span class="text-muted">{format!("( {} )", gpus.len())}</span>
                                </div>
                                {gpus.iter().map(|g| {
                                    let gpu = g.clone();
                                    view! { <HostGpuRow gpu=gpu/> }
                                }).collect::<Vec<_>>()}
                            </div>
                        }
                        .into_any()
                    } else {
                        view! { <div/> }.into_any()
                    }}
                    {if !running_models.is_empty() {
                        // GPU chips resolve against this host's own GPUs —
                        // models attributed to a host run on that host.
                        let gpus_for_labels = host_gpus_to_device_stats(&gpus);
                        view! {
                            <div class="host-card__models">
                                <div class="host-card__models-title">
                                    {"Active on this node"}
                                    <span class="text-muted">
                                        {format!("( {} )", running_models.len())}
                                    </span>
                                </div>
                                <div class="active-models-list">
                                    {running_models.iter().map(|m| {
                                        view! {
                                            <ActiveModelRow
                                                model=m.clone()
                                                gpus_for_labels=gpus_for_labels.clone()
                                                unload_busy=unload_busy
                                                on_unload=on_unload
                                            />
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>
                        }
                        .into_any()
                    } else {
                        view! { <div/> }.into_any()
                    }}
                }
                .into_any()
            } else {
                view! {
                    <div class="host-card__offline">
                        "stats unavailable — waiting for the stats stream"
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 28.6 GiB used of a 32 GiB GPU — the live-node regression case.
    const USED_28_6_GIB: u64 = 30_685_282_304;
    const TOTAL_32_GIB: u64 = 34_359_738_368;

    /// One live-node GPU (Radeon Pro W7900) at 31% util / 52°C with
    /// 28.6 of 32 GiB VRAM used — the regression case for the GPU row.
    fn live_node_gpu() -> HostGpu {
        HostGpu {
            index: 0,
            name: "Advanced Micro Devices, Inc. [AMD/ATI] Radeon Pro W7900".to_string(),
            vram_used_bytes: USED_28_6_GIB as i64,
            vram_total_bytes: TOTAL_32_GIB as i64,
            utilization_percent: 31.4,
            temperature_c: 52.4,
            ..Default::default()
        }
    }

    #[test]
    fn test_vram_label_no_duplicate_unit() {
        let label = vram_label(USED_28_6_GIB, TOTAL_32_GIB);
        assert!(!label.contains("GiB GiB"), "duplicate unit in {label:?}");
    }

    #[test]
    fn test_vram_label_exact_format() {
        let label = vram_label(USED_28_6_GIB, TOTAL_32_GIB);
        assert_eq!(label, "28.6 GiB / 32 GiB");
    }

    #[test]
    fn test_vram_label_unknown_total() {
        assert_eq!(vram_label(1024, 0), "—");
    }

    /// 10 GiB used of a 60 GiB host — the live host-card regression case.
    /// The percent must be clamped 0–100 and the label must be
    /// `used / total GiB (N%)` with a rounded percent in parens.
    #[test]
    fn test_metric_line_percent_and_label_format() {
        let gib = 1024 * 1024 * 1024;
        let (pct, label) = metric_line(10 * gib, 60 * gib);
        assert!((pct - 100.0 / 6.0).abs() < 1e-9, "pct {pct}");
        assert_eq!(label, "10.0 GiB / 60 GiB (17%)");
    }

    /// Corrupt input (used > total) — the bar fill must never exceed 100%.
    #[test]
    fn test_metric_line_clamps_over_100() {
        let gib = 1024 * 1024 * 1024;
        let (pct, label) = metric_line(20 * gib, 16 * gib);
        assert_eq!(pct, 100.0);
        assert_eq!(label, "20.0 GiB / 16 GiB (100%)");
    }

    /// Zero (unknown) total — the same “—” placeholder the GPU row shows
    /// for unknown VRAM, with a 0% bar.
    #[test]
    fn test_metric_line_unknown_total_is_dash() {
        let (pct, label) = metric_line(1024, 0);
        assert_eq!(pct, 0.0);
        assert_eq!(label, "—");
    }

    /// Zero used — a 0% fill and an explicit 0% label.
    #[test]
    fn test_metric_line_zero_used() {
        let gib = 1024 * 1024 * 1024;
        let (pct, label) = metric_line(0, 64 * gib);
        assert_eq!(pct, 0.0);
        assert_eq!(label, "0 GiB / 64 GiB (0%)");
    }

    /// 128 GiB used of a 256 GiB GPU — both values hit the `>=100 GiB`
    /// zero-decimal branch of `format_bytes_gib`/`format_bytes_gib_rounded`.
    #[test]
    fn test_vram_label_hundred_gib_zero_decimals() {
        let used = 128 * 1024 * 1024 * 1024;
        let total = 256 * 1024 * 1024 * 1024;
        assert_eq!(vram_label(used, total), "128 GiB / 256 GiB");
    }

    #[test]
    fn test_clean_gpu_name_strips_amd_prefix() {
        assert_eq!(
            clean_gpu_name("Advanced Micro Devices, Inc. [AMD/ATI] Radeon Pro W7900"),
            "Radeon Pro W7900"
        );
    }

    #[test]
    fn test_clean_gpu_name_strips_nvidia_vendor_and_pci_id() {
        assert_eq!(
            clean_gpu_name("NVIDIA Corporation GeForce RTX 4090 [10de:2685]"),
            "GeForce RTX 4090"
        );
    }

    #[test]
    fn test_clean_gpu_name_strips_intel_prefix() {
        assert_eq!(
            clean_gpu_name("Intel(R) UHD Graphics 770"),
            "UHD Graphics 770"
        );
    }

    #[test]
    fn test_clean_gpu_name_noop_when_clean() {
        assert_eq!(clean_gpu_name("Radeon Pro W7900"), "Radeon Pro W7900");
    }

    #[test]
    fn test_clean_gpu_name_empty() {
        assert_eq!(clean_gpu_name(""), "");
    }

    /// Vendor label with no device name — stripping the `NVIDIA ` prefix
    /// would leave a bare corporate fragment (`"Corporation"`), so the
    /// raw name is returned instead of the fragment.
    #[test]
    fn test_clean_gpu_name_vendor_only_nvidia() {
        assert_eq!(clean_gpu_name("NVIDIA Corporation"), "NVIDIA Corporation");
        assert_eq!(
            clean_gpu_name("NVIDIA Corporation "),
            "NVIDIA Corporation",
            "trailing-space variant must not leave the row empty"
        );
    }

    /// Vendor label with only its bracketed system tag — the tag alone
    /// (`"[AMD/ATI]"`) is not a device name, so the raw name is returned.
    #[test]
    fn test_clean_gpu_name_vendor_only_amd() {
        assert_eq!(
            clean_gpu_name("Advanced Micro Devices, Inc. [AMD/ATI]"),
            "Advanced Micro Devices, Inc. [AMD/ATI]"
        );
    }

    #[test]
    fn test_bar_width_style_rounds_percent() {
        assert_eq!(bar_width_style(89.0), "width: 89%");
        assert_eq!(bar_width_style(31.4), "width: 31%");
        assert_eq!(bar_width_style(0.4), "width: 0%");
    }

    /// Corrupt telemetry outside 0–100 — the fill must never overflow or
    /// go negative.
    #[test]
    fn test_bar_width_style_clamps_out_of_range() {
        assert_eq!(bar_width_style(-5.0), "width: 0%");
        assert_eq!(bar_width_style(112.0), "width: 100%");
    }

    #[test]
    fn test_gpu_row_lines_live_node() {
        let lines = gpu_row_lines(&live_node_gpu());
        assert_eq!(lines.title, "GPU 0 · Radeon Pro W7900");
        assert_eq!(lines.temp, "52°C");
        assert_eq!(lines.util_bar, "width: 31%");
        assert_eq!(lines.util_pct, "31%");
        assert_eq!(lines.vram_label, "28.6 GiB / 32 GiB (89%)");
        assert_eq!(lines.vram_bar, "width: 89%");
    }

    /// Corrupt telemetry (utilization > 100) — the bar fill AND the text
    /// must both clamp to 100, so the row can't read `100%` with a wider
    /// claim than the bar shows.
    #[test]
    fn test_gpu_row_lines_clamps_util_over_100() {
        let mut gpu = live_node_gpu();
        gpu.utilization_percent = 112.0;
        let lines = gpu_row_lines(&gpu);
        assert_eq!(lines.util_bar, "width: 100%");
        assert_eq!(lines.util_pct, "100%");
    }

    /// Unknown VRAM total — the same `“—”` placeholder the row used to
    /// show, with a 0% bar fill.
    #[test]
    fn test_gpu_row_lines_unknown_vram_is_dash() {
        let mut gpu = live_node_gpu();
        gpu.vram_total_bytes = 0;
        let lines = gpu_row_lines(&gpu);
        assert_eq!(lines.vram_label, "—");
        assert_eq!(lines.vram_bar, "width: 0%");
    }

    /// The rendered GPU row must be three lines: (1) title left +
    /// temperature right, (2) small `util` label + utilization bar + %
    /// text, (3) VRAM label + bar. The old combined `N% util · N°C` value
    /// is gone.
    #[test]
    fn test_gpu_row_renders_title_temp_util_and_vram_lines() {
        let html = view! { <HostGpuRow gpu=live_node_gpu()/> }.to_html();
        // Line 1: title left, temperature as its own right-aligned value.
        assert!(
            html.contains("<span class=\"host-gpu-row__name\">GPU 0 · Radeon Pro W7900</span>"),
            "title line: {html}"
        );
        assert!(
            html.contains("<span class=\"host-gpu-row__value\">52°C</span>"),
            "temp line: {html}"
        );
        assert!(
            !html.contains("util ·"),
            "combined util/temp value must be gone: {html}"
        );
        // Line 2: `util` label, bar filled to the util %, % text on the right.
        assert!(
            html.contains("host-gpu-row__util-line"),
            "util line: {html}"
        );
        assert!(
            html.contains("<span class=\"host-gpu-row__metric-label\">util</span>"),
            "util label: {html}"
        );
        assert!(
            html.contains("width: 31%"),
            "util bar must fill to 31%: {html}"
        );
        assert!(
            html.contains("<span class=\"host-gpu-row__metric-value\">31%</span>"),
            "util % text: {html}"
        );
        // Line 3: VRAM label (format unchanged) + bar.
        assert!(
            html.contains("28.6 GiB / 32 GiB (89%)"),
            "vram label: {html}"
        );
        assert!(
            html.contains("width: 89%"),
            "vram bar must fill to 89%: {html}"
        );
        // Row order: title → util → vram.
        let top = html.find("host-gpu-row__top").unwrap();
        let util_line = html.find("host-gpu-row__util-line").unwrap();
        let vram = html.find("host-gpu-row__vram").unwrap();
        assert!(
            top < util_line && util_line < vram,
            "row order top → util → vram: {html}"
        );
    }

    /// The CPU and RAM metric groups must render with IDENTICAL structure
    /// — a label row (name left + plain % value right) followed by a bar
    /// row — so both groups' labels and bars sit on the same horizontal
    /// lines. The old RAM `used / total GiB (N%)` caption is gone.
    #[test]
    fn test_host_card_renders_cpu_ram_groups_identically() {
        let cpu_html = view! {
            <HostMetricGroup name="CPU".to_string() value="42.3%".to_string() bar="width: 42%".to_string()/>
        }
        .to_html();
        let ram_html = view! {
            <HostMetricGroup name="RAM".to_string() value="17%".to_string() bar="width: 17%".to_string()/>
        }
        .to_html();
        // Structural identity: redacting the metric-specific strings must
        // leave byte-identical markup for both groups.
        let strip = |h: &str, name: &str, value: &str, bar: &str| {
            h.replace(&format!(">{name}<"), ">METRIC<")
                .replace(bar, "STYLE")
                .replace(value, "VALUE")
        };
        assert_eq!(
            strip(&cpu_html, "CPU", "42.3%", "width: 42%"),
            strip(&ram_html, "RAM", "17%", "width: 17%"),
            "CPU and RAM groups must share identical markup: CPU={cpu_html} RAM={ram_html}"
        );
        // The absolute contract on one group: name + plain % value on the
        // label row, exactly one bar on the bar row, no VRAM caption span.
        assert!(
            cpu_html.contains("<span class=\"host-gpu-row__name\">CPU</span>"),
            "name: {cpu_html}"
        );
        assert!(
            cpu_html.contains("<span class=\"host-gpu-row__value\">42.3%</span>"),
            "value: {cpu_html}"
        );
        assert!(cpu_html.contains("width: 42%"), "bar: {cpu_html}");
        assert!(
            !cpu_html.contains("host-gpu-row__vram"),
            "caption must be dropped: {cpu_html}"
        );
        assert_eq!(
            cpu_html.matches("host-gpu-row__util-bar").count(),
            1,
            "exactly one bar: {cpu_html}"
        );
        // Label row comes before the bar row.
        let top = cpu_html.find("host-gpu-row__top").unwrap();
        let bar = cpu_html.find("host-gpu-row__util-bar").unwrap();
        assert!(top < bar, "label row before bar row: {cpu_html}");
    }
}
