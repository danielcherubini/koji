//! MTP (Multi-Token Prediction) benchmark form and results display.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::job_log_panel::JobLogPanel;
use crate::pages::benchmarks::selectors::{BackendSelect, ModelEntry, ModelQuantSelect};
use crate::pages::benchmarks::utils::{
    format_mean_stddev, parse_sizes, split_id_quant, split_name_variant, submit_bench_job,
    BenchmarkFormState,
};
use crate::utils::target_value;

#[component]
pub fn MtpBench(
    /// Trigger to bump history refetch after a run completes.
    history_refresh_trigger: RwSignal<u32>,
    shared_state: BenchmarkFormState,
) -> impl IntoView {
    // ── Shared form state (hoisted from parent) ────────────────────────
    let BenchmarkFormState {
        selected_display_name,
        selected_model,
        available_models,
        selected_backend,
        available_backends,
        ..
    } = shared_state;

    // ── Per-tab job state (isolated from other tabs) ───────────────────
    let is_running = RwSignal::new(false);
    let current_job_id = RwSignal::new(Option::<String>::None);
    let benchmark_results = RwSignal::new(Option::<serde_json::Value>::None);

    // ── MTP configuration ──────────────────────────────────────────────
    let draft_max_str = RwSignal::new("0,1,2,3,4,5,6,7,8".to_string());
    let ngl_str = RwSignal::new("99".to_string());
    let flash_attn = RwSignal::new(true);

    // ── Per-form state ─────────────────────────────────────────────────
    let error_msg = RwSignal::new(String::new());

    // ── Submit handler ─────────────────────────────────────────────────
    let submit_benchmark = move || {
        let raw_model = selected_model.get();
        if raw_model.is_empty() {
            return;
        }
        let (model_id, quant) = split_id_quant(&raw_model);

        // Parse "name:variant" format from selected backend.
        let raw_backend = selected_backend.get();
        let (backend_name, gpu_variant) = split_name_variant(&raw_backend);

        let draft_max_values = parse_sizes(&draft_max_str.get());
        let draft_ngl: Option<u32> = if ngl_str.get().is_empty() {
            None
        } else {
            ngl_str.get().parse::<u32>().ok()
        };
        let flash = flash_attn.get();

        benchmark_results.set(None);
        is_running.set(true);
        current_job_id.set(None);

        spawn_local(async move {
            let body = serde_json::json!({
                "model_id": model_id,
                "quant": quant,
                "backend_name": backend_name,
                "gpu_variant": gpu_variant,
                "draft_max_values": draft_max_values,
                "ngl": 99u32,
                "draft_ngl": draft_ngl,
                "flash_attn": flash,
            });

            match submit_bench_job("/tama/v1/benchmarks/mtp-run", body).await {
                Ok(job_id) => {
                    current_job_id.set(Some(job_id));
                }
                Err(err) => {
                    error_msg.set(err);
                    is_running.set(false);
                }
            }
        });
    };

    // ── SSE callbacks ──────────────────────────────────────────────────
    let on_result_cb = Callback::new(move |results_json: String| {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&results_json) {
            benchmark_results.set(Some(parsed));
        }
        // Receiving a result event means the job is done — bump history.
        is_running.set(false);
        history_refresh_trigger.update(|n| *n += 1);
    });
    let on_status_cb = Callback::new(move |status: String| {
        if status != "running" {
            is_running.set(false);
            history_refresh_trigger.update(|n| *n += 1);
        }
    });

    // ── Read-only splits for views ─────────────────────────────────────
    let (available_models_sig, _) = available_models.split();
    let (selected_model_sig, _) = selected_model.split();
    let (available_backends_sig, _) = available_backends.split();
    let (draft_max_sig, _) = draft_max_str.split();
    let (ngl_sig, _) = ngl_str.split();
    let (flash_sig, _) = flash_attn.split();
    let (is_running_sig, _) = is_running.split();
    let (current_job_id_sig, _) = current_job_id.split();
    let (error_sig, _) = error_msg.split();
    let (benchmark_results_sig, _) = benchmark_results.split();

    view! {
        <div>
            // ── Model selection ───────────────────────────────────────
            <section class="card">
                <h3>"Model"</h3>
                <ModelQuantSelect
                    models=available_models_sig
                    selected_model=selected_display_name
                    selected_quant=selected_model
                    label_suffix_fn=Some(|entry: &ModelEntry| {
                        // Entry index 5 is supports_mtp.
                        if entry.5 {
                            None
                        } else {
                            Some("no MTP".to_string())
                        }
                    })
                />
            </section>

            // ── Backend selection ─────────────────────────────────────
            <section class="card">
                <h3>"Backend"</h3>
                <BackendSelect
                    backends=available_backends_sig
                    selected_backend=selected_backend
                    hint_text="Select a specific backend variant, or leave empty to use the model's backend."
                />
            </section>

            // ── MTP Configuration ─────────────────────────────────────
            <section class="card">
                <h3>"MTP Configuration"</h3>
                <div class="grid-2">
                    <div class="form-group">
                        <label>"Draft-n-max values"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || draft_max_sig.get()
                            on:input=move |e| { draft_max_str.set(target_value(&e)); }
                        />
                        <small class="text-muted">"Comma-separated, e.g. 0,1,2,3,4,5,6,7,8"</small>
                    </div>
                    <div class="form-group">
                        <label>"GPU layers"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || ngl_sig.get()
                            on:input=move |e| { ngl_str.set(target_value(&e)); }
                        />
                        <small class="text-muted">"GPU layers for the draft model (default 99)"</small>
                    </div>
                    <div class="form-group">
                        <div class="form-check">
                            <input
                                id="mtp-flash-attn"
                                type="checkbox"
                                prop:checked=move || flash_sig.get()
                                on:change=move |e| {
                                    flash_attn.set(event_target_checked(&e));
                                }
                            />
                            <label class="form-check-label" for="mtp-flash-attn">"Flash attention"</label>
                        </div>
                    </div>
                </div>
            </section>

            // ── Run button ────────────────────────────────────────────
            <div class="text-center my-3">
                <button
                    class="btn btn-primary btn-lg"
                    prop:disabled=move || selected_model_sig.get().is_empty() || is_running_sig.get()
                    on:click=move |_| { submit_benchmark(); }
                >
                    {move || if is_running_sig.get() { "Running..." } else { "▶ Run MTP Benchmark" }}
                </button>
            </div>

            // ── Error display ─────────────────────────────────────────
            {move || {
                let err = error_sig.get();
                if !err.is_empty() {
                    view! {
                        <div class="alert alert-danger mt-2">
                            <p class="mb-0">{err}</p>
                        </div>
                    }.into_any()
                } else {
                    ().into_view().into_any()
                }
            }}

            // ── Progress / logs ───────────────────────────────────────
            {move || {
                if let Some(job_id) = current_job_id_sig.get() {
                    view! {
                        <JobLogPanel
                            job_id=job_id
                            on_result=on_result_cb
                            on_status=on_status_cb
                        />
                    }.into_any()
                } else {
                    ().into_view().into_any()
                }
            }}

            // ── Results display ───────────────────────────────────────
            {move || {
                let Some(result) = benchmark_results_sig.get() else {
                    return ().into_view().into_any();
                };

                let entries: Vec<serde_json::Value> = result
                    .get("entries")
                    .and_then(|v| v.as_array())
                    .map(|a| a.to_vec())
                    .unwrap_or_default();

                let aggregate = result.get("aggregate");
                let agg_accept_rate = aggregate
                    .and_then(|a| a.get("aggregate_accept_rate"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let agg_total_predicted = aggregate
                    .and_then(|a| a.get("total_predicted"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let agg_total_draft = aggregate
                    .and_then(|a| a.get("total_draft"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let agg_total_draft_accepted = aggregate
                    .and_then(|a| a.get("total_draft_accepted"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let agg_wall_total = aggregate
                    .and_then(|a| a.get("wall_s_total"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                if entries.is_empty() {
                    return ().into_view().into_any();
                }

                // Group entries by draft_max value
                let mut groups: std::collections::BTreeMap<u64, Vec<serde_json::Value>> =
                    std::collections::BTreeMap::new();
                for entry in &entries {
                    let draft_max = entry
                        .get("draft_max")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    groups.entry(draft_max).or_default().push(entry.clone());
                }

                view! {
                    <section class="card mt-3">
                        <h3>"MTP Benchmark Results"</h3>

                        // Aggregate summary
                        <div class="bench-summary">
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Total Predicted"</div>
                                <div class="bench-summary__value">{agg_total_predicted.to_string()}</div>
                            </div>
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Total Draft"</div>
                                <div class="bench-summary__value">{agg_total_draft.to_string()}</div>
                            </div>
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Total Accepted"</div>
                                <div class="bench-summary__value">{agg_total_draft_accepted.to_string()}</div>
                            </div>
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Accept Rate"</div>
                                <div class="bench-summary__value">{format!("{:.1}%", agg_accept_rate * 100.0)}</div>
                            </div>
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Wall Time"</div>
                                <div class="bench-summary__value">{format!("{:.1} s", agg_wall_total)}</div>
                            </div>
                        </div>

                        // Per-draft_max group tables
                        {groups.into_iter().map(|(draft_max, group_entries)| {
                            let is_baseline = draft_max == 0;
                            let group_label = if is_baseline {
                                "Baseline (draft-n-max: 0)".to_string()
                            } else {
                                format!("Draft-n-max: {}", draft_max)
                            };

                            // Compute group aggregates
                            let group_wall_total: f64 = group_entries.iter()
                                .filter_map(|e| e.get("wall_s").and_then(|v| v.as_f64()))
                                .sum();
                            let group_pred_total: u64 = group_entries.iter()
                                .filter_map(|e| e.get("predicted_n").and_then(|v| v.as_u64()))
                                .sum();
                            let group_draft_total: u64 = group_entries.iter()
                                .filter_map(|e| e.get("draft_n").and_then(|v| v.as_u64()))
                                .sum();
                            let group_draft_accepted: u64 = group_entries.iter()
                                .filter_map(|e| e.get("draft_n_accepted").and_then(|v| v.as_u64()))
                                .sum();
                            let group_accept_rate = if group_draft_total > 0 {
                                group_draft_accepted as f64 / group_draft_total as f64
                            } else {
                                0.0
                            };
                            let group_avg_tok_s: f64 = group_entries.iter()
                                .filter_map(|e| e.get("predicted_per_second").and_then(|v| v.as_f64()))
                                .sum::<f64>() / group_entries.len() as f64;

                            view! {
                                <div class="mt-3">
                                    <h4>{group_label.clone()}</h4>
                                    <table class="table table-striped">
                                        <thead>
                                            <tr>
                                                <th>"Prompt"</th>
                                                <th class="text-right">"Wall (s)"</th>
                                                <th class="text-right">"Pred"</th>
                                                <th class="text-right">"Draft"</th>
                                                <th class="text-right">"Acc"</th>
                                                <th class="text-right">"Rate"</th>
                                                <th class="text-right">"tok/s"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {group_entries.into_iter().map(|entry| {
                                                let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                                let wall_s = entry.get("wall_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                                let predicted_n = entry.get("predicted_n").and_then(|v| v.as_u64()).unwrap_or(0);
                                                let draft_n = entry.get("draft_n").and_then(|v| v.as_u64()).unwrap_or(0);
                                                let draft_n_accepted = entry.get("draft_n_accepted").and_then(|v| v.as_u64()).unwrap_or(0);
                                                let accept_rate = entry.get("accept_rate").and_then(|v| v.as_f64());
                                                let tok_per_s = entry.get("predicted_per_second").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                                let error: Option<String> = entry
                                                    .get("error")
                                                    .and_then(|v| v.as_str())
                                                    .map(|s| s.to_string());

                                                let rate_display = accept_rate
                                                    .map(|r| format!("{:.0}%", r * 100.0))
                                                    .unwrap_or_else(|| "—".to_string());

                                                let row_class = if error.is_some() {
                                                    "table-danger"
                                                } else {
                                                    ""
                                                };

                                                view! {
                                                    <tr class=row_class>
                                                        <td>
                                                            {name.clone()}
                                                            {if let Some(err) = error {
                                                                view! { <br /><small class="text-danger">{err}</small> }.into_any()
                                                            } else {
                                                                ().into_view().into_any()
                                                            }}
                                                        </td>
                                                        <td class="text-mono text-right">{format!("{:.2}", wall_s)}</td>
                                                        <td class="text-mono text-right">{predicted_n}</td>
                                                        <td class="text-mono text-right">{draft_n}</td>
                                                        <td class="text-mono text-right">{draft_n_accepted}</td>
                                                        <td class="text-mono text-right">{rate_display}</td>
                                                        <td class="text-mono text-right">{format_mean_stddev(tok_per_s, 0.0)}</td>
                                                    </tr>
                                                }
                                            }).collect::<Vec<_>>()}
                                            // Group aggregate row
                                            <tr class="table-active">
                                                <td><strong>"Group Total"</strong></td>
                                                <td class="text-mono text-right"><strong>{format!("{:.2}", group_wall_total)}</strong></td>
                                                <td class="text-mono text-right"><strong>{group_pred_total}</strong></td>
                                                <td class="text-mono text-right"><strong>{group_draft_total}</strong></td>
                                                <td class="text-mono text-right"><strong>{group_draft_accepted}</strong></td>
                                                <td class="text-mono text-right"><strong>{format!("{:.0}%", group_accept_rate * 100.0)}</strong></td>
                                                <td class="text-mono text-right"><strong>{format_mean_stddev(group_avg_tok_s, 0.0)}</strong></td>
                                            </tr>
                                        </tbody>
                                    </table>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </section>
                }.into_any()
            }}
        </div>
    }
}
