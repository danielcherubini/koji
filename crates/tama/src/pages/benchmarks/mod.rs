//! Benchmarks page — run llama-bench and spec-decoding benchmarks from the web UI.

mod mtp_bench;
mod spec_bench;
mod types;
mod utils;

use std::collections::{BTreeMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use self::mtp_bench::MtpBench;
use self::spec_bench::SpecBench;
use self::types::{HistoryEntry, BENCHMARK_TYPES, LLAMA_BENCH_PRESETS};
use self::utils::{
    fetch_bench_backends, format_mean_stddev, format_relative, format_timestamp, parse_sizes,
    split_id_quant, use_benchmark_form_state, BenchmarkFormState,
};
use crate::components::job_log_panel::JobLogPanel;
use crate::components::tab_buttons::{TabButton, TabButtons};
use crate::utils::{extract_and_store_csrf_token, get_request, post_request};

/// Render a table of per-summary results, adding columns for whichever
/// per-run knobs actually vary between rows. A column for a constant knob is
/// redundant with the header card and would just add noise — so we only add
/// one when the field has more than one distinct value across the rows.
///
/// Shared between the live benchmark results and the history accordion detail
/// panel so both look identical.
fn render_summaries_table(summaries: &[serde_json::Value]) -> impl IntoView {
    let get_u64 = |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_u64());
    let get_str =
        |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_str()).map(|x| x.to_string());
    let get_bool = |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_bool());

    // Which per-run knobs vary across rows? Only those get a column.
    let distinct_u64 =
        |k: &str| -> HashSet<u64> { summaries.iter().filter_map(|s| get_u64(s, k)).collect() };
    let distinct_str =
        |k: &str| -> HashSet<String> { summaries.iter().filter_map(|s| get_str(s, k)).collect() };
    let distinct_bool =
        |k: &str| -> HashSet<bool> { summaries.iter().filter_map(|s| get_bool(s, k)).collect() };

    let show_depth = distinct_u64("n_depth").len() > 1;
    let show_batch = distinct_u64("n_batch").len() > 1;
    let show_ubatch = distinct_u64("n_ubatch").len() > 1;
    // KV cache is expressed by two fields. Treat them as a single "KV" column
    // that varies when either side varies.
    let show_kv = distinct_str("type_k").len() > 1 || distinct_str("type_v").len() > 1;
    let show_fa = distinct_bool("flash_attn").len() > 1;

    let rows: Vec<_> = summaries
        .iter()
        .map(|s| {
            let n_prompt = get_u64(s, "prompt_tokens").unwrap_or(0);
            let n_gen = get_u64(s, "gen_tokens").unwrap_or(0);
            let pp_mean = s.get("pp_mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let pp_stddev = s.get("pp_stddev").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tg_mean = s.get("tg_mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tg_stddev = s.get("tg_stddev").and_then(|v| v.as_f64()).unwrap_or(0.0);

            let (test_label, phase, value) = if n_prompt > 0 && n_gen == 0 {
                (
                    format!("pp{}", n_prompt),
                    "PP".to_string(),
                    format_mean_stddev(pp_mean, pp_stddev),
                )
            } else if n_prompt == 0 && n_gen > 0 {
                (
                    format!("tg{}", n_gen),
                    "TG".to_string(),
                    format_mean_stddev(tg_mean, tg_stddev),
                )
            } else {
                (
                    format!("pp{}+tg{}", n_prompt, n_gen),
                    "TG".to_string(),
                    format_mean_stddev(tg_mean, tg_stddev),
                )
            };

            let depth = get_u64(s, "n_depth")
                .map(|v| v.to_string())
                .unwrap_or_default();
            let batch = get_u64(s, "n_batch")
                .map(|v| v.to_string())
                .unwrap_or_default();
            let ubatch = get_u64(s, "n_ubatch")
                .map(|v| v.to_string())
                .unwrap_or_default();
            let kv = {
                let k = get_str(s, "type_k").unwrap_or_default();
                let v = get_str(s, "type_v").unwrap_or_default();
                if k.is_empty() && v.is_empty() {
                    String::new()
                } else if k == v {
                    k
                } else {
                    format!("{}/{}", k, v)
                }
            };
            let fa = get_bool(s, "flash_attn")
                .map(|v| if v { "on" } else { "off" }.to_string())
                .unwrap_or_default();

            (test_label, phase, value, depth, batch, ubatch, kv, fa)
        })
        .collect();

    view! {
        <table class="table table-striped">
            <thead>
                <tr>
                    <th>"Test"</th>
                    <th>"Phase"</th>
                    {show_depth.then(|| view! { <th>"Depth"</th> })}
                    {show_batch.then(|| view! { <th>"Batch"</th> })}
                    {show_ubatch.then(|| view! { <th>"µ-batch"</th> })}
                    {show_kv.then(|| view! { <th>"KV"</th> })}
                    {show_fa.then(|| view! { <th>"Flash"</th> })}
                    <th class="text-right">"t/s (± stddev)"</th>
                </tr>
            </thead>
            <tbody>
                {rows.into_iter().map(|(test_label, phase, value, depth, batch, ubatch, kv, fa)| {
                    view! {
                        <tr>
                            <td class="text-mono">{test_label}</td>
                            <td>{phase}</td>
                            {show_depth.then(|| view! { <td class="text-mono">{depth}</td> })}
                            {show_batch.then(|| view! { <td class="text-mono">{batch}</td> })}
                            {show_ubatch.then(|| view! { <td class="text-mono">{ubatch}</td> })}
                            {show_kv.then(|| view! { <td class="text-mono">{kv}</td> })}
                            {show_fa.then(|| view! { <td>{fa}</td> })}
                            <td class="text-mono text-right">{value}</td>
                        </tr>
                    }
                }).collect::<Vec<_>>()}
            </tbody>
        </table>
    }
}

/// Render a table of spec-decoding benchmark summaries.
///
/// Columns: SPEC TYPE | DRAFT MAX | T/S (±STDDEV) | Δ%
fn render_spec_table(summaries: &[serde_json::Value]) -> impl IntoView {
    let get_u64 = |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_u64());
    let get_str =
        |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_str()).map(|x| x.to_string());

    fn format_delta(delta_pct: f64) -> String {
        if delta_pct >= 0.0 {
            format!("+{:.1}%", delta_pct)
        } else {
            format!("−{:.1}%", (-delta_pct))
        }
    }

    fn delta_badge_class(delta_pct: f64) -> &'static str {
        if delta_pct > 0.5 {
            "badge badge-success"
        } else if delta_pct < -0.5 {
            "badge badge-danger"
        } else {
            "badge badge-muted"
        }
    }

    let rows: Vec<_> = summaries
        .iter()
        .map(|s| {
            let spec_type = get_str(s, "spec_type").unwrap_or_else(|| "—".to_string());
            let draft_max = get_u64(s, "draft_max").unwrap_or(0);
            let tg_mean = s.get("tg_mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tg_stddev = s.get("tg_stddev").and_then(|v| v.as_f64()).unwrap_or(0.0);

            // Prefer pre-rendered delta_pct_display from the backend,
            // fall back to formatting delta_pct raw value.
            let delta_display = if let Some(d) = get_str(s, "delta_pct_display") {
                d
            } else {
                format_delta(s.get("delta_pct").and_then(|v| v.as_f64()).unwrap_or(0.0))
            };
            let delta_class =
                delta_badge_class(s.get("delta_pct").and_then(|v| v.as_f64()).unwrap_or(0.0));

            let ts_display = format_mean_stddev(tg_mean, tg_stddev);

            (spec_type, draft_max, ts_display, delta_display, delta_class)
        })
        .collect();

    view! {
        <table class="table table-striped">
            <thead>
                <tr>
                    <th>"Spec Type"</th>
                    <th>"Draft Max"</th>
                    <th class="text-right">"t/s (± stddev)"</th>
                    <th class="text-right">"Δ%"</th>
                </tr>
            </thead>
            <tbody>
                {rows.into_iter().map(|(spec_type, draft_max, ts_display, delta_display, delta_class)| {
                    view! {
                        <tr>
                            <td>{spec_type}</td>
                            <td class="text-mono">{draft_max}</td>
                            <td class="text-mono text-right">{ts_display}</td>
                            <td class="text-mono text-right">
                                <span class={delta_class}>{delta_display}</span>
                            </td>
                        </tr>
                    }
                }).collect::<Vec<_>>()}
            </tbody>
        </table>
    }
}

/// Render a table of MTP (Multi-Token Prediction) benchmark summaries.
///
/// Columns: TEST | DRAFT MAX | T/S | ACCEPT %
fn render_mtp_table(summaries: &[serde_json::Value]) -> impl IntoView {
    let get_u64 = |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_u64());
    let get_str =
        |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_str()).map(|x| x.to_string());

    let rows: Vec<_> = summaries
        .iter()
        .map(|s| {
            let name = get_str(s, "name").unwrap_or_else(|| "—".to_string());
            let draft_max = get_u64(s, "draft_max").unwrap_or(0);
            let tg_mean = s.get("tg_mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let accept_rate = s.get("accept_rate").and_then(|v| v.as_f64());

            let ts_display = format_mean_stddev(tg_mean, 0.0);
            let acc_display = accept_rate
                .map(|r| format!("{:.0}%", r * 100.0))
                .unwrap_or_else(|| "—".to_string());

            (name, draft_max, ts_display, acc_display)
        })
        .collect();

    view! {
        <table class="table table-striped">
            <thead>
                <tr>
                    <th>"Test"</th>
                    <th>"Draft Max"</th>
                    <th class="text-right">"t/s"</th>
                    <th class="text-right">"Accept %"</th>
                </tr>
            </thead>
            <tbody>
                {rows.into_iter().map(|(name, draft_max, ts_display, acc_display)| {
                    view! {
                        <tr>
                            <td>{name}</td>
                            <td class="text-mono">{draft_max}</td>
                            <td class="text-mono text-right">{ts_display}</td>
                            <td class="text-mono text-right">{acc_display}</td>
                        </tr>
                    }
                }).collect::<Vec<_>>()}
            </tbody>
        </table>
    }
}

#[component]
pub fn Benchmarks() -> impl IntoView {
    // Shared benchmark form state (model/backend selection, job tracking)
    let state = use_benchmark_form_state();
    let BenchmarkFormState {
        selected_display_name,
        selected_model,
        available_models,
        selected_backend,
        available_backends,
        is_running,
        current_job_id,
        benchmark_results,
        model_refresh: _,
    } = state;

    // Fetch available backends for llama-bench selection.
    fetch_bench_backends(available_backends);

    // Test Type dropdown — selects a preset benchmark type that auto-fills form fields.
    let selected_bench_type = RwSignal::new("baseline".to_string());

    // Test configuration
    let pp_sizes_str = RwSignal::new("512".to_string());
    let tg_sizes_str = RwSignal::new("128".to_string());
    let runs = RwSignal::new(3u32);
    let warmup = RwSignal::new(1u32);
    let threads_str = RwSignal::new("auto".to_string());
    let ngl_range = RwSignal::new("".to_string());
    let ctx_override = RwSignal::new("".to_string());

    // Methodology-driven knobs (from llm-inference-tuning-methodology.md):
    //   -b / -ub  : batch / micro-batch — biggest single PP win documented (~36%)
    //   -ctk/-ctv : KV cache quant — MUST be matched or attention falls back to CPU
    //   -d        : depth — pre-fill N tokens, essential when evaluating KV quant
    //   -fa       : flash attention — default on for modern backends
    let batch_sizes_str = RwSignal::new("".to_string());
    let ubatch_sizes_str = RwSignal::new("".to_string());
    let kv_cache_type = RwSignal::new("default".to_string());
    let depth_str = RwSignal::new("".to_string());
    let flash_attn = RwSignal::new(true);

    // History state — always visible.
    let history = RwSignal::new(Vec::<HistoryEntry>::new());
    // IDs of history rows whose per-summary detail panel is open. Each row acts
    // as an accordion toggle — clicking flips its id in this set.
    let expanded_history = RwSignal::new(HashSet::<i64>::new());

    // Trigger for history refetch — incremented whenever we want to reload.
    let history_refresh = RwSignal::new(0u32);

    // Fetch benchmark history on mount and whenever history_refresh changes.
    Effect::new(move |_| {
        let _ = history_refresh.get();
        spawn_local(async move {
            if let Ok(resp) = get_request("/tama/v1/benchmarks/history").send().await {
                extract_and_store_csrf_token(&resp);
                if let Ok(entries) = resp.json::<Vec<HistoryEntry>>().await {
                    history.set(entries);
                }
            }
        });
    });

    let parse_threads = move |s: &str| -> Option<Vec<u32>> {
        if s.trim().to_lowercase() == "auto" || s.trim().is_empty() {
            None
        } else {
            Some(
                s.split(',')
                    .map(|v| v.trim().parse::<u32>().unwrap_or(0))
                    .filter(|v| *v > 0)
                    .collect(),
            )
        }
    };

    // Test Type auto-fill handler — when the user picks a benchmark type,
    // auto-populate the relevant form fields.
    let apply_bench_type = move |bench_type: &str| {
        if let Some((_, preset)) = LLAMA_BENCH_PRESETS.iter().find(|(k, _)| *k == bench_type) {
            pp_sizes_str.set(preset.pp_sizes.to_string());
            tg_sizes_str.set(preset.tg_sizes.to_string());
            batch_sizes_str.set(preset.batch_sizes.to_string());
            ubatch_sizes_str.set(preset.ubatch_sizes.to_string());
            kv_cache_type.set(preset.kv_cache_type.to_string());
            depth_str.set(preset.depth.to_string());
        }
    };

    // Submit benchmark and connect SSE
    let submit_benchmark = move || {
        // selected_model holds "id:quant" — split to extract both parts.
        let raw_model = selected_model.get();
        let (model_id, quant) = split_id_quant(&raw_model);
        let pp = parse_sizes(&pp_sizes_str.get());
        let tg = parse_sizes(&tg_sizes_str.get());
        let runs_val = runs.get();
        let warmup_val = warmup.get();
        let threads = parse_threads(&threads_str.get());
        let ngl = if ngl_range.get().is_empty() {
            None
        } else {
            Some(ngl_range.get())
        };
        let ctx = if ctx_override.get().is_empty() {
            None
        } else {
            ctx_override.get().parse::<u32>().ok()
        };

        // Methodology knobs
        let batch_sizes = parse_sizes(&batch_sizes_str.get());
        let ubatch_sizes = parse_sizes(&ubatch_sizes_str.get());
        let kv = kv_cache_type.get();
        let kv_payload: Option<String> = if kv == "default" { None } else { Some(kv) };
        let depth = parse_sizes(&depth_str.get());
        let fa_payload: Option<bool> = Some(flash_attn.get());

        // Clear any previous results and mark the job as running.
        benchmark_results.set(None);
        is_running.set(true);
        current_job_id.set(None);

        spawn_local(async move {
            let backend_name = if selected_backend.get().is_empty() {
                None
            } else {
                Some(selected_backend.get())
            };
            let body = serde_json::json!({
                "model_id": model_id,
                "quant": quant,
                "backend_name": backend_name,
                "benchmark_type": Some(selected_bench_type.get()),
                "pp_sizes": pp,
                "tg_sizes": tg,
                "runs": runs_val,
                "warmup": warmup_val,
                "threads": threads,
                "ngl_range": ngl,
                "ctx_override": ctx,
                "batch_sizes": batch_sizes,
                "ubatch_sizes": ubatch_sizes,
                "kv_cache_type": kv_payload,
                "depth": depth,
                "flash_attn": fa_payload,
            });

            let submitted = async {
                let resp = post_request("/tama/v1/benchmarks/run")
                    .header("Content-Type", "application/json")
                    .body(body.to_string())
                    .ok()?;
                let resp = resp.send().await.ok()?;
                let body = resp.json::<serde_json::Value>().await.ok()?;
                body.get("job_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }
            .await;

            match submitted {
                Some(job_id) => {
                    current_job_id.set(Some(job_id));
                }
                None => {
                    // Submission failed — roll back is_running so the user can retry.
                    is_running.set(false);
                }
            }
        });
    };

    // Callbacks passed to JobLogPanel.
    let on_result_cb = Callback::new(move |results_json: String| {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&results_json) {
            benchmark_results.set(Some(parsed));
        }
        history_refresh.update(|n| *n += 1);
    });
    let on_status_cb = Callback::new(move |status: String| {
        if status != "running" {
            is_running.set(false);
            history_refresh.update(|n| *n += 1);
        }
    });

    // Read-only splits for views
    let (available_models_sig, _) = available_models.split();
    let (selected_display_sig, _) = selected_display_name.split();
    let (selected_model_sig, _) = selected_model.split();
    let (available_backends_sig, _) = available_backends.split();
    let (pp_sizes_sig, _) = pp_sizes_str.split();
    let (tg_sizes_sig, _) = tg_sizes_str.split();
    let (runs_sig, _) = runs.split();
    let (warmup_sig, _) = warmup.split();
    let (threads_sig, _) = threads_str.split();
    let (ngl_sig, _) = ngl_range.split();
    let (batch_sig, _) = batch_sizes_str.split();
    let (ubatch_sig, _) = ubatch_sizes_str.split();
    let (kv_sig, _) = kv_cache_type.split();
    let (depth_sig, _) = depth_str.split();
    let (fa_sig, _) = flash_attn.split();
    let (history_sig, _) = history.split();
    let (expanded_sig, _) = expanded_history.split();
    let (is_running_sig, _) = is_running.split();
    let (current_job_id_sig, _) = current_job_id.split();

    // Tab toggle — switch between llama-bench and spec-decoding views.
    let active_tab: RwSignal<String> = RwSignal::new("llama-bench".to_string());

    view! {
        <div class="page-header">
            <h1>"Benchmarks"</h1>
        </div>

        // Tab buttons
        <TabButtons
            active=Signal::derive(move || active_tab.get().to_string())
            tabs=vec![
                TabButton { key: "llama-bench".into(), label: "LLaMA-Bench".into() },
                TabButton { key: "spec-decode".into(), label: "Spec Decoding".into() },
                TabButton { key: "mtp-testing".into(), label: "MTP Testing".into() },
            ]
            on_select=Callback::new(move |key| active_tab.set(key))
        />

        // LLaMA-Bench tab content
        {move || {
            if active_tab.get() == "mtp-testing" {
                view! { <MtpBench /> }.into_any()
            } else if active_tab.get() == "spec-decode" {
                view! { <SpecBench /> }.into_any()
            } else {
                view! {
                    // Test Type dropdown
                    <section class="card">
                        <h3>"Test Type"</h3>
                        <select
                            class="form-select"
                            on:change=move |e| {
                                let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                                selected_bench_type.set(val.clone());
                                apply_bench_type(&val);
                            }
                        >
                            {BENCHMARK_TYPES.iter().map(|(val, label)| {
                                let is_selected = move || selected_bench_type.get() == *val;
                                view! {
                                    <option value=*val selected=is_selected>{*label}</option>
                                }.into_any()
                            }).collect::<Vec<_>>()}
                        </select>
                    </section>

                    // Model selection — two-step: model, then quant. Models can ship with
        // multiple quants (e.g. Q4_K_M vs Q6_K) and the delta matters for
        // benchmarking, so we make the quant an explicit choice.
        <section class="card">
            <h3>"Model"</h3>
            <div class="grid-2">
                <div class="form-group">
                    <label>"Model"</label>
                    <select
                        class="form-select"
                        on:change=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                            selected_display_name.set(val);
                        }
                    >
                        <option value="" disabled selected=move || selected_display_sig.get().is_empty()>"Select a model..."</option>
                        {move || {
                            let models = available_models_sig.get();
                            // Deduplicate by display_name; BTreeMap keeps them
                            // sorted alphabetically for stable rendering.
                            let mut grouped: BTreeMap<String, ()> = BTreeMap::new();
                            for (_, name, _) in models.iter() {
                                grouped.insert(name.clone(), ());
                            }
                            grouped.keys().map(|name| {
                                let value = name.clone();
                                let label = name.clone();
                                view! {
                                    <option value=value>{label}</option>
                                }.into_any()
                            }).collect::<Vec<_>>()
                        }}
                    </select>
                </div>
                <div class="form-group">
                    <label>"Quant"</label>
                    <select
                        class="form-select"
                        prop:disabled=move || selected_display_sig.get().is_empty()
                        on:change=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                            selected_model.set(val);
                        }
                    >
                        <option value="" disabled>"Select quant..."</option>
                        {move || {
                            let models = available_models_sig.get();
                            let dn = selected_display_sig.get();
                            let selected_id = selected_model_sig.get();
                            // Flatten all quants from matching model entries into individual options.
                            models.iter()
                                .filter(|(_, name, _)| name == &dn)
                                .flat_map(|(id, _, quants)| {
                                    quants.iter().map(move |quant| (id.clone(), quant.clone()))
                                })
                                .map(|(id_clone, quant)| {
                                    let value = format!("{}:{}", id_clone, quant);
                                    let is_selected = value == selected_id;
                                    view! {
                                        <option value=value selected=is_selected>{quant}</option>
                                    }.into_any()
                                }).collect::<Vec<_>>()
                        }}
                    </select>
                </div>
            </div>
        </section>

        // Backend selection (which llama-bench to use)
        <section class="card">
            <h3>"Backend"</h3>
            <select
                class="form-select"
                on:change=move |e| {
                    let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                    selected_backend.set(val);
                }
            >
                <option value="">"Auto (model's backend)"</option>
                {move || {
                    let backends = available_backends_sig.get();
                    backends.iter().map(|(name, display)| {
                        let name_clone = name.clone();
                        let display_clone = display.clone();
                        view! {
                            <option value=name_clone>{display_clone}</option>
                        }.into_any()
                    }).collect::<Vec<_>>()
                }}
            </select>
            <small class="bench-hint">
                "Select a specific backend's llama-bench, or leave empty to use the model's backend."
            </small>
        </section>

        // Test configuration
        <section class="card">
            <h3>"Test Configuration"</h3>
            <div class="grid-2">
                <div class="form-group">
                    <label>"Prompt sizes (tokens)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || pp_sizes_sig.get()
                        on:input=move |e| { pp_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 128,256,512"</small>
                </div>
                <div class="form-group">
                    <label>"Generation lengths (tokens)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || tg_sizes_sig.get()
                        on:input=move |e| { tg_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 32,64,128"</small>
                </div>
                <div class="form-group">
                    <label>"Runs"</label>
                    <input
                        type="number"
                        class="form-control"
                        prop:value=move || runs_sig.get()
                        min="1" max="20"
                        on:input=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value();
                            if let Ok(n) = val.parse::<u32>() { runs.set(n); }
                        }
                    />
                </div>
                <div class="form-group">
                    <label>"Warmup runs"</label>
                    <input
                        type="number"
                        class="form-control"
                        prop:value=move || warmup_sig.get()
                        min="0" max="10"
                        on:input=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value();
                            if let Ok(n) = val.parse::<u32>() { warmup.set(n); }
                        }
                    />
                </div>
                <div class="form-group">
                    <label>"Threads"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || threads_sig.get()
                        on:input=move |e| { threads_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"auto, or comma-separated e.g. 4,8,16"</small>
                </div>
                <div class="form-group">
                    <label>"GPU layers range (sweet spot)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || ngl_sig.get()
                        on:input=move |e| { ngl_range.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"e.g. 0-99+1 to sweep, or empty for all"</small>
                </div>
            </div>
        </section>

        // Advanced tuning — knobs from the LLM-inference-tuning methodology.
        // Each one is worth a full paragraph of explanation; the small text
        // below each field is the cheat-sheet version.
        <section class="card">
            <h3>"Advanced Tuning"</h3>
            <div class="grid-2">
                <div class="form-group">
                    <label>"Batch size (-b)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || batch_sig.get()
                        on:input=move |e| { batch_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Logical batch. Try 512,1024,2048 — can yield up to ~36% PP."</small>
                </div>
                <div class="form-group">
                    <label>"Micro-batch size (-ub)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || ubatch_sig.get()
                        on:input=move |e| { ubatch_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Physical micro-batch. Typically ≤ batch size."</small>
                </div>
                <div class="form-group">
                    <label>"KV cache type (-ctk/-ctv)"</label>
                    <select
                        class="form-select"
                        on:change=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                            kv_cache_type.set(val);
                        }
                    >
                        {move || {
                            let current = kv_sig.get();
                            vec!["default", "f16", "q8_0", "q4_0"].into_iter().map(|opt| {
                                let opt_str = opt.to_string();
                                let selected = opt == current;
                                let label = match opt {
                                    "default" => "Default (backend)",
                                    other => other,
                                };
                                view! {
                                    <option value=opt_str selected=selected>{label}</option>
                                }.into_any()
                            }).collect::<Vec<_>>()
                        }}
                    </select>
                    <small class="text-muted">"Applied to both K and V. Mismatched pair = CPU attention fallback."</small>
                </div>
                <div class="form-group">
                    <label>"Depth (-d)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || depth_sig.get()
                        on:input=move |e| { depth_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Pre-fill tokens before timing. e.g. 0,4096,16384 when testing KV quant."</small>
                </div>
                <div class="form-group">
                    <div class="form-check">
                        <input
                            id="bench-flash-attn"
                            type="checkbox"
                            prop:checked=move || fa_sig.get()
                            on:change=move |e| {
                                let checked = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().checked();
                                flash_attn.set(checked);
                            }
                        />
                        <label class="form-check-label" for="bench-flash-attn">"Flash attention (-fa)"</label>
                    </div>
                    <small class="text-muted">"Default on. Disable to measure attention-kernel impact."</small>
                </div>
            </div>
        </section>

        // Run button
        <div class="text-center my-3">
            <button
                class="btn btn-primary btn-lg"
                prop:disabled=move || selected_model_sig.get().is_empty() || is_running_sig.get()
                on:click=move |_| { submit_benchmark(); }
            >
                {move || if is_running_sig.get() { "Running..." } else { "▶ Run Benchmark" }}
            </button>
        </div>

        // Progress / logs — handled by JobLogPanel component.
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
                view! { <div></div> }.into_any()
            }
        }}

        // Benchmark results — single table with "t/s ± stddev" plus a header
        // card that surfaces the model metadata (backend, GPU, VRAM, load
        // time, batch/ubatch/KV choices) from the full BenchReport payload.
        {move || {
            let Some(report) = benchmark_results.get() else {
                return view! { <div></div> }.into_any();
            };

            // Accept either the full BenchReport shape or a bare summaries array
            // (legacy). Normalise to (summaries, model_info, vram, load_time, config).
            let (summaries, model_info, vram, load_time, config) = if let Some(arr) = report.as_array() {
                (arr.clone(), serde_json::Value::Null, serde_json::Value::Null, 0.0, serde_json::Value::Null)
            } else {
                let summaries = report.get("summaries")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let model_info = report.get("model_info").cloned().unwrap_or(serde_json::Value::Null);
                let vram = report.get("vram").cloned().unwrap_or(serde_json::Value::Null);
                let load_time = report.get("load_time_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let config = report.get("config").cloned().unwrap_or(serde_json::Value::Null);
                (summaries, model_info, vram, load_time, config)
            };

            if summaries.is_empty() {
                return view! { <div></div> }.into_any();
            }

            // Header card fields
            let mi_name = model_info.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mi_quant = model_info.get("quant").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mi_backend = model_info.get("backend").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mi_gpu = model_info.get("gpu_variant").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mi_ctx = model_info.get("context_length").and_then(|v| v.as_u64());
            let vram_used = vram.get("used_mib").and_then(|v| v.as_u64());
            let vram_total = vram.get("total_mib").and_then(|v| v.as_u64());
            let cfg_batch = config.get("batch_sizes").and_then(|v| v.as_array()).cloned();
            let cfg_ubatch = config.get("ubatch_sizes").and_then(|v| v.as_array()).cloned();
            let cfg_kv = config.get("kv_cache_type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let cfg_depth = config.get("depth").and_then(|v| v.as_array()).cloned();
            let cfg_fa = config.get("flash_attn").and_then(|v| v.as_bool());

            let has_header = !mi_name.is_empty();

            view! {
                <section class="card mt-3">
                    <h3>"Benchmark Results"</h3>

                    {if has_header {
                        view! {
                            <div class="bench-summary">
                                <div class="bench-summary__item">
                                    <div class="bench-summary__label">"Model"</div>
                                    <div class="bench-summary__value">{mi_name.clone()}</div>
                                </div>
                                {if !mi_quant.is_empty() {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"Quant"</div>
                                            <div class="bench-summary__value">{mi_quant}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                <div class="bench-summary__item">
                                    <div class="bench-summary__label">"Backend"</div>
                                    <div class="bench-summary__value">{format!("{} · {}", mi_backend, mi_gpu)}</div>
                                </div>
                                {if let (Some(used), Some(total)) = (vram_used, vram_total) {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"VRAM"</div>
                                            <div class="bench-summary__value">{format!("{} / {} MiB", used, total)}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(ctx) = mi_ctx {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"Context"</div>
                                            <div class="bench-summary__value">{ctx.to_string()}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                {if load_time > 0.0 {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"Load time"</div>
                                            <div class="bench-summary__value">{format!("{:.1} ms", load_time)}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(b) = cfg_batch.as_ref() {
                                    if !b.is_empty() {
                                        let s = b.iter().filter_map(|v| v.as_u64()).map(|v| v.to_string()).collect::<Vec<_>>().join(",");
                                        view! {
                                            <div class="bench-summary__item">
                                                <div class="bench-summary__label">"Batch"</div>
                                                <div class="bench-summary__value">{s}</div>
                                            </div>
                                        }.into_any()
                                    } else { view!{ <div></div> }.into_any() }
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(b) = cfg_ubatch.as_ref() {
                                    if !b.is_empty() {
                                        let s = b.iter().filter_map(|v| v.as_u64()).map(|v| v.to_string()).collect::<Vec<_>>().join(",");
                                        view! {
                                            <div class="bench-summary__item">
                                                <div class="bench-summary__label">"µ-batch"</div>
                                                <div class="bench-summary__value">{s}</div>
                                            </div>
                                        }.into_any()
                                    } else { view!{ <div></div> }.into_any() }
                                } else { view!{ <div></div> }.into_any() }}
                                {if !cfg_kv.is_empty() {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"KV cache"</div>
                                            <div class="bench-summary__value">{cfg_kv}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(b) = cfg_depth.as_ref() {
                                    if !b.is_empty() {
                                        let s = b.iter().filter_map(|v| v.as_u64()).map(|v| v.to_string()).collect::<Vec<_>>().join(",");
                                        view! {
                                            <div class="bench-summary__item">
                                                <div class="bench-summary__label">"Depth"</div>
                                                <div class="bench-summary__value">{s}</div>
                                            </div>
                                        }.into_any()
                                    } else { view!{ <div></div> }.into_any() }
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(fa) = cfg_fa {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"Flash attn"</div>
                                            <div class="bench-summary__value">{if fa { "on" } else { "off" }}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }}

                    {render_summaries_table(&summaries)}
                </section>
            }.into_any()
        }}
                    }.into_any() // close inner view!{} for else branch (llama-bench form)
                }
                }}    // closes tab conditional closure

        // History — always shown. Newest rows appear at the top because the
        // server returns ORDER BY created_at DESC.
        <section class="card mt-3">
            <h3>"Benchmark History"</h3>
            {move || {
                let entries = history_sig.get();
                if entries.is_empty() {
                    view! {
                        <p class="text-muted">"No benchmarks yet. Run one above to see results here."</p>
                    }.into_any()
                } else {
                    view! {
                        <table class="table table-striped">
                            <thead>
                                <tr>
                                    <th style="width:1.5rem"></th>
                                    <th>"When"</th>
                                    <th>"Model"</th>
                                    <th>"Type"</th>
                                    <th>"Engine"</th>
                                    <th>"Backend"</th>
                                    <th>"PP / TG sizes"</th>
                                    <th>"Best t/s"</th>
                                    <th>"Status"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {entries.into_iter().map(|entry| {
                                    let entry_id = entry.id;
                                    let when_title = format_timestamp(entry.created_at);
                                    let when_rel = format_relative(entry.created_at);
                                    let badge_class = if entry.status == "success" {
                                        "badge badge-success"
                                    } else {
                                        "badge badge-danger"
                                    };
                                    let name = entry.display_name.clone().unwrap_or_else(|| entry.model_id.clone());
                                    let quant_suffix = entry.quant
                                        .as_ref()
                                        .filter(|q| !q.is_empty())
                                        .map(|q| format!(" · {}", q))
                                        .unwrap_or_default();
                                    let model_cell = format!("{}{}", name, quant_suffix);

                                    let arr = entry.results.as_array();
                                    let best = |field: &str| -> String {
                                        arr.and_then(|items| {
                                            items.iter()
                                                .filter_map(|s| s.get(field).and_then(|v| v.as_f64()))
                                                .filter(|v| *v > 0.01)
                                                .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.max(v))))
                                        })
                                        .map(|v| format!("{v:.0}"))
                                        .unwrap_or_else(|| "—".to_string())
                                    };
                                    let best_cell = format!("PP {} · TG {}", best("pp_mean"), best("tg_mean"));

                                    let sizes = format!(
                                        "{} / {}",
                                        entry.pp_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
                                        entry.tg_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
                                    );

                                    // Reactive expansion state for this row. Leptos re-renders
                                    // just the chevron and the detail <tr> when the set flips.
                                    let is_open = Memo::new(move |_| expanded_sig.get().contains(&entry_id));
                                    let toggle = move |_| {
                                        expanded_history.update(|set| {
                                            if !set.insert(entry_id) { set.remove(&entry_id); }
                                        });
                                    };
                                    let summaries = entry.results.as_array().cloned().unwrap_or_default();
                                    let status_text = entry.status.clone();
                                    let backend_text = entry.backend.clone();
                                    let bench_type_text = entry.benchmark_type.clone().unwrap_or_default();
                                    let type_badge_class = match entry.benchmark_type.as_deref() {
                                        Some("baseline") => "badge badge-muted",
                                        Some("pp_sweep") => "badge badge-info",
                                        Some("kv_quant_q8") | Some("kv_quant_q4") => "badge badge-success",
                                        Some("context_test") => "badge badge-warning",
                                        Some("spec_scan") | Some("spec_sweep") => "badge badge-danger",
                                        _ => "badge badge-muted",
                                    };
                                    let engine_text = entry
                                        .engine
                                        .clone()
                                        .unwrap_or_else(|| "llama_bench".to_string());
                                    let when_title_for_row = when_title.clone();

                                    // Engine badge — distinguishes llama-bench from spec-decode and MTP runs
                                    let engine_badge = if engine_text == "llama_cli_spec" || engine_text == "llama_cli_mtp" {
                                        "badge badge-info".to_string()
                                    } else {
                                        "badge badge-muted".to_string()
                                    };

                                    view! {
                                        <tr class="bench-history__row" on:click=toggle>
                                            <td class="text-mono text-muted">{move || if is_open.get() { "▾" } else { "▸" }}</td>
                                            <td title=when_title_for_row>{when_rel}</td>
                                            <td>{model_cell}</td>
                                            <td><span class={type_badge_class}>{bench_type_text}</span></td>
                                            <td><span class={engine_badge}>{engine_text.clone()}</span></td>
                                            <td><span class="badge badge-muted">{backend_text}</span></td>
                                            <td class="text-mono">{sizes}</td>
                                            <td class="text-mono">{best_cell}</td>
                                            <td><span class={badge_class}>{status_text}</span></td>
                                        </tr>
                                        {move || is_open.get().then(|| {
                                            let detail_table = match engine_text.as_str() {
                                                "llama_cli_spec" => render_spec_table(&summaries).into_any(),
                                                "llama_cli_mtp" => render_mtp_table(&summaries).into_any(),
                                                _ => render_summaries_table(&summaries).into_any(),
                                            };
                                            view! {
                                                <tr class="bench-history__detail">
                                                    <td></td>
                                                    <td colspan="8">{detail_table}</td>
                                                </tr>
                                            }
                                        })}
                                    }.into_any()
                                }).collect::<Vec<_>>()}
                            </tbody>
                        </table>
                    }.into_any()
                }
            }}
        </section>
    }
}
