//! LLaMA-Bench form and results display.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::job_log_panel::JobLogPanel;
use crate::pages::benchmarks::render_summaries_table;
use crate::pages::benchmarks::selectors::{BackendSelect, ModelQuantSelect};
use crate::pages::benchmarks::types::{BENCHMARK_TYPES, LLAMA_BENCH_PRESETS};
use crate::pages::benchmarks::utils::{
    parse_sizes, parse_threads, split_id_quant, split_name_variant, submit_bench_job, target_bool,
    BenchmarkFormState,
};
use crate::utils::target_value;

#[component]
pub fn LlamaBench(
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
        model_n_batch,
        model_n_ubatch,
    } = shared_state;

    // ── Per-tab job state (isolated from other tabs) ───────────────────
    let is_running = RwSignal::new(false);
    let current_job_id = RwSignal::new(Option::<String>::None);
    let benchmark_results = RwSignal::new(Option::<serde_json::Value>::None);

    // ── Test configuration ─────────────────────────────────────────────
    let selected_bench_type = RwSignal::new("baseline".to_string());

    let pp_sizes_str = RwSignal::new("2048".to_string());
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

    // When the model's n_batch/n_ubatch changes, prefill the batch/ubatch inputs
    // only when the model has values set (respecting that preset/model defaults
    // may already have populated these fields).
    Effect::new(move |_| {
        if let Some(n_batch) = model_n_batch.get() {
            batch_sizes_str.set(n_batch.to_string());
        }
        if let Some(n_ubatch) = model_n_ubatch.get() {
            ubatch_sizes_str.set(n_ubatch.to_string());
        }
    });

    // ── Per-form state ─────────────────────────────────────────────────
    let error_msg = RwSignal::new(String::new());

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

    // ── Submit handler ─────────────────────────────────────────────────
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
        error_msg.set(String::new());

        spawn_local(async move {
            // Parse "name:variant" format from selected backend.
            let raw_backend = selected_backend.get();
            let (backend_name, gpu_variant) = split_name_variant(&raw_backend);

            let body = serde_json::json!({
                "model_id": model_id,
                "quant": quant,
                "backend_name": backend_name,
                "gpu_variant": gpu_variant,
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

            match submit_bench_job("/tama/v1/benchmarks/run", body).await {
                Ok(job_id) => {
                    current_job_id.set(Some(job_id));
                }
                Err(err) => {
                    // Submission failed — roll back is_running and show error.
                    is_running.set(false);
                    error_msg.set(err);
                }
            }
        });
    };

    // ── SSE callbacks ──────────────────────────────────────────────────
    let on_result_cb = Callback::new(move |results_json: String| {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&results_json) {
            benchmark_results.set(Some(parsed));
        }
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
    let (ctx_sig, _) = ctx_override.split();
    let (is_running_sig, _) = is_running.split();
    let (current_job_id_sig, _) = current_job_id.split();
    let (error_sig, _) = error_msg.split();
    let (benchmark_results_sig, _) = benchmark_results.split();

    view! {
        <div>
            // ── Test Type dropdown ──────────────────────────────────────
            <section class="card">
                <h3>"Test Type"</h3>
                <select
                    class="form-select"
                    on:change=move |e| {
                        let val = target_value(&e);
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

            // ── Model selection ─────────────────────────────────────────
            <section class="card">
                <h3>"Model"</h3>
                <ModelQuantSelect
                    models=available_models_sig
                    selected_model=selected_display_name
                    selected_quant=selected_model
                />
            </section>

            // ── Backend selection ───────────────────────────────────────
            <section class="card">
                <h3>"Backend"</h3>
                <BackendSelect
                    backends=available_backends_sig
                    selected_backend=selected_backend
                    hint_text="Select a specific backend's llama-bench, or leave empty to use the model's backend."
                />
            </section>

            // ── Test configuration ──────────────────────────────────────
            <section class="card">
                <h3>"Test Configuration"</h3>
                <div class="grid-2">
                    <div class="form-group">
                        <label>"Prompt sizes (tokens)"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || pp_sizes_sig.get()
                            on:input=move |e| { pp_sizes_str.set(target_value(&e)); }
                        />
                        <small class="text-muted">"Comma-separated, e.g. 128,256,512"</small>
                    </div>
                    <div class="form-group">
                        <label>"Generation lengths (tokens)"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || tg_sizes_sig.get()
                            on:input=move |e| { tg_sizes_str.set(target_value(&e)); }
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
                                let val = target_value(&e);
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
                                let val = target_value(&e);
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
                            on:input=move |e| { threads_str.set(target_value(&e)); }
                        />
                        <small class="text-muted">"auto, or comma-separated e.g. 4,8,16"</small>
                    </div>
                    <div class="form-group">
                        <label>"GPU layers range (sweet spot)"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || ngl_sig.get()
                            on:input=move |e| { ngl_range.set(target_value(&e)); }
                        />
                        <small class="text-muted">"e.g. 0-99+1 to sweep, or empty for all"</small>
                    </div>
                    <div class="form-group">
                        <label>"Context size override"</label>
                        <input
                            type="number"
                            class="form-control"
                            prop:value=move || ctx_sig.get()
                            on:input=move |e| { ctx_override.set(target_value(&e)); }
                        />
                        <small class="text-muted">"Override the model's context length (tokens). Leave empty to use default."</small>
                    </div>
                </div>
            </section>

            // ── Advanced tuning ─────────────────────────────────────────
            <section class="card">
                <h3>"Advanced Tuning"</h3>
                <div class="grid-2">
                    <div class="form-group">
                        <label>"Batch size (-b)"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || batch_sig.get()
                            on:input=move |e| { batch_sizes_str.set(target_value(&e)); }
                        />
                        <small class="text-muted">"Logical batch. Try 512,1024,2048 — can yield up to ~36% PP."</small>
                    </div>
                    <div class="form-group">
                        <label>"Micro-batch size (-ub)"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || ubatch_sig.get()
                            on:input=move |e| { ubatch_sizes_str.set(target_value(&e)); }
                        />
                        <small class="text-muted">"Physical micro-batch. Typically ≤ batch size."</small>
                    </div>
                    <div class="form-group">
                        <label>"KV cache type (-ctk/-ctv)"</label>
                        <select
                            class="form-select"
                            on:change=move |e| {
                                let val = target_value(&e);
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
                            on:input=move |e| { depth_str.set(target_value(&e)); }
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
                                    let checked = target_bool(&e);
                                    flash_attn.set(checked);
                                }
                            />
                            <label class="form-check-label" for="bench-flash-attn">"Flash attention (-fa)"</label>
                        </div>
                        <small class="text-muted">"Default on. Disable to measure attention-kernel impact."</small>
                    </div>
                </div>
            </section>

            // ── Run button ──────────────────────────────────────────────
            <div class="text-center my-3">
                <button
                    class="btn btn-primary btn-lg"
                    prop:disabled=move || selected_model_sig.get().is_empty() || is_running_sig.get()
                    on:click=move |_| { submit_benchmark(); }
                >
                    {move || if is_running_sig.get() { "Running..." } else { "▶ Run Benchmark" }}
                </button>
            </div>

            // ── Error display ───────────────────────────────────────────
            {move || {
                let err = error_sig.get();
                if !err.is_empty() {
                    view! {
                        <div class="alert alert-danger mt-2">
                            <p class="mb-0">{err}</p>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // ── Progress / logs ─────────────────────────────────────────
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

            // ── Results display ─────────────────────────────────────────
            {move || {
                let Some(report) = benchmark_results_sig.get() else {
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
        </div>
    }
}
