//! Client-side filter toolbar logic for the Models page: search, state
//! pills, sort, and group-by. Everything operates on the already-fetched
//! `data.models` — nothing here triggers a refetch.
//!
//! Pipeline order is fixed: search → state pill → sort → group-by
//! (see [`apply_pipeline`] and [`group_survivors`]).
//!
//! The sort/group helpers are a port of the #136 toolbar logic that
//! briefly lived on the dashboard (commit `c7decf9a`); that copy was
//! deleted during the plan-192/193 dashboard refactor and these are its
//! only surviving expressions.

use super::ModelEntry;
use crate::components::model_card::model_status_badge_label;
use crate::core_mirrors::ModelState;

/// Sort criteria (restored from #136).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SortBy {
    #[default]
    Name,
    Status,
    Gpu,
    Family,
    Vendor,
}

/// Optional group-by (restored from #136).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GroupBy {
    Gpu,
    Family,
    Vendor,
    Status,
}

/// Single-select view filter (state pills on the toolbar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ViewFilter {
    #[default]
    All,
    Loaded,
    Idle,
    Failed,
    Disabled,
}

/// Extract trailing numeric index from a GPU device string (e.g. "CUDA10" → 10).
fn extract_gpu_index(device: &str) -> Option<u32> {
    let mut digits = String::new();
    for c in device.chars().rev() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            break;
        }
    }
    if digits.is_empty() {
        None
    } else {
        let num_str = digits.chars().rev().collect::<String>();
        num_str.parse::<u32>().ok()
    }
}

/// True when `query` (trimmed) is empty, or when its case-insensitive form
/// appears anywhere in the entry's display name, api name, model+repo id,
/// or quant. `None` fields simply don't match.
pub(super) fn matches_search(m: &ModelEntry, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    [
        m.display_name.as_deref(),
        m.api_name.as_deref(),
        m.model.as_deref(),
        m.quant.as_deref(),
    ]
    .iter()
    .flatten()
    .any(|f| f.to_lowercase().contains(&q))
}

/// True when the entry passes the active state pill.
///
/// `Disabled` is the `enabled` flag, NOT a lifecycle state — a model that
/// is disabled while loaded shows under BOTH the `Loaded` and `Disabled`
/// pills.
///
/// NOTE (as of plan-194): `GET /tama/v1/models` currently only surfaces
/// `idle`/`starting`/`ready` (failed rows fold back to Idle in the proxy's
/// wire-row `row_model_state` in tama-core `proxy::status`), so the
/// `Failed` pill is an empty bucket and `Unloading` is not observed.
/// The arms are kept: a follow-up surfacing failed rows into the list
/// state makes them live without a frontend change.
pub(super) fn matches_view(m: &ModelEntry, v: ViewFilter) -> bool {
    match v {
        ViewFilter::All => true,
        ViewFilter::Loaded => matches!(m.state, ModelState::Ready),
        ViewFilter::Idle => matches!(m.state, ModelState::Idle),
        ViewFilter::Failed => matches!(m.state, ModelState::Failed),
        ViewFilter::Disabled => !m.enabled,
    }
}

/// Extract vendor from an entry using the #136 chain of fallbacks:
/// display name split on `':'`, else api name split on `':'`, else
/// `hf_base_model` split on `'/'`; first non-empty piece, else `"other"`.
pub(super) fn extract_vendor(m: &ModelEntry) -> String {
    for (field, separator) in &[
        (&m.display_name, ':'),
        (&m.api_name, ':'),
        (&m.hf_base_model, '/'),
    ] {
        if let Some(name) = field {
            if let Some(vendor) = name.split(*separator).next() {
                let vendor = vendor.trim();
                if !vendor.is_empty() {
                    return vendor.to_string();
                }
            }
        }
    }
    "other".to_string()
}

/// Returns `(priority, index)` for GPU sorting: GPU devices sort first
/// (`priority 0`) in numeric device order; `None` / digit-less devices
/// sort last (`(1, 0)`), #136 behavior.
pub(super) fn extract_gpu_sort_key(gpu_device: &Option<String>) -> (u32, u32) {
    match gpu_device {
        Some(device) => {
            let index = extract_gpu_index(device).unwrap_or(0);
            (0, index)
        }
        None => (1, 0),
    }
}

/// Returns a comparable string for all non-GPU sorts.
///
/// `Status` uses the RAW state string (`ModelState::as_str`), not the
/// human badge label — #136 parity: sorting is alphabetical over
/// `"failed","idle","ready","starting","unloading"`.
pub(super) fn extract_sort_key(m: &ModelEntry, sort_by: SortBy) -> String {
    match sort_by {
        SortBy::Name => super::model_display_name(m),
        SortBy::Status => m.state.as_str().to_string(),
        SortBy::Family => m.hf_architecture_type.clone().unwrap_or_default(),
        SortBy::Vendor => extract_vendor(m),
        SortBy::Gpu => String::new(), // GPU sort handled separately in sort_models
    }
}

/// Sort models in place by the given sort criterion.
pub(super) fn sort_models(models: &mut [ModelEntry], sort_by: SortBy) {
    match sort_by {
        SortBy::Gpu => {
            models.sort_by(|a, b| {
                let ka = extract_gpu_sort_key(&a.gpu_device);
                let kb = extract_gpu_sort_key(&b.gpu_device);
                ka.cmp(&kb)
            });
        }
        _ => {
            models.sort_by_key(|a| extract_sort_key(a, sort_by));
        }
    }
}

/// Returns the grouping key for a model.
pub(super) fn extract_group_key(m: &ModelEntry, by: GroupBy) -> String {
    match by {
        GroupBy::Gpu => super::gpu_group_label(&m.gpu_device),
        GroupBy::Family => m
            .hf_architecture_type
            .clone()
            .unwrap_or_else(|| String::from("Unknown")),
        GroupBy::Vendor => extract_vendor(m),
        GroupBy::Status => model_status_badge_label(&m.state).to_string(),
    }
}

/// Returns display order for group headers: for GPU groups, numeric
/// device order with "No GPU" last; everything else is 0 so the label
/// tiebreak (ascending) applies.
pub(super) fn group_display_order(by: GroupBy, key: &str) -> u32 {
    match by {
        GroupBy::Gpu => {
            if key == "No GPU" {
                return u32::MAX;
            }
            extract_gpu_index(key).unwrap_or(0)
        }
        _ => 0,
    }
}

/// Parse a stored/rendered string into a `SortBy` (unknown → `Name`).
pub(super) fn parse_sort_by(s: &str) -> SortBy {
    match s {
        "status" => SortBy::Status,
        "gpu" => SortBy::Gpu,
        "family" => SortBy::Family,
        "vendor" => SortBy::Vendor,
        _ => SortBy::Name,
    }
}

/// Parse a stored/rendered string into an `Option<GroupBy>` (`""`/unknown → `None`).
pub(super) fn parse_group_by(s: &str) -> Option<GroupBy> {
    match s {
        "gpu" => Some(GroupBy::Gpu),
        "family" => Some(GroupBy::Family),
        "vendor" => Some(GroupBy::Vendor),
        "status" => Some(GroupBy::Status),
        _ => None,
    }
}

/// Parse a rendered string into a `ViewFilter` (unknown → `All`).
/// Kept for API parity with [`parse_sort_by`] / [`parse_group_by`]: the
/// toolbar pills set the variant directly, so nothing outside the tests
/// below calls it yet.
#[allow(dead_code)] // Used by tests + parity with the other parse fns
pub(super) fn parse_view_filter(s: &str) -> ViewFilter {
    match s {
        "loaded" => ViewFilter::Loaded,
        "idle" => ViewFilter::Idle,
        "failed" => ViewFilter::Failed,
        "disabled" => ViewFilter::Disabled,
        _ => ViewFilter::All,
    }
}

/// Apply the fixed filter pipeline — search → state pill → sort — to an
/// already-fetched model list. Pure: no fetches, no mutation of `models`.
pub(super) fn apply_pipeline(
    models: &[ModelEntry],
    query: &str,
    view: ViewFilter,
    sort_by: SortBy,
) -> Vec<ModelEntry> {
    let mut visible: Vec<ModelEntry> = models
        .iter()
        .filter(|m| matches_search(m, query) && matches_view(m, view))
        .cloned()
        .collect();
    sort_models(&mut visible, sort_by);
    visible
}

/// Bucket models by an existing (already-sorted) order into ordered
/// groups. Group keys are ordered by [`group_display_order`] with label
/// ascending as the tiebreak; members keep their input order.
pub(super) fn group_survivors(
    models: &[ModelEntry],
    by: GroupBy,
) -> Vec<(String, Vec<ModelEntry>)> {
    let mut buckets: Vec<(String, Vec<ModelEntry>)> = Vec::new();
    for m in models {
        let key = extract_group_key(m, by);
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, bucket)) => bucket.push(m.clone()),
            None => buckets.push((key, vec![m.clone()])),
        }
    }
    buckets.sort_by(|(a, _), (b, _)| {
        group_display_order(by, a)
            .cmp(&group_display_order(by, b))
            .then_with(|| a.cmp(b))
    });
    buckets
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn state_of(state: &str) -> ModelState {
        match state {
            "ready" => ModelState::Ready,
            "failed" => ModelState::Failed,
            "loading" | "starting" => ModelState::Starting,
            "unloading" => ModelState::Unloading,
            _ => ModelState::Idle,
        }
    }

    fn entry_with(
        display_name: Option<&str>,
        api_name: Option<&str>,
        model: Option<&str>,
        quant: Option<&str>,
        hf_base_model: Option<&str>,
        gpu_device: Option<&str>,
        state: &str,
    ) -> ModelEntry {
        ModelEntry {
            id: 0,
            backend: "test".to_string(),
            model: model.map(String::from),
            quant: quant.map(String::from),
            enabled: true,
            state: state_of(state),
            api_name: api_name.map(String::from),
            display_name: display_name.map(String::from),
            gpu_device: gpu_device.map(String::from),
            gpu_variant: None,
            hf_architecture_type: None,
            hf_base_model: hf_base_model.map(String::from),
            hf_format: None,
            context_length: None,
            cache_type_k: None,
            cache_type_v: None,
            spec_types: vec![],
            log_source: None,
            vllm: serde_json::Value::Null,
        }
    }

    /// Convenience constructor: display name + optional GPU + state.
    fn entry(name: &str, gpu: Option<&str>, state: &str) -> ModelEntry {
        entry_with(Some(name), None, None, None, None, gpu, state)
    }

    // ── matches_search ────────────────────────────────────────────────────

    #[test]
    fn test_matches_search_empty_query_matches_all() {
        let m = entry_with(None, None, None, None, None, None, "idle");
        assert!(matches_search(&m, ""));
        assert!(matches_search(&m, "   "));
        let g = entry("Zebra", Some("CUDA0"), "ready");
        assert!(matches_search(&g, "   \t  "));
    }

    #[test]
    fn test_matches_search_repo_and_quant() {
        let m = entry_with(
            None,
            None,
            Some("unsloth/Qwen3.5-4B-GGUF"),
            Some("Q4_K_M"),
            None,
            None,
            "idle",
        );
        assert!(matches_search(&m, "unsloth"));
        assert!(matches_search(&m, "q4_k_m"));
        assert!(!matches_search(&m, "nonexistent"));
    }

    #[test]
    fn test_matches_search_case_insensitive_display_name() {
        let m = entry("Zephyr-7B-it", None, "ready");
        assert!(matches_search(&m, "ZEPhYr"));
        assert!(matches_search(&m, "it"));
        assert!(!matches_search(&m, "llama"));
    }

    // ── matches_view ──────────────────────────────────────────────────────

    #[test]
    fn test_matches_view_loaded_uses_ready_state() {
        let ready = entry("a", None, "ready");
        let idle = entry("b", None, "idle");
        assert!(matches_view(&ready, ViewFilter::Loaded));
        assert!(!matches_view(&idle, ViewFilter::Loaded));
        assert!(!matches_view(&idle, ViewFilter::Loaded));
    }

    #[test]
    fn test_matches_view_disabled_uses_enabled_flag() {
        // A disabled+loaded model matches BOTH pills (plan acceptance).
        let mut disabled_loaded = entry("a", None, "ready");
        disabled_loaded.enabled = false;
        assert!(matches_view(&disabled_loaded, ViewFilter::Disabled));
        assert!(matches_view(&disabled_loaded, ViewFilter::Loaded));
        assert!(!matches_view(
            &entry("b", None, "idle"),
            ViewFilter::Disabled
        ));
    }

    #[test]
    fn test_matches_view_all() {
        let mut disabled = entry("a", None, "failed");
        disabled.enabled = false;
        assert!(matches_view(&disabled, ViewFilter::All));
        assert!(matches_view(
            &entry("b", Some("CUDA1"), "unloading"),
            ViewFilter::All
        ));
    }

    // ── extract_vendor (ported from extract_vendor_model_status) ─────────

    #[test]
    fn test_extract_vendor_from_display_name() {
        let m = entry("Unsloth: Qwen3.6 27B", None, "idle");
        assert_eq!(extract_vendor(&m), "Unsloth");
    }

    #[test]
    fn test_extract_vendor_from_api_name() {
        let m = entry_with(
            None,
            Some("vendor:model-name"),
            None,
            None,
            None,
            None,
            "idle",
        );
        assert_eq!(extract_vendor(&m), "vendor");
    }

    #[test]
    fn test_extract_vendor_from_hf_base_model() {
        let m = entry_with(
            None,
            None,
            None,
            None,
            Some("Qwen/Qwen3.6-27B"),
            None,
            "idle",
        );
        assert_eq!(extract_vendor(&m), "Qwen");
    }

    #[test]
    fn test_extract_vendor_fallback_other() {
        let m = entry_with(None, None, None, None, None, None, "idle");
        assert_eq!(extract_vendor(&m), "other");
    }

    // ── extract_gpu_sort_key (ported from extract_gpu_sort_key_model_status) ──

    #[test]
    fn test_extract_gpu_sort_key_cuda() {
        let m = entry_with(None, None, None, None, None, Some("CUDA1"), "idle");
        assert_eq!(extract_gpu_sort_key(&m.gpu_device), (0, 1));
    }

    #[test]
    fn test_extract_gpu_sort_key_rocm() {
        let m = entry_with(None, None, None, None, None, Some("ROCm0"), "idle");
        assert_eq!(extract_gpu_sort_key(&m.gpu_device), (0, 0));
    }

    #[test]
    fn test_extract_gpu_sort_key_none() {
        let m = entry_with(None, None, None, None, None, None, "idle");
        assert_eq!(extract_gpu_sort_key(&m.gpu_device), (1, 0));
    }

    #[test]
    fn test_extract_gpu_sort_key_multidigit() {
        let m = entry_with(None, None, None, None, None, Some("CUDA10"), "idle");
        assert_eq!(extract_gpu_sort_key(&m.gpu_device), (0, 10));
    }

    #[test]
    fn test_extract_gpu_sort_key_no_number() {
        let m = entry_with(None, None, None, None, None, Some("GPU"), "idle");
        assert_eq!(extract_gpu_sort_key(&m.gpu_device), (0, 0));
    }

    // ── parse_sort_by / parse_group_by / parse_view_filter ────────────────

    #[test]
    fn test_parse_sort_by() {
        assert_eq!(parse_sort_by("name"), SortBy::Name);
        assert_eq!(parse_sort_by("status"), SortBy::Status);
        assert_eq!(parse_sort_by("gpu"), SortBy::Gpu);
        assert_eq!(parse_sort_by("family"), SortBy::Family);
        assert_eq!(parse_sort_by("vendor"), SortBy::Vendor);
        assert_eq!(parse_sort_by("unknown"), SortBy::Name);
    }

    #[test]
    fn test_parse_group_by() {
        assert_eq!(parse_group_by("gpu"), Some(GroupBy::Gpu));
        assert_eq!(parse_group_by("family"), Some(GroupBy::Family));
        assert_eq!(parse_group_by("vendor"), Some(GroupBy::Vendor));
        assert_eq!(parse_group_by("status"), Some(GroupBy::Status));
        assert_eq!(parse_group_by("none"), None);
        assert_eq!(parse_group_by("unknown"), None);
        assert_eq!(parse_group_by(""), None);
    }

    #[test]
    fn test_parse_view_filter_unknown_defaults_to_all() {
        assert_eq!(parse_view_filter("loaded"), ViewFilter::Loaded);
        assert_eq!(parse_view_filter("idle"), ViewFilter::Idle);
        assert_eq!(parse_view_filter("failed"), ViewFilter::Failed);
        assert_eq!(parse_view_filter("disabled"), ViewFilter::Disabled);
        assert_eq!(parse_view_filter(""), ViewFilter::All);
        assert_eq!(parse_view_filter("bogus"), ViewFilter::All);
    }

    // ── extract_group_key (ported from extract_group_key_model_status) ────

    #[test]
    fn test_extract_group_key_gpu() {
        let m = entry_with(None, None, None, None, None, Some("CUDA1"), "idle");
        assert_eq!(extract_group_key(&m, GroupBy::Gpu), "GPU 1");
    }

    #[test]
    fn test_extract_group_key_status_loaded() {
        let m = entry_with(None, None, None, None, None, None, "ready");
        assert_eq!(extract_group_key(&m, GroupBy::Status), "Loaded");
    }

    #[test]
    fn test_extract_group_key_status_idle() {
        let m = entry_with(None, None, None, None, None, None, "idle");
        assert_eq!(extract_group_key(&m, GroupBy::Status), "Idle");
    }

    #[test]
    fn test_extract_group_key_family_unknown() {
        let m = entry_with(None, None, None, None, None, None, "idle");
        assert_eq!(extract_group_key(&m, GroupBy::Family), "Unknown");
    }

    // ── group_display_order ───────────────────────────────────────────────

    #[test]
    fn test_group_display_order_gpu_no_gpu_last() {
        assert_eq!(group_display_order(GroupBy::Gpu, "No GPU"), u32::MAX);
        assert_eq!(group_display_order(GroupBy::Gpu, "GPU 0"), 0);
        assert_eq!(group_display_order(GroupBy::Gpu, "GPU 1"), 1);
    }

    #[test]
    fn test_group_display_order_non_gpu_zero() {
        assert_eq!(group_display_order(GroupBy::Family, "qwen35"), 0);
        assert_eq!(group_display_order(GroupBy::Vendor, "Unsloth"), 0);
    }

    // ── sort_models (ported from sort_models_status integration tests) ────

    #[test]
    fn test_sort_models_by_name() {
        let mut models = vec![entry("Zebra", None, "idle"), entry("Alpha", None, "idle")];
        sort_models(&mut models, SortBy::Name);
        assert_eq!(
            models[0].display_name,
            Some("Alpha".to_string()),
            "Alpha should sort before Zebra"
        );
    }

    #[test]
    fn test_sort_models_by_gpu_gpu_first() {
        let mut models = vec![
            entry_with(None, None, None, None, None, None, "idle"),
            entry_with(None, None, None, None, None, Some("CUDA0"), "idle"),
            entry_with(None, None, None, None, None, Some("CUDA1"), "idle"),
        ];
        sort_models(&mut models, SortBy::Gpu);
        assert!(
            models[0].gpu_device.is_some(),
            "GPU models should sort first"
        );
        assert!(
            models[1].gpu_device.is_some(),
            "GPU models should sort first"
        );
        assert!(models[2].gpu_device.is_none(), "Non-GPU should sort last");
    }

    #[test]
    fn test_sort_models_by_gpu_numeric_order() {
        let mut models = vec![
            entry_with(None, None, None, None, None, Some("CUDA1"), "idle"),
            entry_with(None, None, None, None, None, Some("CUDA0"), "idle"),
            entry_with(None, None, None, None, None, Some("CUDA10"), "idle"),
        ];
        sort_models(&mut models, SortBy::Gpu);
        assert_eq!(models[0].gpu_device, Some("CUDA0".to_string()));
        assert_eq!(models[1].gpu_device, Some("CUDA1".to_string()));
        assert_eq!(models[2].gpu_device, Some("CUDA10".to_string()));
    }

    // ── apply_pipeline / group_survivors ──────────────────────────────────

    #[test]
    fn test_apply_pipeline_composes_search_and_view() {
        let mut disabled_ready = entry("unsloth/qwen Q4_K_M", Some("CUDA1"), "ready");
        disabled_ready.enabled = false;
        let mut failed_idle = entry("other/llama Q8", None, "failed");
        failed_idle.enabled = false; // disabled but failed — must stay out of Loaded
        let models = vec![
            entry("meta/llama Q8_0", None, "idle"),
            disabled_ready,
            failed_idle,
        ];

        // Only the disabled-ready model is loaded; the disabled-failed
        // model must NOT leak into the Loaded pill.
        let loaded = apply_pipeline(&models, "", ViewFilter::Loaded, SortBy::Name);
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].display_name,
            Some("unsloth/qwen Q4_K_M".to_string())
        );

        // Search narrows further; Disabled pill sees both disabled models.
        let q = apply_pipeline(&models, "qwen", ViewFilter::Disabled, SortBy::Name);
        assert_eq!(q.len(), 1);
        let disabled = apply_pipeline(&models, "", ViewFilter::Disabled, SortBy::Name);
        assert_eq!(disabled.len(), 2);

        // Sort is applied after filtering (single survivor aside, check order in the full set).
        let all = apply_pipeline(&models, "", ViewFilter::All, SortBy::Name);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].display_name, Some("meta/llama Q8_0".to_string()));
    }

    #[test]
    fn test_group_survivors_gpu_order_no_gpu_last() {
        // group_survivors must NOT rely on input order for GPU ordering
        // (input is the already-sorted pipeline output, but this proves
        // the numeric tie-break independent of input order).
        let models = vec![
            entry("a", None, "idle"), // No GPU
            entry("b", Some("CUDA10"), "idle"),
            entry("c", Some("CUDA1"), "idle"),
            entry_with(None, Some("a1"), None, None, None, Some("CUDA0"), "idle"),
        ];
        let groups = group_survivors(&models, GroupBy::Gpu);
        let labels: Vec<&str> = groups.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            labels,
            vec!["GPU 0", "GPU 1", "GPU 10", "No GPU"],
            "NUMERIC GPU order with No GPU last (not lexicographic)"
        );
        assert_eq!(
            groups[3].1.len(),
            1,
            "single No-GPU entry in the last bucket"
        );
    }
}
