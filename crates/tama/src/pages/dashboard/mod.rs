use leptos::prelude::*;
use leptos_router::components::A;
use wasm_bindgen::prelude::*;

use crate::components::active_model_row::ActiveModelRow;
use crate::components::alert_banner::{AlertBanner, AlertVariant};
use crate::components::host_card::HostCard;
use crate::components::modal::Modal;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
use crate::components::{bar_chart::nice_max, BarChart};
use crate::utils::{get_request, handle_response, post_request, rw_signal_to_signal};

mod metrics;
pub use metrics::*;

#[cfg(test)]
mod tests;

#[component]
pub fn Dashboard() -> impl IntoView {
    let buckets = RwSignal::new(Vec::<MetricBucket>::new());
    let current = RwSignal::new(MetricCurrent::default());
    // Per-tamad host entries from the SSE `hosts[]` field (plan-191 Task 9).
    let hosts = RwSignal::new(Vec::<HostStats>::new());
    // Proxy-local status: (version, uptime_seconds) from /tama/v1/system/health,
    // rendered in the header status pill (plan-192 Task 2).
    let proxy_meta = RwSignal::new(None::<(String, f64)>);
    let fetch_failed = RwSignal::new(false);
    // Incrementing this signal re-runs the Effect that opens the EventSource.
    let connect_trigger = RwSignal::new(0u32);

    // Open (or re-open) an EventSource each time connect_trigger changes.
    Effect::new(move |_| {
        let _ = connect_trigger.get(); // track signal

        let es = match web_sys::EventSource::new("/tama/v1/system/metrics/stream") {
            Ok(es) => es,
            Err(_) => {
                fetch_failed.set(true);
                return;
            }
        };

        // Refresh the header status pill (version + uptime) from the health
        // endpoint — re-runs whenever the SSE stream (re)connects.
        let proxy_meta = proxy_meta;
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(resp) = get_request("/tama/v1/system/health").send().await {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    if let (Some(ver), Some(up)) = (
                        v.get("version").and_then(|x| x.as_str()),
                        v.get("uptime_seconds").and_then(|x| x.as_f64()),
                    ) {
                        proxy_meta.set(Some((ver.to_string(), up)));
                    }
                }
            }
        });

        // Handler for "snapshot" events — updates the buckets array (for bar
        // charts), the current state (for big-number displays, GPU cards,
        // model list, inference stats), and the per-tamad hosts (plan-191
        // Task 9 host cards).
        let on_snapshot =
            Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    if let Ok(snapshot) = serde_json::from_str::<MetricsSnapshot>(&data_str) {
                        fetch_failed.set(false);
                        buckets.set(snapshot.buckets);
                        current.set(snapshot.current.clone());
                        hosts.set(snapshot.hosts);
                    }
                }
            });
        let _ =
            es.add_event_listener_with_callback("snapshot", on_snapshot.as_ref().unchecked_ref());
        on_snapshot.forget();

        // Error handler — flag for the empty-history retry UI, and detect auth failures.
        let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
            fetch_failed.set(true);
            crate::utils::sse_session_check();
        });
        es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        // Close the EventSource when the effect re-runs or the component unmounts.
        on_cleanup(move || {
            es.close();
        });
    });

    // Manual retry: close and re-open the EventSource.
    let manual_refresh = move |_| {
        fetch_failed.set(false);
        connect_trigger.update(|n| *n += 1);
    };

    let restart: Action<(), (), LocalStorage> = Action::new_unsync(|_: &()| async move {
        if let Ok(resp) = post_request("/tama/v1/system/restart").send().await {
            let _ = handle_response(&resp);
        }
    });

    // Per-model unload action, wired to the same REST endpoint used by the
    // `/models` page. Unsync because `gloo_net::Request` returns `!Send`
    // futures in the WASM target.
    //
    // We use a manual "busy" signal instead of relying on Action::pending()
    // because in some WASM error scenarios (e.g. proxy returns 500 with no
    // backend configured), the pending flag can get stuck and never reset,
    // leaving buttons permanently disabled with "Loading…" text.
    let unload_busy = RwSignal::new(false);

    // Pull Model modal
    let pull_modal_open = RwSignal::new(false);

    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            unload_busy.set(true);
            // Same as load — ignore errors, SSE will push the updated state.
            if let Ok(resp) = post_request(&format!("/tama/v1/models/{}/unload", id))
                .send()
                .await
            {
                let _ = handle_response(&resp);
            }
            unload_busy.set(false);
        }
    });

    // Shared unload callback handed to every active-model row (inside host
    // cards and in the "Unassigned" group alike).
    let on_unload = Callback::new(move |id: String| {
        unload_action.dispatch(id);
    });

    view! {
        // Header control plane: title + cluster summary line, gateway status
        // pill, and the pinned Pull / Restart actions (plan-192 Task 2).
        <div class="page-header">
            <div class="dashboard-header">
                <h1>"Dashboard"</h1>
                <div class="dashboard-cluster-subtitle">
                    {move || {
                        let host_list = hosts.get();
                        let cur = current.get();
                        let gpu_count: usize = host_list.iter().map(|h| h.gpus.len()).sum();
                        let active_count = loaded_or_starting_models(&cur.models).len();
                        format_cluster_subtitle(host_list.len(), gpu_count, active_count, cur.tps)
                    }}
                </div>
            </div>
            <div class="page-header-actions">
                {move || {
                    if fetch_failed.get() {
                        view! {
                            <span class="gateway-pill gateway-pill--offline">
                                {gateway_status_text(false, None, None)}
                            </span>
                        }
                    } else {
                        let meta = proxy_meta.get();
                        view! {
                            <span class="gateway-pill gateway-pill--online">
                                {gateway_status_text(
                                    true,
                                    meta.as_ref().map(|(v, _)| v.as_str()),
                                    meta.as_ref().map(|(_, u)| *u),
                                )}
                            </span>
                        }
                    }
                }}
                <button class="btn btn-secondary" on:click=move |_| {
                    restart.dispatch(());
                }>"Restart"</button>
                <button class="btn btn-secondary" on:click=move |_| pull_modal_open.set(true)>"Pull Model"</button>
            </div>
        </div>

        {move || {
            let buf = buckets.get();
            let cur = current.get();
            if fetch_failed.get() && buf.is_empty() {
                // Network error, no data yet — show error with retry button
                return view! {
                    <div class="card">
                        <AlertBanner variant=AlertVariant::Error>"Failed to load metrics stream. Is Tama running?"</AlertBanner>
                        <button class="btn btn-secondary btn-sm mt-2" on:click=manual_refresh>"Retry"</button>
                    </div>
                }.into_any();
            }

            // Inference telemetry (plan-192 Task 3): the 15-minute TG/PP
            // series from pre-aggregated buckets plus the per-card live
            // values. The backend owns the aggregation — the view only
            // derives chart y-ceilings (peak == 0 → flat 1.0 scale) and
            // per-token latencies up front.
            let telemetry = build_inference_telemetry(&buf);
            let timestamps: Vec<i64> = buf.iter().map(|s| s.ts_unix_ms).collect();
            let has_data = !buf.is_empty();
            // Each sparkline scales against its own window peak (TG vs PP
            // differ by orders of magnitude), guarded so an empty window
            // renders a flat 1.0 scale instead of a zero-height chart.
            let tg_y_max = if telemetry.tg_peak > 0.0 {
                nice_max(telemetry.tg_peak)
            } else {
                1.0
            };
            let pp_y_max = if telemetry.pp_peak > 0.0 {
                nice_max(telemetry.pp_peak)
            } else {
                1.0
            };
            // Token Generation card: live tok/s + derived inter-token latency.
            let tg_visible = has_data && cur.tps.is_some();
            let tg_value = match cur.tps {
                Some(t) => format!("{t:.1} tok/s"),
                None => "—".to_string(),
            };
            let tg_secondary = cur.tps.and_then(ms_per_token).map(|ms| {
                format!("ITL {ms:.1} ms/tok · peak {:.0} tok/s", telemetry.tg_peak)
            });
            // Prompt Processing card: live tok/s + derived prefill latency.
            let pp_visible = has_data && cur.prompt_tps.is_some();
            let pp_value = match cur.prompt_tps {
                Some(t) => format!("{t:.1} tok/s"),
                None => "—".to_string(),
            };
            let pp_secondary = cur.prompt_tps.and_then(ms_per_token).map(|ms| {
                format!("prefill {ms:.1} ms/tok · peak {:.0} tok/s", telemetry.pp_peak)
            });

            view! {

                // Hosts section — one card per registered tamad, with the
                // former "Active Models" section merged in (host-centric
                // grouping): models attributed to a host render inside its
                // card; hostless or unmatched models render in the
                // "Unassigned" group below the grid. The full catalog with
                // sort/group management lives on `/tama/models` (plan-192
                // Task 2).
                {move || {
                    let host_list = hosts.get();
                    let cur = current.get();
                    let active = loaded_or_starting_models(&cur.models);
                    let active_count = active.len();
                    let host_names: Vec<String> =
                        host_list.iter().map(|h| h.name.clone()).collect();
                    let (mut by_host, unassigned) =
                        partition_models_by_host(active, &host_names);
                    // GPU chips in the unassigned rows resolve against all
                    // tamad hosts' GPUs — the proxy presents no local
                    // hardware (plan-191 Task 9).
                    let all_host_gpus: Vec<HostGpu> = host_list
                        .iter()
                        .flat_map(|host| host.gpus.iter().cloned())
                        .collect();
                    let gpus_for_labels = host_gpus_to_device_stats(&all_host_gpus);
                    let host_count = host_list.len();
                    let cards: Vec<AnyView> = host_list
                        .iter()
                        .map(|h| {
                            let meta = h.clone();
                            let models_for_host =
                                by_host.remove(&meta.name).unwrap_or_default();
                            view! {
                                <HostCard
                                    name=meta.name
                                    online=meta.online
                                    cpu_percent=Some(meta.cpu_percent)
                                    memory=Some((
                                        meta.memory.used_bytes,
                                        meta.memory.total_bytes,
                                    ))
                                    gpus=meta.gpus
                                    running_models=models_for_host
                                    on_unload=on_unload
                                    unload_busy=unload_busy.into()
                                />
                            }
                            .into_any()
                        })
                        .collect();
                    view! {
                        <section class="dashboard-hosts">
                            <div class="page-header">
                                <h2>"Hosts"</h2>
                                <div class="models-toolbar">
                                    <span class="text-muted">
                                        {format!(
                                            "{} tamad{} · {} loaded",
                                            host_count,
                                            if host_count == 1 { "" } else { "s" },
                                            active_count,
                                        )}
                                    </span>
                                    <A attr:class="btn btn-secondary btn-sm" href="/tama/models">
                                        "Manage Models →"
                                    </A>
                                </div>
                            </div>
                            <div class="host-card-grid">{cards}</div>
                            {if host_list.is_empty() && active_count == 0 {
                                view! {
                                    <div class="card card--centered">
                                        <p class="text-muted">
                                            "No tamads registered — start a tamad on your inference host to connect compute."
                                        </p>
                                    </div>
                                }
                                .into_any()
                            } else {
                                view! { <div/> }.into_any()
                            }}
                            {if !unassigned.is_empty() {
                                let unassigned_count = unassigned.len();
                                let rows: Vec<AnyView> = unassigned
                                    .iter()
                                    .map(|m| {
                                        view! {
                                            <ActiveModelRow
                                                model=m.clone()
                                                gpus_for_labels=gpus_for_labels.clone()
                                                unload_busy=unload_busy.into()
                                                on_unload=on_unload
                                            />
                                        }
                                        .into_any()
                                    })
                                    .collect();
                                view! {
                                    <div class="host-unassigned">
                                        <div class="host-unassigned__head">
                                            <h3>"Unassigned"</h3>
                                            <span class="text-muted">
                                                {format!("{unassigned_count} models active")}
                                            </span>
                                        </div>
                                        <div class="active-models-list">{rows}</div>
                                    </div>
                                }
                                .into_any()
                            } else if active_count == 0 {
                                view! {
                                    <div class="card card--centered">
                                        <p class="text-muted">
                                            "⚪ No models currently active · "
                                            <A href="/tama/models">"Browse & Load a Model →"</A>
                                        </p>
                                    </div>
                                }
                                .into_any()
                            } else {
                                view! { <div/> }.into_any()
                            }}
                        </section>
                    }.into_any()
                }}

                // Inference Telemetry section — pure gateway inference
                // metrics (plan-192 Task 3): live generation/prefill
                // throughput with 15-minute sparklines from the
                // pre-aggregated SSE buckets, plus cache & speculative
                // decoding efficiency. Rendered last — at the bottom of the
                // page, below the Active Models and Hosts sections. Cards
                // reuse .stat-card / .sparkline-container; the grid is just
                // .grid-stats + the --inference modifier.
                <section class="dashboard-telemetry">
                    <div class="telemetry-heading">
                        <h2>"Inference Telemetry"</h2>
                        <span class="text-muted">"(Past 15 minutes)"</span>
                    </div>
                    <div class="grid-stats grid-stats--inference">
                        // Token Generation card — live generation throughput
                        // + 15m green sparkline.
                        <div class="stat-card">
                            <div class="stat-card-head">
                                <div class="card-header">"Token Generation"</div>
                                <div class="stat-card-value-group">
                                    {if tg_visible {
                                        view! {
                                            <div class="card-value">{tg_value.clone()}</div>
                                            {if let Some(sec) = &tg_secondary {
                                                view! {
                                                    <div class="card-secondary">{sec.clone()}</div>
                                                }.into_any()
                                            } else {
                                                view! { <span></span> }.into_any()
                                            }}
                                        }.into_any()
                                    } else {
                                        view! {
                                            <div class="card-value-empty">"—"</div>
                                        }.into_any()
                                    }}
                                </div>
                            </div>
                            <div class="sparkline-container">
                                <BarChart
                                    data=telemetry.tg.clone()
                                    max_value=tg_y_max
                                    color="var(--accent-green)".to_string()
                                    height=60.0
                                    timestamps=timestamps.clone()
                                    unit_label="tok/s".to_string()
                                />
                            </div>
                        </div>

                        // Prompt Processing card — live prefill throughput
                        // + 15m blue sparkline.
                        <div class="stat-card">
                            <div class="stat-card-head">
                                <div class="card-header">"Prompt Processing"</div>
                                <div class="stat-card-value-group">
                                    {if pp_visible {
                                        view! {
                                            <div class="card-value">{pp_value.clone()}</div>
                                            {if let Some(sec) = &pp_secondary {
                                                view! {
                                                    <div class="card-secondary">{sec.clone()}</div>
                                                }.into_any()
                                            } else {
                                                view! { <span></span> }.into_any()
                                            }}
                                        }.into_any()
                                    } else {
                                        view! {
                                            <div class="card-value-empty">"—"</div>
                                        }.into_any()
                                    }}
                                </div>
                            </div>
                            <div class="sparkline-container">
                                <BarChart
                                    data=telemetry.pp.clone()
                                    max_value=pp_y_max
                                    color="var(--accent-blue)".to_string()
                                    height=60.0
                                    timestamps=timestamps.clone()
                                    unit_label="tok/s".to_string()
                                />
                            </div>
                        </div>

                        // Cache & Speculative Efficiency card — no sparkline;
                        // prompt-cache hit rate + draft acceptance rate with
                        // a live decoding status.
                        <div class="stat-card">
                            <div class="card-header">"Cache & Speculative Efficiency"</div>
                            <div class="efficiency-grid">
                                <div class="efficiency-item">
                                    {match cur.cache_hit_pct {
                                        Some(p) => view! {
                                            <div class="card-value">{format!("{p:.0}%")}</div>
                                        }.into_any(),
                                        None => view! {
                                            <div class="card-value-empty">"—"</div>
                                        }.into_any(),
                                    }}
                                    <div class="card-secondary">"Prefix/KV Cache Hit"</div>
                                </div>
                                <div class="efficiency-item">
                                    {match cur.spec_accept_pct {
                                        Some(p) => view! {
                                            <div class="card-value">{format!("{p:.0}%")}</div>
                                        }.into_any(),
                                        None => view! {
                                            <div class="card-value-empty">"—"</div>
                                        }.into_any(),
                                    }}
                                    <div class="card-secondary">"Speculative Acceptance"</div>
                                </div>
                            </div>
                            <div class="sparkline-container">
                                {if cur.spec_decoding_active {
                                    view! {
                                        <span class="spec-status spec-status--active">
                                            "● spec decoding active"
                                        </span>
                                    }.into_any()
                                } else {
                                    view! {
                                        <span class="spec-status text-muted">
                                            "○ spec decoding inactive"
                                        </span>
                                    }.into_any()
                                }}
                            </div>
                        </div>
                    </div>
                </section>

            }.into_any()
        }}

        <Modal
            open=rw_signal_to_signal(pull_modal_open)
            on_close=Callback::new(move |_| pull_modal_open.set(false))
            title="Pull Model".to_string()
        >
            <PullQuantWizard
                initial_repo=Signal::derive(String::new)
                is_open=rw_signal_to_signal(pull_modal_open)
                on_complete=Callback::new(move |_completed: Vec<CompletedQuant>| {
                    pull_modal_open.set(false);
                    connect_trigger.update(|n| *n += 1);
                })
                on_close=Callback::new(move |_| pull_modal_open.set(false))
            />
        </Modal>
    }
}
