//! Benchmarks page — run llama-bench and spec-decoding benchmarks from the web UI.

mod llama_bench;
mod mtp_bench;
mod selectors;
mod spec_bench;
pub mod types;
mod utils;

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;

use self::llama_bench::LlamaBench;
use self::mtp_bench::MtpBench;
use self::spec_bench::SpecBench;
use self::types::BenchmarkHistoryEntry;
use self::utils::{
    fetch_installed_backend_variants, format_mean_stddev, format_relative, format_timestamp,
    use_benchmark_form_state, BenchmarkFormState,
};
use crate::components::tab_buttons::{TabButton, TabButtons};
use crate::utils::{extract_and_store_csrf_token, get_request};

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
        selected_display_name: _,
        selected_model: _,
        available_models: _,
        selected_backend: _,
        available_backends,
        is_running: _,
        current_job_id: _,
        benchmark_results: _,
        model_n_batch: _,
        model_n_ubatch: _,
    } = state;

    // Fetch installed backend variants for llama-bench selection.
    fetch_installed_backend_variants(available_backends);

    // History state — always visible.
    let history = RwSignal::new(Vec::<BenchmarkHistoryEntry>::new());
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
                if let Ok(entries) = resp.json::<Vec<BenchmarkHistoryEntry>>().await {
                    history.set(entries);
                }
            }
        });
    });

    // ── Read-only splits for shared view (history) ───────────────
    let (history_sig, _) = history.split();
    let (expanded_sig, _) = expanded_history.split();

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

        // LLaMA-Bench tab content (shared form state hoisted from parent)
        {move || {
            if active_tab.get() == "mtp-testing" {
                view! { <MtpBench history_refresh_trigger=history_refresh shared_state=state.clone() /> }.into_any()
            } else if active_tab.get() == "spec-decode" {
                view! { <SpecBench history_refresh_trigger=history_refresh shared_state=state.clone() /> }.into_any()
            } else {
                view! { <LlamaBench history_refresh_trigger=history_refresh shared_state=state.clone() /> }.into_any()
            }
        }}

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
                                    let badge_class = match entry.status.as_str() {
                                        "success" => "badge badge-success",
                                        "partial" => "badge badge-warning",
                                        _ => "badge badge-danger",
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
