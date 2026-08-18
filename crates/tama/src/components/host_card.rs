//! Host card component — one card per registered tamad on the dashboard
//! system section (plan-191 Task 9), plus the proxy-local card.
//!
//! Tamad cards show name, online status, version, CPU, RAM, and per-GPU
//! VRAM/utilization/temperature from the SSE `hosts[]` stream. The proxy
//! card shows only its own version + uptime — the proxy presents no
//! hardware (ADR-0010).

use leptos::prelude::*;

use crate::pages::dashboard::{format_bytes_gib, format_bytes_gib_rounded, HostGpu};

/// One row of per-GPU telemetry for a tamad host card.
#[component]
pub fn HostGpuRow(gpu: HostGpu) -> impl IntoView {
    let util = gpu.utilization_percent;
    let vram_label = if gpu.vram_total_bytes > 0 {
        format!(
            "{} / {} GiB",
            format_bytes_gib(gpu.vram_used_bytes.max(0) as u64),
            format_bytes_gib_rounded(gpu.vram_total_bytes.max(0) as u64)
        )
    } else {
        "—".to_string()
    };
    let name = gpu.name.clone();
    view! {
        <div class="host-gpu-row">
            <span class="host-gpu-row__name">{name}</span>
            <span class="host-gpu-row__vram">{vram_label}</span>
            <div class="host-gpu-row__util-bar">
                <div
                    class="progress-bar-fill"
                    style=format!("width: {:.0}%", util.clamp(0.0, 100.0))
                />
            </div>
            <span class="host-gpu-row__value">{format!("{util:.0}%")}</span>
            <span class="host-gpu-row__temp">{format!("{:.0}°C", gpu.temperature_c)}</span>
        </div>
    }
}

/// HostCard — one card per tamad (or the proxy-local card).
///
/// For the proxy card, pass `cpu_percent = None`, `memory = None` and an
/// empty `gpus` with `uptime` set: the card then shows only version +
/// uptime (the proxy presents no hardware).
#[component]
pub fn HostCard(
    /// Host display name (tamad name, or "Proxy").
    name: String,
    /// Whether the host is currently reachable (tamad stats stream open).
    online: bool,
    /// The host's self-reported version.
    version: Option<String>,
    /// CPU usage % (None hides the CPU/RAM row — proxy card).
    cpu_percent: Option<f64>,
    /// RAM used/total in bytes (None hides the RAM row — proxy card).
    memory: Option<(u64, u64)>,
    /// GPUs (tamad hosts only).
    gpus: Vec<HostGpu>,
    /// Extra info line under the header (e.g. "Up 2h 13m" for the proxy).
    uptime: Option<String>,
) -> impl IntoView {
    view! {
        <div class="card host-card">
            <div class="host-card__head">
                <div class="host-card__title">{name}</div>
                {if online {
                    view! { <span class="badge badge-success">"online"</span> }.into_any()
                } else {
                    view! { <span class="badge badge-danger">"offline"</span> }.into_any()
                }}
            </div>
            <div class="host-card__info">
                {if let Some(ref v) = version {
                    v.clone()
                } else {
                    "—".to_string()
                }}
                {if let Some(ref up) = uptime {
                    format!(" · Up {up}")
                } else {
                    String::new()
                }}
            </div>
            {if online {
                view! {
                    <div class="host-card__stats">
                        {if let Some(cpu) = cpu_percent {
                            view! {
                                <div class="host-card__stat">
                                    <span class="host-card__stat-label">"CPU"</span>
                                    <span class="host-card__stat-value">{format!("{cpu:.1}%")}</span>
                                    <div class="progress-bar">
                                        <div
                                            class="progress-bar-fill"
                                            style=format!("width: {:.0}%", cpu.clamp(0.0, 100.0))
                                        />
                                    </div>
                                </div>
                            }
                            .into_any()
                        } else {
                            view! { <div/> }.into_any()
                        }}
                        {if let Some((used, total)) = memory {
                            view! {
                                <div class="host-card__stat">
                                    <span class="host-card__stat-label">"RAM"</span>
                                    <span class="host-card__stat-value">
                                        {format!("{} / {}", format_bytes_gib(used), format_bytes_gib_rounded(total))}
                                    </span>
                                    <div class="progress-bar">
                                        <div
                                            class="progress-bar-fill"
                                            style=format!(
                                                "width: {:.1}%",
                                                if total > 0 {
                                                    used as f64 / total as f64 * 100.0
                                                } else {
                                                    0.0
                                                }
                                            )
                                        />
                                    </div>
                                </div>
                            }
                            .into_any()
                        } else {
                            view! { <div/> }.into_any()
                        }}
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
