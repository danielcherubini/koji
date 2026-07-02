use super::*;
use crate::components::gpu_device_card::{
    derive_device_state, device_display_label, find_device_index, format_vram_short,
    loaded_model_display, model_gpu_label, GpuDeviceState,
};

/// `MetricCurrent` must deserialize a payload that has no `models` field at
/// all (older backend builds, cached responses) by defaulting to an empty
/// `Vec`. The `#[serde(default)]` attribute on the field is what makes this
/// work — without it, deserialization would fail with a `missing field`
/// error and break the dashboard during a partial rollout.
#[test]
fn metric_current_deserializes_without_models_field() {
    let json = r#"{
        "models_loaded": 0
    }"#;

    let cur: MetricCurrent = serde_json::from_str(json)
        .expect("MetricCurrent without `models` must deserialize via #[serde(default)]");

    assert_eq!(cur.models_loaded, 0);
    assert!(
        cur.models.is_empty(),
        "missing `models` field must default to an empty Vec"
    );
}

/// `MetricHistoryPoint` must deserialize a payload that has no `network`
/// field at all (older backend builds, cached responses) by defaulting to
/// `None`. The `#[serde(default, skip_serializing_if = "Option::is_none")]`
/// attributes on the field make this work.
#[test]
fn metric_history_point_deserializes_without_network_field() {
    let json = r#"{
        "ts_unix_ms": 1700000000000,
        "cpu_usage_pct": 12.5,
        "ram_used_mib": 2048,
        "ram_total_mib": 16384
    }"#;

    let sample: MetricHistoryPoint = serde_json::from_str(json)
        .expect("MetricHistoryPoint without `network` must deserialize via #[serde(default)]");

    assert_eq!(sample.ts_unix_ms, 1_700_000_000_000);
    assert_eq!(sample.cpu_usage_pct, 12.5);
    assert!(
        sample.network.is_none(),
        "missing `network` field must default to None"
    );
}

/// The `format_number` helper must produce comma-separated thousands.
#[test]
fn format_number_adds_commas() {
    assert_eq!(format_number(0), "0");
    assert_eq!(format_number(999), "999");
    assert_eq!(format_number(1000), "1,000");
    assert_eq!(format_number(12345), "12,345");
    assert_eq!(format_number(123456), "123,456");
    assert_eq!(format_number(1234567), "1,234,567");
    assert_eq!(format_number(16384), "16,384");
    assert_eq!(format_number(65183), "65,183");
}

/// `active_models` returns entries whose state is "ready", "loading", or
/// "unloading", preserving order and including all fields.
#[test]
fn active_models_returns_ready_loading_unloading_entries() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "c".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "ik_llama".into(),
            state: "loading".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "d".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "ik_llama".into(),
            state: "failed".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "e".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".into(),
            state: "unloading".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert_eq!(active.len(), 3);
    assert_eq!(active[0].id, "a");
    assert_eq!(active[0].state, "ready");
    assert_eq!(active[1].id, "c");
    assert_eq!(active[1].state, "loading");
    assert_eq!(active[2].id, "e");
    assert_eq!(active[2].state, "unloading");
}

/// `active_models` includes ready, loading, and unloading models.
#[test]
fn active_models_filters_to_active_states() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "c".into(),
            state: "loading".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "d".into(),
            state: "unloading".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert_eq!(active.len(), 3);
    assert_eq!(active[0].id, "a");
    assert_eq!(active[1].id, "c");
    assert_eq!(active[2].id, "d");
}

/// `active_models` returns an empty vec when all models are idle or failed.
#[test]
fn active_models_returns_empty_when_none_active() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "failed".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert!(active.is_empty());
}

/// `active_models` returns a clone of all models when all are active.
#[test]
fn active_models_returns_all_when_all_active() {
    let models = vec![
        ModelStatus {
            id: "x".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "y".into(),
            state: "loading".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert_eq!(active.len(), 2);
    assert_eq!(active[0].id, "x");
    assert_eq!(active[1].id, "y");
}

/// `active_models` returns an empty vec for an empty input slice.
#[test]
fn active_models_returns_empty_for_empty_input() {
    let models: Vec<ModelStatus> = vec![];
    let active = active_models(&models);
    assert!(active.is_empty());
}

/// `inactive_models` returns entries whose state is NOT "ready", "loading",
/// or "unloading" — i.e. idle, failed, and any unknown states.
#[test]
fn inactive_models_returns_idle_failed_and_unknown_entries() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "c".into(),
            state: "loading".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "d".into(),
            state: "failed".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "e".into(),
            state: "unloading".into(),
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 2);
    assert_eq!(inactive[0].id, "b");
    assert_eq!(inactive[0].state, "idle");
    assert_eq!(inactive[1].id, "d");
    assert_eq!(inactive[1].state, "failed");
}

/// `inactive_models` returns an empty vec when all models are active
/// (ready, loading, or unloading).
#[test]
fn inactive_models_returns_empty_when_all_active() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "loading".into(),
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert!(inactive.is_empty());
}

/// `inactive_models` returns all models when none are active.
#[test]
fn inactive_models_returns_all_when_none_active() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "failed".into(),
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 2);
    assert_eq!(inactive[0].id, "a");
    assert_eq!(inactive[1].id, "b");
}

/// `inactive_models` returns an empty vec for an empty input slice.
#[test]
fn inactive_models_returns_empty_for_empty_input() {
    let models: Vec<ModelStatus> = vec![];
    let inactive = inactive_models(&models);
    assert!(inactive.is_empty());
}

/// `inactive_models` preserves all model fields (display_name, quant,
/// context_length, db_id, backend) so the Inactive Models section can
/// render them without any data loss.
#[test]
fn inactive_models_preserves_all_fields() {
    let models = vec![
        ModelStatus {
            id: "llama3-8b".into(),
            db_id: Some(1),
            api_name: Some("meta-llama/Llama-3-8B".into()),
            display_name: Some("Llama 3 8B".into()),
            backend: "llama_cpp".into(),
            state: "ready".into(),
            quant: Some("Q4_K_M".into()),
            context_length: Some(8192),
            ..Default::default()
        },
        ModelStatus {
            id: "mistral-7b".into(),
            db_id: Some(2),
            api_name: Some("mistralai/Mistral-7B".into()),
            display_name: Some("Mistral 7B".into()),
            backend: "llama_cpp".into(),
            state: "idle".into(),
            quant: Some("Q4_0".into()),
            context_length: Some(32768),
            ..Default::default()
        },
        ModelStatus {
            id: "gemma-2b".into(),
            db_id: Some(3),
            api_name: Some("google/gemma-2b".into()),
            display_name: Some("Gemma 2B".into()),
            backend: "llama_cpp".into(),
            state: "failed".into(),
            quant: Some("Q5_K_M".into()),
            context_length: Some(4096),
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 2);

    // Verify idle model fields are preserved
    let idle_model = &inactive
        .iter()
        .find(|m| m.state == "idle")
        .expect("idle model missing");
    assert_eq!(idle_model.id, "mistral-7b");
    assert_eq!(idle_model.db_id, Some(2));
    assert_eq!(idle_model.display_name, Some("Mistral 7B".into()));
    assert_eq!(idle_model.quant, Some("Q4_0".into()));
    assert_eq!(idle_model.context_length, Some(32768));
    assert_eq!(idle_model.backend, "llama_cpp");

    // Verify failed model fields are preserved
    let failed_model = &inactive
        .iter()
        .find(|m| m.state == "failed")
        .expect("failed model missing");
    assert_eq!(failed_model.id, "gemma-2b");
    assert_eq!(failed_model.db_id, Some(3));
    assert_eq!(failed_model.display_name, Some("Gemma 2B".into()));
    assert_eq!(failed_model.quant, Some("Q5_K_M".into()));
    assert_eq!(failed_model.context_length, Some(4096));
    assert_eq!(failed_model.backend, "llama_cpp");
}

/// `active_models` and `inactive_models` are symmetric complements:
/// together they must contain exactly all input models, with no overlap.
#[test]
fn active_and_inactive_models_are_symmetric_complements() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "c".into(),
            state: "loading".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "d".into(),
            state: "failed".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    let inactive = inactive_models(&models);

    assert_eq!(active.len() + inactive.len(), models.len());

    // No overlap: no model id appears in both lists.
    let active_ids: Vec<&str> = active.iter().map(|m| m.id.as_str()).collect();
    for inactive_model in &inactive {
        assert!(
            !active_ids.contains(&inactive_model.id.as_str()),
            "model '{}' should not be in both active and inactive",
            inactive_model.id
        );
    }
}

/// When the backend includes a populated `models` array, every `ModelStatus`
/// must round-trip with its `id`, `backend`, and `state` fields preserved.
#[test]
fn metric_current_deserializes_models_field() {
    let json = r#"{
        "models_loaded": 1,
        "models": [
            { "id": "alpha", "api_name": "org/alpha", "backend": "llama_cpp", "loaded": true, "state": "ready" },
            { "id": "beta",  "api_name": "org/beta",  "backend": "ik_llama",  "loaded": false, "state": "idle" }
        ]
    }"#;

    let cur: MetricCurrent =
        serde_json::from_str(json).expect("MetricCurrent with `models` must deserialize");

    assert_eq!(cur.models.len(), 2);

    assert_eq!(cur.models[0].id, "alpha");
    assert_eq!(cur.models[0].api_name, Some("org/alpha".to_string()));
    assert_eq!(cur.models[0].backend, "llama_cpp");
    assert_eq!(cur.models[0].state, "ready");

    assert_eq!(cur.models[1].id, "beta");
    assert_eq!(cur.models[1].api_name, Some("org/beta".to_string()));
    assert_eq!(cur.models[1].backend, "ik_llama");
    assert_eq!(cur.models[1].state, "idle");
}

// ── GpuDeviceCard helper tests ────────────────────────────────────────────

fn make_test_model(id: &str, state: &str, gpu_device: Option<&str>) -> ModelStatus {
    ModelStatus {
        id: id.to_string(),
        db_id: None,
        api_name: None,
        display_name: None,
        backend: "llama_cpp".to_string(),
        #[allow(deprecated)]
        loaded: state == "ready",
        state: state.to_string(),
        quant: None,
        context_length: None,
        hf_architecture_type: None,
        hf_base_model: None,
        gpu_variant: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_types: vec![],
        gpu_device: gpu_device.map(|s| s.to_string()),
        tps: None,
        prompt_tps: None,
        error_message: None,
    }
}

fn make_test_gpu(device_id: &str, vendor: &str) -> GpuDeviceStats {
    GpuDeviceStats {
        device_id: device_id.to_string(),
        name: "Test GPU".to_string(),
        vendor: vendor.to_string(),
        utilization_pct: None,
        vram: None,
        temperature_c: None,
        power_w: None,
        fan_pct: None,
    }
}

#[test]
fn test_derive_state_active_when_ready_model() {
    let models = vec![make_test_model("m1", "ready", Some("GPU0"))];
    assert_eq!(derive_device_state(&models, "GPU0"), GpuDeviceState::Active);
}

#[test]
fn test_derive_state_loading_when_loading_model() {
    let models = vec![make_test_model("m1", "loading", Some("GPU0"))];
    assert_eq!(
        derive_device_state(&models, "GPU0"),
        GpuDeviceState::Loading
    );
}

#[test]
fn test_derive_state_failed_when_only_failed() {
    let models = vec![make_test_model("m1", "failed", Some("GPU0"))];
    assert_eq!(derive_device_state(&models, "GPU0"), GpuDeviceState::Failed);
}

#[test]
fn test_derive_state_idle_when_no_models() {
    let models: Vec<ModelStatus> = vec![];
    assert_eq!(derive_device_state(&models, "GPU0"), GpuDeviceState::Idle);
}

#[test]
fn test_derive_state_loading_overrides_ready() {
    let models = vec![
        make_test_model("m1", "ready", Some("GPU0")),
        make_test_model("m2", "loading", Some("GPU0")),
    ];
    assert_eq!(
        derive_device_state(&models, "GPU0"),
        GpuDeviceState::Loading
    );
}

#[test]
fn test_device_display_label_format() {
    assert_eq!(device_display_label(0), "GPU 0");
    assert_eq!(device_display_label(3), "GPU 3");
}

#[test]
fn test_find_device_index_match() {
    let gpus = vec![
        make_test_gpu("GPU0", "nvidia"),
        make_test_gpu("GPU1", "nvidia"),
    ];
    assert_eq!(find_device_index(&gpus, "GPU0"), Some(0));
    assert_eq!(find_device_index(&gpus, "GPU1"), Some(1));
}

#[test]
fn test_find_device_index_no_match() {
    let gpus = vec![make_test_gpu("GPU0", "nvidia")];
    assert_eq!(find_device_index(&gpus, "GPU1"), None);
}

#[test]
fn test_model_gpu_label_resolves_to_position() {
    let gpus = vec![
        make_test_gpu("GPU0", "nvidia"),
        make_test_gpu("GPU1", "nvidia"),
    ];
    let model = make_test_model("m1", "ready", Some("GPU0"));
    assert_eq!(model_gpu_label(&gpus, &model), Some("GPU 0".to_string()));
}

#[test]
fn test_loaded_model_display_transferring() {
    let models = vec![make_test_model("m1", "loading", Some("GPU0"))];
    let display = loaded_model_display(&models, "GPU0");
    assert!(display.is_some());
    let d = display.unwrap();
    assert_eq!(d.name, "m1");
    assert!(d.transferring);
}

#[test]
fn test_loaded_model_display_active() {
    let models = vec![make_test_model("m1", "ready", Some("GPU0"))];
    let display = loaded_model_display(&models, "GPU0");
    assert!(display.is_some());
    let d = display.unwrap();
    assert_eq!(d.name, "m1");
    assert!(!d.transferring);
}

#[test]
fn test_loaded_model_display_none_when_idle() {
    let models: Vec<ModelStatus> = vec![];
    assert_eq!(loaded_model_display(&models, "GPU0"), None);
}

#[test]
fn test_format_vram_short() {
    let vram = VramInfo {
        used_mib: 22937,
        total_mib: 24576,
    };
    assert_eq!(format_vram_short(&vram), "22.4 / 24.0 GB");
}

// ── Sort/Group helper tests ────────────────────────────────────────────────

fn make_sort_model(
    display_name: Option<&str>,
    api_name: Option<&str>,
    hf_base_model: Option<&str>,
    gpu_device: Option<&str>,
    state: &str,
) -> ModelStatus {
    ModelStatus {
        id: "test".to_string(),
        db_id: None,
        api_name: api_name.map(|s| s.to_string()),
        display_name: display_name.map(|s| s.to_string()),
        backend: "test".to_string(),
        #[allow(deprecated)]
        loaded: false,
        state: state.to_string(),
        quant: None,
        context_length: None,
        hf_architecture_type: None,
        hf_base_model: hf_base_model.map(|s| s.to_string()),
        gpu_variant: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_types: vec![],
        gpu_device: gpu_device.map(|s| s.to_string()),
        tps: None,
        prompt_tps: None,
        error_message: None,
    }
}

// ── extract_vendor_model_status tests ─────────────────────────────────────

#[test]
fn test_extract_vendor_from_display_name() {
    let m = make_sort_model(Some("Unsloth: Qwen3.6 27B"), None, None, None, "idle");
    assert_eq!(extract_vendor_model_status(&m), "Unsloth");
}

#[test]
fn test_extract_vendor_from_api_name() {
    let m = make_sort_model(None, Some("vendor:model-name"), None, None, "idle");
    assert_eq!(extract_vendor_model_status(&m), "vendor");
}

#[test]
fn test_extract_vendor_from_hf_base_model() {
    let m = make_sort_model(None, None, Some("Qwen/Qwen3.6-27B"), None, "idle");
    assert_eq!(extract_vendor_model_status(&m), "Qwen");
}

#[test]
fn test_extract_vendor_fallback_other() {
    let m = make_sort_model(None, None, None, None, "idle");
    assert_eq!(extract_vendor_model_status(&m), "other");
}

// ── extract_gpu_sort_key_model_status tests ───────────────────────────────

#[test]
fn test_extract_gpu_sort_key_cuda() {
    let m = make_sort_model(None, None, None, Some("CUDA1"), "idle");
    assert_eq!(extract_gpu_sort_key_model_status(&m.gpu_device), (0, 1));
}

#[test]
fn test_extract_gpu_sort_key_rocm() {
    let m = make_sort_model(None, None, None, Some("ROCm0"), "idle");
    assert_eq!(extract_gpu_sort_key_model_status(&m.gpu_device), (0, 0));
}

#[test]
fn test_extract_gpu_sort_key_none() {
    let m = make_sort_model(None, None, None, None, "idle");
    assert_eq!(extract_gpu_sort_key_model_status(&m.gpu_device), (1, 0));
}

#[test]
fn test_extract_gpu_sort_key_multidigit() {
    let m = make_sort_model(None, None, None, Some("CUDA10"), "idle");
    assert_eq!(extract_gpu_sort_key_model_status(&m.gpu_device), (0, 10));
}

#[test]
fn test_extract_gpu_sort_key_no_number() {
    let m = make_sort_model(None, None, None, Some("GPU"), "idle");
    assert_eq!(extract_gpu_sort_key_model_status(&m.gpu_device), (0, 0));
}

// ── gpu_group_label_model_status tests ────────────────────────────────────

#[test]
fn test_gpu_group_label_cuda() {
    let m = make_sort_model(None, None, None, Some("CUDA0"), "idle");
    assert_eq!(gpu_group_label_model_status(&m.gpu_device), "GPU 0");
}

#[test]
fn test_gpu_group_label_rocm() {
    let m = make_sort_model(None, None, None, Some("ROCm1"), "idle");
    assert_eq!(gpu_group_label_model_status(&m.gpu_device), "GPU 1");
}

#[test]
fn test_gpu_group_label_none() {
    let m = make_sort_model(None, None, None, None, "idle");
    assert_eq!(gpu_group_label_model_status(&m.gpu_device), "No GPU");
}

#[test]
fn test_gpu_group_label_no_number() {
    let m = make_sort_model(None, None, None, Some("GPU"), "idle");
    assert_eq!(gpu_group_label_model_status(&m.gpu_device), "GPU");
}

// ── capitalize_first tests ────────────────────────────────────────────────

#[test]
fn test_capitalize_first() {
    assert_eq!(capitalize_first("qwen35"), "Qwen35");
    assert_eq!(capitalize_first(""), "");
}

// ── parse_sort_by tests ───────────────────────────────────────────────────

#[test]
fn test_parse_sort_by() {
    assert_eq!(parse_sort_by("name"), SortBy::Name);
    assert_eq!(parse_sort_by("gpu"), SortBy::Gpu);
    assert_eq!(parse_sort_by("unknown"), SortBy::Name);
}

// ── parse_group_by tests ──────────────────────────────────────────────────

#[test]
fn test_parse_group_by() {
    assert_eq!(parse_group_by("gpu"), Some(GroupBy::Gpu));
    assert_eq!(parse_group_by("none"), None);
    assert_eq!(parse_group_by("unknown"), None);
}

// ── extract_group_key_model_status tests ──────────────────────────────────

#[test]
fn test_extract_group_key_gpu() {
    let m = make_sort_model(None, None, None, Some("CUDA1"), "idle");
    assert_eq!(extract_group_key_model_status(&m, GroupBy::Gpu), "GPU 1");
}

#[test]
fn test_extract_group_key_status_loaded() {
    let m = make_sort_model(None, None, None, None, "ready");
    assert_eq!(
        extract_group_key_model_status(&m, GroupBy::Status),
        "Loaded"
    );
}

#[test]
fn test_extract_group_key_status_idle() {
    let m = make_sort_model(None, None, None, None, "idle");
    assert_eq!(extract_group_key_model_status(&m, GroupBy::Status), "Idle");
}

// ── group_display_order tests ─────────────────────────────────────────────

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

// ── sort_models_status integration tests ──────────────────────────────────

#[test]
fn test_sort_models_by_name() {
    let mut models = vec![
        make_sort_model(Some("Zebra"), None, None, None, "idle"),
        make_sort_model(Some("Alpha"), None, None, None, "idle"),
    ];
    sort_models_status(&mut models, SortBy::Name);
    assert_eq!(
        models[0].display_name,
        Some("Alpha".to_string()),
        "Alpha should sort before Zebra"
    );
}

#[test]
fn test_sort_models_by_gpu_gpu_first() {
    let mut models = vec![
        make_sort_model(None, None, None, None, "idle"),
        make_sort_model(None, None, None, Some("CUDA0"), "idle"),
        make_sort_model(None, None, None, Some("CUDA1"), "idle"),
    ];
    sort_models_status(&mut models, SortBy::Gpu);
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
        make_sort_model(None, None, None, Some("CUDA1"), "idle"),
        make_sort_model(None, None, None, Some("CUDA0"), "idle"),
        make_sort_model(None, None, None, Some("CUDA10"), "idle"),
    ];
    sort_models_status(&mut models, SortBy::Gpu);
    assert_eq!(models[0].gpu_device, Some("CUDA0".to_string()));
    assert_eq!(models[1].gpu_device, Some("CUDA1".to_string()));
    assert_eq!(models[2].gpu_device, Some("CUDA10".to_string()));
}
