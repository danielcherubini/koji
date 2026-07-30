//! Utility functions for the benchmarks page.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::types;
use crate::utils::{extract_and_store_csrf_token, get_request, post_request};

/// Parse a comma-separated string of integers into a Vec<u32>.
/// Zero is a meaningful value — `-p 0` pins llama-bench to pure-TG mode.
pub fn parse_sizes(s: &str) -> Vec<u32> {
    s.split(',')
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .filter_map(|v| v.parse::<u32>().ok())
        .collect()
}

/// Render "mean ± stddev" with one decimal place, or a single value when
/// stddev rounds to zero.
pub fn format_mean_stddev(mean: f64, stddev: f64) -> String {
    if stddev > 0.05 {
        format!("{:.1} ± {:.1}", mean, stddev)
    } else {
        format!("{:.1}", mean)
    }
}

/// Split "id:quant" composite into (model_id, quant).
/// No colon → (whole string, None).
pub fn split_id_quant(raw: &str) -> (String, Option<String>) {
    if let Some(colon) = raw.find(':') {
        (raw[..colon].to_string(), Some(raw[colon + 1..].to_string()))
    } else {
        (raw.to_string(), None)
    }
}

/// Split "name:variant" composite into (name, variant).
/// Empty → (None, None); no colon → (Some(whole), None).
pub fn split_name_variant(raw: &str) -> (Option<String>, Option<String>) {
    if raw.is_empty() {
        (None, None)
    } else if let Some((name, variant)) = raw.split_once(':') {
        (Some(name.to_string()), Some(variant.to_string()))
    } else {
        (Some(raw.to_string()), None)
    }
}

/// Shared reactive state for benchmark forms.
#[derive(Clone)]
pub struct BenchmarkFormState {
    pub selected_display_name: RwSignal<String>,
    pub selected_model: RwSignal<String>,
    pub available_models: RwSignal<Vec<types::ModelListItem>>,
    pub selected_backend: RwSignal<String>,
    pub available_backends: RwSignal<Vec<(String, String)>>,
    pub is_running: RwSignal<bool>,
    pub current_job_id: RwSignal<Option<String>>,
    pub benchmark_results: RwSignal<Option<serde_json::Value>>,
    /// Trigger for refetching models. Incremented to force a refresh.
    #[expect(dead_code)]
    pub model_refresh: RwSignal<u32>,
    /// Prefilled batch size from the selected model's n_batch (if set).
    pub model_n_batch: RwSignal<Option<u32>>,
    /// Prefilled micro-batch size from the selected model's n_ubatch (if set).
    pub model_n_ubatch: RwSignal<Option<u32>>,
}

/// Create shared benchmark form state with universal Effects:
/// - fetch-models-on-refresh
/// - auto-select-first-quant-on-display-name-change
pub fn use_benchmark_form_state() -> BenchmarkFormState {
    let selected_display_name = RwSignal::new(String::new());
    let selected_model = RwSignal::new(String::new());
    let available_models = RwSignal::new(Vec::<types::ModelListItem>::new());
    let model_n_batch = RwSignal::new(None);
    let model_n_ubatch = RwSignal::new(None);
    let selected_backend = RwSignal::new(String::new());
    let available_backends = RwSignal::new(Vec::<(String, String)>::new());
    let is_running = RwSignal::new(false);
    let current_job_id = RwSignal::new(Option::<String>::None);
    let benchmark_results = RwSignal::new(Option::<serde_json::Value>::None);
    let model_refresh = RwSignal::new(0u32);

    // Fetch available models on mount.
    Effect::new(move |_| {
        let _ = model_refresh.get();
        spawn_local(async move {
            if let Ok(resp) = get_request("/tama/v1/models").send().await {
                extract_and_store_csrf_token(&resp);
                if let Ok(root) = resp.json::<serde_json::Value>().await {
                    if let Some(models_arr) = root.get("models").and_then(|v| v.as_array()) {
                        // Flatten parse_model results (one tuple per quant) and deduplicate
                        // by (display_name, quant) keeping the first id for each unique pair.
                        let mut seen: std::collections::HashSet<(String, String)> =
                            std::collections::HashSet::new();
                        let model_list: Vec<types::ModelListItem> = models_arr
                            .iter()
                            .filter_map(super::types::parse_model)
                            .flatten()
                            .filter(|(_, name, quant, _, _)| {
                                seen.insert((name.clone(), quant.clone()))
                            })
                            .map(|(id, name, quant, n_batch, n_ubatch)| {
                                (id, name, vec![quant], n_batch, n_ubatch)
                            })
                            .collect();
                        available_models.update(|list| *list = model_list);
                    }
                }
            }
        });
    });

    // When the display_name changes, auto-select the first quant so the id is
    // always populated. Value format is "id:quant".
    Effect::new(move |_| {
        let dn = selected_display_name.get();
        let models = available_models.get();
        if let Some((id, _, quants, n_batch, n_ubatch)) =
            models.iter().find(|(_, name, _, _, _)| name == &dn)
        {
            if let Some(first_quant) = quants.first() {
                selected_model.set(format!("{}:{}", id, first_quant));
            } else {
                selected_model.set(id.clone());
            }
            // Prefill batch/ubatch inputs from model defaults (only when set).
            model_n_batch.set(*n_batch);
            model_n_ubatch.set(*n_ubatch);
        } else {
            selected_model.set(String::new());
            model_n_batch.set(None);
            model_n_ubatch.set(None);
        }
    });

    BenchmarkFormState {
        selected_display_name,
        selected_model,
        available_models,
        selected_backend,
        available_backends,
        is_running,
        current_job_id,
        benchmark_results,
        model_refresh,
        model_n_batch,
        model_n_ubatch,
    }
}

/// Fetch installed backend variants for spec/mtp forms.
/// Reads both root["backends"] and root["custom"], installed-only,
/// value format is "name:variant".
pub fn fetch_installed_backend_variants(available_backends: RwSignal<Vec<(String, String)>>) {
    spawn_local(async move {
        if let Ok(resp) = get_request("/tama/v1/backends").send().await {
            extract_and_store_csrf_token(&resp);
            if let Ok(root) = resp.json::<serde_json::Value>().await {
                // /v1/backends returns { backends: [BackendCardDto], custom: [BackendCardDto] }
                // BackendCardDto has: type, display_name, gpu_variant, installed
                let mut backend_list: Vec<(String, String)> = Vec::new();
                for arr_key in ["backends", "custom"] {
                    if let Some(arr) = root.get(arr_key).and_then(|v| v.as_array()) {
                        for b in arr {
                            let installed = b
                                .get("installed")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            if !installed {
                                continue;
                            }
                            let name = b
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let display = b
                                .get("display_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or(&name)
                                .to_string();
                            let variant = b
                                .get("gpu_variant")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !variant.is_empty() {
                                let value = format!("{}:{}", name, variant);
                                let label = if variant == "cpu" {
                                    display
                                } else {
                                    format!("{} ({})", display, variant)
                                };
                                backend_list.push((value, label));
                            }
                        }
                    }
                }
                available_backends.update(|list| *list = backend_list);
            }
        }
    });
}

/// Format a Unix timestamp (seconds since epoch) as a local-time
/// "YYYY-MM-DD HH:MM" string using `js_sys::Date`.
///
/// Previously this rebuilt the date manually with
/// `Date::new_with_year_month_day`, which always yields midnight local — the
/// hour/minute fields came out as `00:00` regardless of the input timestamp.
/// We now construct the `Date` from the full ms-since-epoch so `getHours` /
/// `getMinutes` reflect the actual moment the benchmark ran.
///
/// Note: `js_sys::Date::get_month()` returns 0-indexed months (0=Jan), hence
/// the `+1` adjustment below.
pub fn format_timestamp(ts: i64) -> String {
    let ms = wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0);
    let date = js_sys::Date::new(&ms);
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
    )
}

/// Submit a benchmark job via POST and return the job_id.
///
/// Posts `body` as JSON to `url`. On HTTP status >= 400 returns the response
/// text as an error. On success parses `job_id` from the JSON body.
pub async fn submit_bench_job(url: &str, body: serde_json::Value) -> Result<String, String> {
    let resp = post_request(url)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .map_err(|e| format!("Request build failed: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if resp.status() >= 400 {
        let err_text = resp
            .text()
            .await
            .unwrap_or_else(|_| format!("Request failed with status {}", resp.status()));
        return Err(err_text);
    }

    let json = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    json.get("job_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Response missing job_id".to_string())
}

/// Format a Unix timestamp as a short relative "time ago" string (e.g. "5m
/// ago", "2h ago", "3d ago"). Falls back to the absolute format for anything
/// older than a week.
pub fn format_relative(ts: i64) -> String {
    let now_ms = js_sys::Date::now();
    let then_ms = ts as f64 * 1000.0;
    let delta_ms = (now_ms - then_ms).max(0.0);
    let secs = (delta_ms / 1000.0) as i64;
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 7 * 86_400 {
        format!("{}d ago", secs / 86_400)
    } else {
        format_timestamp(ts)
    }
}
