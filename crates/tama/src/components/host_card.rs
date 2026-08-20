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
/// shows for unknown VRAM, with a 0% bar. Shared by `HostGpuRow` and the
/// host card's RAM row so one implementation drives both.
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

/// One row of per-GPU telemetry for a tamad host card: top line shows the
/// GPU index + cleaned name, core utilization % and temperature; the bottom
/// line shows the VRAM label with a full-width usage bar.
#[component]
pub fn HostGpuRow(gpu: HostGpu) -> impl IntoView {
    let util = gpu.utilization_percent.clamp(0.0, 100.0);
    let total = gpu.vram_total_bytes.max(0) as u64;
    let used = gpu.vram_used_bytes.max(0) as u64;
    let (vram_pct, vram_text) = metric_line(used, total);
    let title = format!("GPU {} · {}", gpu.index, clean_gpu_name(&gpu.name));
    view! {
        <div class="host-gpu-row">
            <div class="host-gpu-row__top">
                <span class="host-gpu-row__name">{title}</span>
                <span class="host-gpu-row__value">
                    {format!("{util:.0}% util · {:.0}°C", gpu.temperature_c)}
                </span>
            </div>
            <div class="host-gpu-row__bottom">
                <span class="host-gpu-row__vram">{vram_text}</span>
                <div class="host-gpu-row__util-bar">
                    <div
                        class="progress-bar-fill"
                        style=format!("width: {vram_pct:.0}%")
                    />
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
                                    <div class="host-gpu-row">
                                        <div class="host-gpu-row__top">
                                            <span class="host-gpu-row__name">"CPU"</span>
                                            <span class="host-gpu-row__value">{format!("{cpu:.1}%")}</span>
                                        </div>
                                        <div class="host-gpu-row__bottom">
                                            <div class="host-gpu-row__util-bar">
                                                <div
                                                    class="progress-bar-fill"
                                                    style=format!("width: {:.0}%", cpu.clamp(0.0, 100.0))
                                                />
                                            </div>
                                        </div>
                                    </div>
                                }
                                .into_any()
                            } else {
                                view! { <div/> }.into_any()
                            }}
                            {if let Some((used, total)) = memory {
                                let (ram_pct, ram_text) = metric_line(used, total);
                                let ram_pct_text = if total > 0 {
                                    format!("{ram_pct:.0}%")
                                } else {
                                    "—".to_string()
                                };
                                view! {
                                    <div class="host-gpu-row">
                                        <div class="host-gpu-row__top">
                                            <span class="host-gpu-row__name">"RAM"</span>
                                            <span class="host-gpu-row__value">{ram_pct_text}</span>
                                        </div>
                                        <div class="host-gpu-row__bottom">
                                            <span class="host-gpu-row__vram">{ram_text}</span>
                                            <div class="host-gpu-row__util-bar">
                                                <div
                                                    class="progress-bar-fill"
                                                    style=format!("width: {ram_pct:.0}%")
                                                />
                                            </div>
                                        </div>
                                    </div>
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
}
