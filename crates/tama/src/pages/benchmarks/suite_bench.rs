//! Benchmark suite form — runs llama_bench + spec (+ MTP) sequentially.
//!
//! Uses capability-driven checkboxes to auto-select benchmark types based on
//! the selected model's capabilities.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::job_log_panel::JobLogPanel;
use crate::pages::benchmarks::selectors::{BackendSelect, ModelQuantSelect};
use crate::pages::benchmarks::utils::{
    parse_sizes, parse_threads, split_id_quant, split_name_variant, submit_bench_job,
    BenchmarkFormState,
};
use crate::utils::target_value;

/// Draft-MTP is the primary MTP indicator (ngram-simple is spec decoding, not MTP).
const DRAFT_MTP_ID: &str = "draft-mtp";

#[component]
pub fn SuiteBench(
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

    // ── Benchmark type checkboxes ──────────────────────────────────────
    // llama_bench and spec are always enabled. MTP types are auto-ticked when
    // the selected model supports MTP, disabled with tooltip otherwise.
    let all_bench_types: RwSignal<Vec<String>> =
        RwSignal::new(vec!["llama-bench".to_string(), "spec".to_string()]);

    // ── Advanced overrides (collapsed by default) ───────────────────────
    let advanced_open = RwSignal::new(false);
    let pp_sizes_str = RwSignal::new("2048".to_string());
    let tg_sizes_str = RwSignal::new("128".to_string());
    let runs = RwSignal::new(3u32);
    let warmup = RwSignal::new(1u32);
    let threads_str = RwSignal::new("auto".to_string());
    let batch_sizes_str = RwSignal::new("".to_string());
    let ubatch_sizes_str = RwSignal::new("".to_string());
    let kv_cache_type = RwSignal::new("default".to_string());
    let depth_str = RwSignal::new("".to_string());
    let flash_attn = RwSignal::new(true);

    // ── Per-form state ─────────────────────────────────────────────────
    let error_msg = RwSignal::new(String::new());

    // ── MTP type checkboxes (derived from capabilities) ────────────────
    let mtp_types: RwSignal<Vec<String>> = RwSignal::new(vec![DRAFT_MTP_ID.to_string()]);

    // Whether the currently selected model supports MTP (drives checkbox disabled state).
    let supports_mtp = RwSignal::new(false);

    // When the selected model changes, update MTP checkbox state based on
    // its capabilities. Also update batch/ubatch from model defaults.
    Effect::new(move |_| {
        let dn = selected_display_name.get();
        let models = available_models.get();
        if let Some((_, _, _, n_batch, n_ubatch, cap_mtp)) =
            models.iter().find(|(_, name, _, _, _, _)| name == &dn)
        {
            // Track MTP capability for the checkbox disabled state.
            supports_mtp.set(*cap_mtp);
            // Auto-tick MTP types when the model supports them.
            mtp_types.set(if *cap_mtp {
                vec![DRAFT_MTP_ID.to_string()]
            } else {
                vec![]
            });
            // Add to all_bench_types if any MTP types are checked.
            let has_mtp = !mtp_types.get().is_empty();
            let mut types = vec!["llama-bench".to_string(), "spec".to_string()];
            if has_mtp {
                types.push("mtp".to_string());
            }
            all_bench_types.set(types);

            // Prefill batch/ubatch from model defaults.
            if let Some(nb) = n_batch {
                batch_sizes_str.set(nb.to_string());
            }
            if let Some(nub) = n_ubatch {
                ubatch_sizes_str.set(nub.to_string());
            }
        } else {
            supports_mtp.set(false);
            mtp_types.set(vec![]);
            all_bench_types.set(vec!["llama-bench".to_string(), "spec".to_string()]);
        }
    });

    // ── Submit handler ─────────────────────────────────────────────────
    let submit_suite = move || {
        let raw_model = selected_model.get();
        if raw_model.is_empty() {
            return;
        }
        let (model_id, quant) = split_id_quant(&raw_model);

        // Parse "name:variant" format from selected backend.
        let raw_backend = selected_backend.get();
        let (backend_name, gpu_variant) = split_name_variant(&raw_backend);

        // Collect benchmark types to run.
        let types = all_bench_types.get();
        if types.is_empty() {
            error_msg.set("No benchmark types selected.".to_string());
            return;
        }

        is_running.set(true);
        current_job_id.set(None);
        error_msg.set(String::new());

        // Capture override values before entering async block.
        let pp_sizes_val = pp_sizes_str.get();
        let tg_sizes_val = tg_sizes_str.get();
        let threads_val = threads_str.get();
        let batch_sizes_val = batch_sizes_str.get();
        let ubatch_sizes_val = ubatch_sizes_str.get();
        let kv_cache_val = kv_cache_type.get();
        let depth_val = depth_str.get();
        let flash_attn_val = flash_attn.get();
        let runs_val = runs.get();
        let warmup_val = warmup.get();

        spawn_local(async move {
            // Parse override strings into typed values for the request.
            let pp_sizes = parse_sizes(&pp_sizes_val);
            let tg_sizes = parse_sizes(&tg_sizes_val);
            let threads = parse_threads(&threads_val);
            let batch_sizes = parse_sizes(&batch_sizes_val);
            let ubatch_sizes = parse_sizes(&ubatch_sizes_val);
            let depth = parse_sizes(&depth_val);

            // Build body — only include non-default overrides so the backend
            // can distinguish "explicitly set" from "use default".
            let mut body = serde_json::json!({
                "model_id": model_id,
                "quant": quant,
                "backend_name": backend_name,
                "gpu_variant": gpu_variant,
                "types": types,
            });

            // Add override fields when they differ from defaults.
            if !pp_sizes.is_empty() && pp_sizes != vec![2048] {
                body["pp_sizes"] = serde_json::json!(pp_sizes);
            }
            if !tg_sizes.is_empty() && tg_sizes != vec![128] {
                body["tg_sizes"] = serde_json::json!(tg_sizes);
            }
            if runs_val != 3 {
                body["runs"] = runs_val.into();
            }
            if warmup_val != 1 {
                body["warmup"] = warmup_val.into();
            }
            if let Some(t) = threads {
                if !t.is_empty() {
                    body["threads"] = serde_json::json!(t);
                }
            }
            if !batch_sizes.is_empty() {
                body["batch_sizes"] = serde_json::json!(batch_sizes);
            }
            if !ubatch_sizes.is_empty() {
                body["ubatch_sizes"] = serde_json::json!(ubatch_sizes);
            }
            if kv_cache_val != "default" {
                body["kv_cache_type"] = kv_cache_val.into();
            }
            if !depth.is_empty() {
                body["depth"] = serde_json::json!(depth);
            }
            if !flash_attn_val {
                body["flash_attn"] = false.into();
            }

            match submit_bench_job("/tama/v1/benchmarks/suite", body).await {
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
    let (all_bench_types_sig, _) = all_bench_types.split();
    let (advanced_open_sig, _) = advanced_open.split();
    let (pp_sizes_sig, _) = pp_sizes_str.split();
    let (tg_sizes_sig, _) = tg_sizes_str.split();
    let (runs_sig, _) = runs.split();
    let (warmup_sig, _) = warmup.split();
    let (threads_sig, _) = threads_str.split();
    let (batch_sig, _) = batch_sizes_str.split();
    let (ubatch_sig, _) = ubatch_sizes_str.split();
    let (kv_sig, _) = kv_cache_type.split();
    let (depth_sig, _) = depth_str.split();
    let (fa_sig, _) = flash_attn.split();
    let (is_running_sig, _) = is_running.split();
    let (current_job_id_sig, _) = current_job_id.split();
    let (error_sig, _) = error_msg.split();

    view! {
        <div>
            // ── Model selection ───────────────────────────────────────
            <section class="card">
                <h3>"Model"</h3>
                <ModelQuantSelect
                    models=available_models_sig
                    selected_model=selected_display_name
                    selected_quant=selected_model
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

            // ── Benchmark types (capability-driven) ───────────────────
            <section class="card">
                <h3>"Benchmark Types"</h3>
                <small class="bench-hint">
                    "llama_bench and spec are always included. MTP types are auto-selected when the model supports multi-token prediction."
                </small>
                {move || {
                    let types = all_bench_types_sig.get();
                    let mtp_supported = supports_mtp.get();
                    vec!["llama_bench", "spec", "mtp"].into_iter().map(|typ| {
                        let is_checked = types.contains(&typ.to_string());
                        let is_mtp = typ == "mtp";
                        // Disable MTP checkbox only when model doesn't support it,
                        // not based on checked state — avoids permanent disable after unchecking.
                        let disabled = is_mtp && !mtp_supported;
                        let label_text = match typ {
                            "llama_bench" => "LLaMA-Bench (baseline performance)",
                            "spec" => "Speculative Decoding (draft-mtp + ngram types)",
                            "mtp" => "MTP (Multi-Token Prediction)",
                            _ => typ,
                        };
                        view! {
                            <div class="form-check">
                                <input
                                    type="checkbox"
                                    id=format!("suite-type-{}", typ)
                                    prop:checked=is_checked
                                    prop:disabled=disabled
                                    on:change=move |e| {
                                        let checked = event_target_checked(&e);
                                        all_bench_types.update(|t| {
                                            if checked {
                                                if !t.contains(&typ.to_string()) {
                                                    t.push(typ.to_string());
                                                }
                                            } else {
                                                t.retain(|x| x != typ);
                                            }
                                        });
                                    }
                                />
                                <label class="form-check-label" for=format!("suite-type-{}", typ)>
                                    {label_text}
                                </label>
                                {if is_mtp && !mtp_supported {
                                    view! {
                                        <small class="text-muted">" (model does not support MTP)"</small>
                                    }.into_any()
                                } else {
                                    view! { <span/> }.into_any()
                                }}
                            </div>
                        }.into_any()
                    }).collect::<Vec<_>>()
                }}
            </section>

            // ── Advanced overrides (collapsed) ────────────────────────
            <section class="card">
                <h3
                    class="suite-advanced__header"
                    on:click=move |_| {
                        advanced_open.update(|v| *v = !*v);
                    }
                >
                    {move || if advanced_open_sig.get() { "▾ " } else { "▸ " }}
                    "Advanced Overrides"
                </h3>
                <Show when=move || advanced_open_sig.get()>
                    <div class="suite-advanced__content">
                        <small class="bench-hint">
                            "Override suite defaults. Leave empty to use model/config defaults."
                        </small>
                        <div class="grid-2">
                            <div class="form-group">
                                <label>"Prompt sizes (tokens)"</label>
                                <input
                                    type="text"
                                    class="form-control"
                                    prop:value=move || pp_sizes_sig.get()
                                    on:input=move |e| { pp_sizes_str.set(target_value(&e)); }
                                />
                                <small class="text-muted">"Default: 2048. Comma-separated."</small>
                            </div>
                            <div class="form-group">
                                <label>"Generation lengths (tokens)"</label>
                                <input
                                    type="text"
                                    class="form-control"
                                    prop:value=move || tg_sizes_sig.get()
                                    on:input=move |e| { tg_sizes_str.set(target_value(&e)); }
                                />
                                <small class="text-muted">"Default: 128. Comma-separated."</small>
                            </div>
                            <div class="form-group">
                                <label>"Runs per type"</label>
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
                                <small class="text-muted">"Default: auto. Comma-separated e.g. 4,8,16"</small>
                            </div>
                            <div class="form-group">
                                <label>"Batch size (-b)"</label>
                                <input
                                    type="text"
                                    class="form-control"
                                    prop:value=move || batch_sig.get()
                                    on:input=move |e| { batch_sizes_str.set(target_value(&e)); }
                                />
                                <small class="text-muted">"Default: from model config. Comma-separated."</small>
                            </div>
                            <div class="form-group">
                                <label>"Micro-batch size (-ub)"</label>
                                <input
                                    type="text"
                                    class="form-control"
                                    prop:value=move || ubatch_sig.get()
                                    on:input=move |e| { ubatch_sizes_str.set(target_value(&e)); }
                                />
                                <small class="text-muted">"Default: from model config. Comma-separated."</small>
                            </div>
                            <div class="form-group">
                                <label>"KV cache type"</label>
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
                            </div>
                            <div class="form-group">
                                <label>"Depth (-d)"</label>
                                <input
                                    type="text"
                                    class="form-control"
                                    prop:value=move || depth_sig.get()
                                    on:input=move |e| { depth_str.set(target_value(&e)); }
                                />
                                <small class="text-muted">"Pre-fill tokens. Comma-separated."</small>
                            </div>
                            <div class="form-group">
                                <div class="form-check">
                                    <input
                                        id="suite-flash-attn"
                                        type="checkbox"
                                        prop:checked=move || fa_sig.get()
                                        on:change=move |e| {
                                            flash_attn.set(event_target_checked(&e));
                                        }
                                    />
                                    <label class="form-check-label" for="suite-flash-attn">"Flash attention"</label>
                                </div>
                            </div>
                        </div>
                    </div>
                </Show>
            </section>

            // ── Run button ────────────────────────────────────────────
            <div class="text-center my-3">
                <button
                    class="btn btn-primary btn-lg"
                    prop:disabled=move || selected_model_sig.get().is_empty() || is_running_sig.get()
                    on:click=move |_| { submit_suite(); }
                >
                    {move || if is_running_sig.get() { "Running Suite..." } else { "▶ Run Suite" }}
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
                    view! { <div></div> }.into_any()
                }
            }}

            // ── Progress / logs ───────────────────────────────────────
            {move || {
                if let Some(job_id) = current_job_id_sig.get() {
                    view! {
                        <JobLogPanel
                            job_id=job_id
                            on_status=on_status_cb
                        />
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

        </div>
    }
}
