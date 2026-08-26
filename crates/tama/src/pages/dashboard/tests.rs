use super::*;
use crate::components::gpu_device_card::{
    device_display_label, find_device_index, model_gpu_label,
};
use crate::core_mirrors::{GpuVendor, ModelState};

/// `MetricCurrent` must deserialize a payload that has no `models` field at
/// all (older backend builds, cached responses) by defaulting to an empty
/// `Vec`. The `#[serde(default)]` attribute on the field is what makes this
/// work — without it, deserialization would fail with a `missing field`
/// error and break the dashboard during a partial rollout.
#[test]
fn test_metric_current_deserializes_without_models_field() {
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

/// `MetricBucket` must deserialize a payload that has no `network`
/// field at all (older backend builds, cached responses) by defaulting to
/// `None`. The `#[serde(default, skip_serializing_if = "Option::is_none")]`
/// attributes on the field make this work.
#[test]
fn test_metric_bucket_deserializes_without_network_field() {
    let json = r#"{
        "ts_unix_ms": 1700000000000,
        "cpu_usage_pct": 12.5,
        "ram_used_mib": 2048,
        "ram_total_mib": 16384,
        "complete": true
    }"#;

    let bucket: MetricBucket = serde_json::from_str(json)
        .expect("MetricBucket without `network` must deserialize via #[serde(default)]");

    assert_eq!(bucket.ts_unix_ms, 1_700_000_000_000);
    assert_eq!(bucket.cpu_usage_pct, 12.5);
    assert!(
        bucket.network.is_none(),
        "missing `network` field must default to None"
    );
    assert!(bucket.complete, "complete field must deserialize");
}

/// The `format_number` helper must produce comma-separated thousands.
#[test]
fn test_format_number_adds_commas() {
    assert_eq!(format_number(0), "0");
    assert_eq!(format_number(999), "999");
    assert_eq!(format_number(1000), "1,000");
    assert_eq!(format_number(12345), "12,345");
    assert_eq!(format_number(123456), "123,456");
    assert_eq!(format_number(1234567), "1,234,567");
    assert_eq!(format_number(16384), "16,384");
    assert_eq!(format_number(65183), "65,183");
}

/// `active_models` returns entries whose state is "ready", "starting", or
/// "unloading", preserving order and including all fields.
#[test]
fn test_active_models_returns_ready_loading_unloading_entries() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".into(),
            state: ModelState::Ready,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".into(),
            state: ModelState::Idle,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "c".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "ik_llama".into(),
            state: ModelState::Starting,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "d".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "ik_llama".into(),
            state: ModelState::Failed,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "e".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".into(),
            state: ModelState::Unloading,
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert_eq!(active.len(), 3);
    assert_eq!(active[0].id, "a");
    assert_eq!(active[0].state, ModelState::Ready);
    assert_eq!(active[1].id, "c");
    assert_eq!(active[1].state, ModelState::Starting);
    assert_eq!(active[2].id, "e");
    assert_eq!(active[2].state, ModelState::Unloading);
}

/// `active_models` includes ready, loading, and unloading models.
#[test]
fn test_active_models_filters_to_active_states() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            state: ModelState::Ready,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            state: ModelState::Idle,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "c".into(),
            state: ModelState::Starting,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "d".into(),
            state: ModelState::Unloading,
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
fn test_active_models_returns_empty_when_none_active() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            state: ModelState::Idle,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            state: ModelState::Failed,
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert!(active.is_empty());
}

/// `active_models` returns a clone of all models when all are active.
#[test]
fn test_active_models_returns_all_when_all_active() {
    let models = vec![
        ModelStateSnapshot {
            id: "x".into(),
            state: ModelState::Ready,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "y".into(),
            state: ModelState::Starting,
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
fn test_active_models_returns_empty_for_empty_input() {
    let models: Vec<ModelStateSnapshot> = vec![];
    let active = active_models(&models);
    assert!(active.is_empty());
}

/// `inactive_models` returns entries whose state is NOT "ready", "starting",
/// or "unloading" — i.e. idle, failed, and any unknown states.
#[test]
fn test_inactive_models_returns_idle_failed_and_unknown_entries() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            state: ModelState::Ready,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            state: ModelState::Idle,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "c".into(),
            state: ModelState::Starting,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "d".into(),
            state: ModelState::Failed,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "e".into(),
            state: ModelState::Unloading,
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 2);
    assert_eq!(inactive[0].id, "b");
    assert_eq!(inactive[0].state, ModelState::Idle);
    assert_eq!(inactive[1].id, "d");
    assert_eq!(inactive[1].state, ModelState::Failed);
}

/// `inactive_models` returns an empty vec when all models are active
/// (ready, loading, or unloading).
#[test]
fn test_inactive_models_returns_empty_when_all_active() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            state: ModelState::Ready,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            state: ModelState::Starting,
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert!(inactive.is_empty());
}

/// `inactive_models` returns all models when none are active.
#[test]
fn test_inactive_models_returns_all_when_none_active() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            state: ModelState::Idle,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            state: ModelState::Failed,
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
fn test_inactive_models_returns_empty_for_empty_input() {
    let models: Vec<ModelStateSnapshot> = vec![];
    let inactive = inactive_models(&models);
    assert!(inactive.is_empty());
}

/// `inactive_models` preserves all model fields (display_name, quant,
/// context_length, db_id, backend) so the Inactive Models section can
/// render them without any data loss.
#[test]
fn test_inactive_models_preserves_all_fields() {
    let models = vec![
        ModelStateSnapshot {
            id: "llama3-8b".into(),
            db_id: Some(1),
            api_name: Some("meta-llama/Llama-3-8B".into()),
            display_name: Some("Llama 3 8B".into()),
            backend: "llama_cpp".into(),
            state: ModelState::Ready,
            quant: Some("Q4_K_M".into()),
            context_length: Some(8192),
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "mistral-7b".into(),
            db_id: Some(2),
            api_name: Some("mistralai/Mistral-7B".into()),
            display_name: Some("Mistral 7B".into()),
            backend: "llama_cpp".into(),
            state: ModelState::Idle,
            quant: Some("Q4_0".into()),
            context_length: Some(32768),
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "gemma-2b".into(),
            db_id: Some(3),
            api_name: Some("google/gemma-2b".into()),
            display_name: Some("Gemma 2B".into()),
            backend: "llama_cpp".into(),
            state: ModelState::Failed,
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
        .find(|m| m.state == ModelState::Idle)
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
        .find(|m| m.state == ModelState::Failed)
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
fn test_active_and_inactive_models_are_symmetric_complements() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            state: ModelState::Ready,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            state: ModelState::Idle,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "c".into(),
            state: ModelState::Starting,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "d".into(),
            state: ModelState::Failed,
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

/// When the backend includes a populated `models` array, every `ModelStateSnapshot`
/// must round-trip with its `id`, `backend`, and `state` fields preserved.
#[test]
fn test_metric_current_deserializes_models_field() {
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
    assert_eq!(cur.models[0].state, ModelState::Ready);

    assert_eq!(cur.models[1].id, "beta");
    assert_eq!(cur.models[1].api_name, Some("org/beta".to_string()));
    assert_eq!(cur.models[1].backend, "ik_llama");
    assert_eq!(cur.models[1].state, ModelState::Idle);
}

// ── GpuDeviceCard helper tests ────────────────────────────────────────────

fn make_test_model(id: &str, state: &str, gpu_device: Option<&str>) -> ModelStateSnapshot {
    let model_state = match state {
        "idle" => ModelState::Idle,
        "loading" | "starting" => ModelState::Starting,
        "ready" => ModelState::Ready,
        "unloading" => ModelState::Unloading,
        "failed" => ModelState::Failed,
        _ => ModelState::Idle,
    };
    ModelStateSnapshot {
        id: id.to_string(),
        db_id: None,
        api_name: None,
        display_name: None,
        backend: "llama_cpp".to_string(),
        state: model_state,
        quant: None,
        context_length: None,
        hf_architecture_type: None,
        hf_base_model: None,
        hf_format: None,
        gpu_variant: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_types: vec![],
        gpu_device: gpu_device.map(|s| s.to_string()),
        tps: None,
        prompt_tps: None,
        error_message: None,
        is_docker: false,
        host_name: None,
    }
}

fn make_test_gpu(device_id: &str, vendor: &str) -> GpuDeviceStats {
    let gpu_vendor = match vendor {
        "amd" => GpuVendor::Amd,
        _ => GpuVendor::Nvidia,
    };
    GpuDeviceStats {
        device_id: device_id.to_string(),
        name: "Test GPU".to_string(),
        vendor: gpu_vendor,
        utilization_pct: None,
        vram: None,
        temperature_c: None,
        power_w: None,
        fan_pct: None,
    }
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

/// `MetricsSnapshot` must deserialize a payload with no `hosts` field at all
/// (old backend builds between deploys) by defaulting to an empty array —
/// same back-compat contract as the `models`/`network` defaults.
#[test]
fn test_metrics_snapshot_deserializes_without_hosts_field() {
    let json = r#"{ "buckets": [], "current": { "models_loaded": 0 } }"#;
    let snap: MetricsSnapshot = serde_json::from_str(json).expect("snapshot deserializes");
    assert!(
        snap.hosts.is_empty(),
        "`hosts` must default to an empty Vec"
    );
}

/// A full per-tamad `HostStats` payload (the SSE `hosts[]` wire shape,
/// plan-191 Task 9) deserializes, with `version` optional.
#[test]
fn test_host_stats_deserializes_sse_shape() {
    let json = r#"{
        "tamad_id": "uuid-1",
        "name": "gpu-box",
        "online": true,
        "version": "2.1.0",
        "cpu_percent": 12.5,
        "memory": { "total_bytes": 32768, "used_bytes": 10240 },
        "gpus": [
            {
                "index": 0,
                "name": "gfx9",
                "driver_version": "",
                "vram_total_bytes": 17179869184,
                "vram_used_bytes": 4294967296,
                "utilization_percent": 55.0,
                "temperature_c": 61.0,
                "power_w": 120.0
            }
        ]
    }"#;
    let host: HostStats = serde_json::from_str(json).expect("HostStats deserializes");
    assert_eq!(host.tamad_id, "uuid-1");
    assert_eq!(host.name, "gpu-box");
    assert!(host.online);
    assert_eq!(host.version.as_deref(), Some("2.1.0"));
    assert_eq!(host.memory.total_bytes, 32768);
    assert_eq!(host.gpus.len(), 1);
    assert_eq!(host.gpus[0].utilization_percent, 55.0);

    // Offline host without a health check yet: version absent.
    let json2 = r#"{ "tamad_id": "uuid-2", "name": "cpu-box", "online": false }"#;
    let host2: HostStats = serde_json::from_str(json2).expect("section-2 deserializes");
    assert!(!host2.online);
    assert!(host2.version.is_none());
    assert!(host2.gpus.is_empty());
}

/// `format_uptime` renders human durations for the proxy card.
#[test]
fn test_format_uptime() {
    assert_eq!(format_uptime(30.0), "30s");
    assert_eq!(format_uptime(59.0), "59s");
    assert_eq!(format_uptime(60.0), "1m");
    assert_eq!(format_uptime(7_530.0), "2h 5m");
    assert_eq!(format_uptime(90_061.0), "1d 1h");
    assert_eq!(format_uptime(-5.0), "0s");
}

/// `format_bytes_gib` renders compact GiB for host RAM/VRAM values.
#[test]
fn test_format_bytes_gib() {
    assert_eq!(format_bytes_gib(0), "0 GiB");
    assert_eq!(
        format_bytes_gib(32 * 1024 * 1024 * 1024),
        "32.0 GiB",
        "32 GiB"
    );
    assert_eq!(
        format_bytes_gib(32768),
        "0.0 GiB",
        "32 KiB rounds to 0.0 GiB"
    );
    assert_eq!(format_bytes_gib(512 * 1024 * 1024 * 1024), "512 GiB");
}

/// `format_bytes_gib_rounded` renders whole-GiB subtitles (min 1 GiB).
#[test]
fn test_format_bytes_gib_rounded() {
    assert_eq!(format_bytes_gib_rounded(0), "0 GiB");
    assert_eq!(format_bytes_gib_rounded(16 * 1024 * 1024 * 1024), "16 GiB");
    assert_eq!(format_bytes_gib_rounded(1024), "1 GiB");
}

// ── loaded_or_starting_models tests ───────────────────────────────────────

/// `loaded_or_starting_models` returns only `Ready` and `Starting` models,
/// preserving input order.
#[test]
fn test_loaded_or_starting_models_returns_ready_and_starting_only() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            state: ModelState::Ready,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            state: ModelState::Idle,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "c".into(),
            state: ModelState::Starting,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "d".into(),
            state: ModelState::Unloading,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "e".into(),
            state: ModelState::Failed,
            ..Default::default()
        },
    ];

    let active = loaded_or_starting_models(&models);
    assert_eq!(active.len(), 2);
    assert_eq!(active[0].id, "a");
    assert_eq!(active[0].state, ModelState::Ready);
    assert_eq!(active[1].id, "c");
    assert_eq!(active[1].state, ModelState::Starting);
}

/// Unlike `active_models`, a model that has started unloading no longer
/// counts as loaded — the dashboard's Active Models section must drop it.
#[test]
fn test_loaded_or_starting_models_excludes_unloading() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            state: ModelState::Unloading,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            state: ModelState::Ready,
            ..Default::default()
        },
    ];

    let active = loaded_or_starting_models(&models);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "b");
}

/// `loaded_or_starting_models` returns an empty vec when no model is
/// ready or starting.
#[test]
fn test_loaded_or_starting_models_returns_empty_when_none_loaded() {
    let models = vec![
        ModelStateSnapshot {
            id: "a".into(),
            state: ModelState::Idle,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "b".into(),
            state: ModelState::Failed,
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "c".into(),
            state: ModelState::Unloading,
            ..Default::default()
        },
    ];

    assert!(loaded_or_starting_models(&models).is_empty());
}

/// `loaded_or_starting_models` returns an empty vec for an empty input slice.
#[test]
fn test_loaded_or_starting_models_returns_empty_for_empty_input() {
    let models: Vec<ModelStateSnapshot> = vec![];
    assert!(loaded_or_starting_models(&models).is_empty());
}

/// `loaded_or_starting_models` preserves all model fields so the Active
/// Models rows can render without data loss.
#[test]
fn test_loaded_or_starting_models_preserves_fields() {
    let models = vec![
        ModelStateSnapshot {
            id: "alpha".into(),
            db_id: Some(7),
            api_name: Some("org/alpha".into()),
            display_name: Some("Alpha 27B".into()),
            backend: "ik_llama".into(),
            state: ModelState::Starting,
            quant: Some("fp8".into()),
            context_length: Some(262144),
            tps: Some(12.0),
            ..Default::default()
        },
        ModelStateSnapshot {
            id: "beta".into(),
            state: ModelState::Idle,
            ..Default::default()
        },
    ];

    let active = loaded_or_starting_models(&models);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "alpha");
    assert_eq!(active[0].db_id, Some(7));
    assert_eq!(active[0].display_name, Some("Alpha 27B".into()));
    assert_eq!(active[0].quant, Some("fp8".into()));
    assert_eq!(active[0].context_length, Some(262144));
    assert_eq!(active[0].tps, Some(12.0));
}

// ── format_cluster_subtitle tests ─────────────────────────────────────────

/// The cluster subtitle renders the canonical line with rounded
/// throughput and count-inflected words (`1 Model`, `2 Nodes`).
#[test]
fn test_format_cluster_subtitle_full() {
    assert_eq!(
        format_cluster_subtitle(2, 3, 1, Some(53.4)),
        "2 Nodes (3 GPUs) · 1 Model Active · 53 tok/s"
    );
}

/// Singular counts render with singular labels: `1 Node (1 GPU) ·
/// 1 Model Active`.
#[test]
fn test_format_cluster_subtitle_singular() {
    assert_eq!(
        format_cluster_subtitle(1, 1, 1, Some(53.0)),
        "1 Node (1 GPU) · 1 Model Active · 53 tok/s"
    );
}

/// Throughput renders as `—` when `None` or zero, and a zero-sized cluster
/// still produces a well-formed line.
#[test]
fn test_format_cluster_subtitle_zero_cluster_and_no_throughput() {
    assert_eq!(
        format_cluster_subtitle(0, 0, 0, None),
        "0 Nodes (0 GPUs) · 0 Models Active · —"
    );
    assert_eq!(
        format_cluster_subtitle(1, 2, 2, Some(0.0)),
        "1 Node (2 GPUs) · 2 Models Active · —"
    );
}

// ── gateway_status_text tests ─────────────────────────────────────────────

/// The header status pill text covers every known/unknown combination of
/// version + uptime, and always reports offline when the stream is down.
#[test]
fn test_gateway_status_text_variants() {
    assert_eq!(
        gateway_status_text(false, Some("2.1.0"), Some(8_100.0)),
        "● Gateway Offline"
    );
    assert_eq!(
        gateway_status_text(true, Some("2.1.0"), Some(7_950.0)),
        "● Gateway Online (v2.1.0) · Up 2h 12m"
    );
    assert_eq!(
        gateway_status_text(true, None, Some(7_950.0)),
        "● Gateway Online · Up 2h 12m"
    );
    assert_eq!(
        gateway_status_text(true, Some("2.1.0"), None),
        "● Gateway Online (v2.1.0)"
    );
    assert_eq!(gateway_status_text(true, None, None), "● Gateway Online");
}

// ── format_model_meta_parts tests ─────────────────────────────────────────

/// All meta parts present: `gpu_variant · quant · {ctx}k ctx · format`,
/// with context length abbreviated in thousands.
#[test]
fn test_format_model_meta_parts_full() {
    let m = ModelStateSnapshot {
        gpu_variant: Some("radiance (rocm)".into()),
        quant: Some("fp8".into()),
        context_length: Some(262_144),
        hf_format: Some("safetensors".into()),
        ..Default::default()
    };
    assert_eq!(
        format_model_meta_parts(&m).join(" · "),
        "radiance (rocm) · fp8 · 262k ctx · safetensors"
    );
}

/// Context lengths of 1000+ are rounded to the nearest thousand, not
/// truncated: `262_800` → `263k`, `1_500` → `2k` (truncation would give
/// `1k`).
#[test]
fn test_format_model_meta_parts_ctx_rounds_to_nearest_thousand() {
    let m = ModelStateSnapshot {
        context_length: Some(262_800),
        ..Default::default()
    };
    assert_eq!(format_model_meta_parts(&m), vec!["263k ctx".to_string()]);

    let m2 = ModelStateSnapshot {
        context_length: Some(1_500),
        ..Default::default()
    };
    assert_eq!(format_model_meta_parts(&m2), vec!["2k ctx".to_string()]);
}

/// Missing parts are skipped and joined gracefully.
#[test]
fn test_format_model_meta_parts_skips_missing_parts() {
    let m = ModelStateSnapshot {
        quant: Some("Q4_K_M".into()),
        context_length: Some(4_096),
        ..Default::default()
    };
    assert_eq!(format_model_meta_parts(&m).join(" · "), "Q4_K_M · 4k ctx");
}

/// Context lengths below 1000 are rendered as raw numbers.
#[test]
fn test_format_model_meta_parts_raw_ctx_under_1k() {
    let m = ModelStateSnapshot {
        context_length: Some(512),
        ..Default::default()
    };
    assert_eq!(format_model_meta_parts(&m), vec!["512 ctx".to_string()]);
}

/// A model with no meta info at all produces no parts.
#[test]
fn test_format_model_meta_parts_empty() {
    let m = ModelStateSnapshot::default();
    assert!(format_model_meta_parts(&m).is_empty());
}

// ── Inference telemetry helper tests (plan-192 Task 3) ────────────────────

fn make_bucket(ts_unix_ms: i64, tps: f32, prompt_tps: f32) -> MetricBucket {
    MetricBucket {
        ts_unix_ms,
        cpu_usage_pct: 0.0,
        ram_used_mib: 0,
        ram_total_mib: 0,
        network: None,
        gpu_utils: vec![],
        tps,
        prompt_tps,
        complete: true,
    }
}

/// `build_inference_telemetry` returns per-bucket TG/PP series in bucket
/// (oldest → newest) order with peaks equal to the window max.
#[test]
fn test_build_inference_telemetry_series_and_peaks() {
    let buckets = vec![
        make_bucket(1_000_000, 10.0, 500.0),
        make_bucket(1_030_000, 40.5, 120.0),
        make_bucket(1_060_000, 25.0, 800.0),
    ];

    let telemetry = build_inference_telemetry(&buckets);

    assert_eq!(
        telemetry.tg,
        vec![10.0, 40.5, 25.0],
        "TG series must preserve bucket order and values"
    );
    assert_eq!(
        telemetry.pp,
        vec![500.0, 120.0, 800.0],
        "PP series must preserve bucket order and values"
    );
    assert_eq!(telemetry.tg_peak, 40.5, "TG peak must be the window max");
    assert_eq!(telemetry.pp_peak, 800.0, "PP peak must be the window max");
}

/// A single-bucket window yields a length-1 series whose peaks equal that
/// bucket's own values (the chart scales against its only peak).
#[test]
fn test_build_inference_telemetry_single_bucket() {
    let buckets = vec![make_bucket(1_000_000, 42.0, 900.0)];

    let telemetry = build_inference_telemetry(&buckets);

    assert_eq!(
        telemetry.tg,
        vec![42.0],
        "single bucket → length-1 TG series"
    );
    assert_eq!(
        telemetry.pp,
        vec![900.0],
        "single bucket → length-1 PP series"
    );
    assert_eq!(
        telemetry.tg_peak, 42.0,
        "single-bucket TG peak is that bucket's value"
    );
    assert_eq!(
        telemetry.pp_peak, 900.0,
        "single-bucket PP peak is that bucket's value"
    );
}

/// An empty window yields empty series and zero peaks (the view renders a
/// flat max-1.0 chart from these, never dividing by zero).
#[test]
fn test_build_inference_telemetry_empty_window() {
    let telemetry = build_inference_telemetry(&[]);

    assert!(telemetry.tg.is_empty(), "empty window → no TG samples");
    assert!(telemetry.pp.is_empty(), "empty window → no PP samples");
    assert_eq!(telemetry.tg_peak, 0.0, "empty window → TG peak 0.0");
    assert_eq!(telemetry.pp_peak, 0.0, "empty window → PP peak 0.0");
}

/// `ms_per_token` is the latency form of throughput (1000 / tps ms) and is
/// `None` for non-positive or NaN throughputs.
#[test]
fn test_ms_per_token() {
    assert_eq!(ms_per_token(0.0), None, "0 tok/s has no ITL");
    assert_eq!(ms_per_token(-12.5), None, "negative throughput has no ITL");
    assert_eq!(
        ms_per_token(f32::NAN),
        None,
        "NaN throughput must be rejected by the tps > 0.0 guard"
    );

    let ms = ms_per_token(53.0).expect("53 tok/s must produce an ITL");
    assert!(
        (ms - 1000.0 / 53.0).abs() < 1e-9,
        "53 tok/s → 1000/53 ms/tok, got {ms}"
    );
    assert!(
        (ms - 18.87).abs() < 0.01,
        "53 tok/s ≈ 18.87 ms/tok, got {ms}"
    );
}

/// `host_name` serde round-trip on the frontend mirror: a populated value
/// survives serialize → deserialize, `None` is omitted from the wire JSON,
/// and a payload without the field at all (old backend builds) defaults to
/// None — the same back-compat contract as the other optional fields.
#[test]
fn test_model_state_snapshot_host_name_serde_roundtrip() {
    let json = r#"{
        "id": "m1",
        "backend": "llama_cpp",
        "host_name": "gpu-box"
    }"#;
    let snap: ModelStateSnapshot =
        serde_json::from_str(json).expect("snapshot with host_name deserializes");
    assert_eq!(snap.host_name.as_deref(), Some("gpu-box"));

    let out = serde_json::to_value(&snap).expect("serialize");
    assert_eq!(
        out.get("host_name").and_then(|v| v.as_str()),
        Some("gpu-box")
    );

    let mut without = snap.clone();
    without.host_name = None;
    let out = serde_json::to_value(&without).expect("serialize");
    assert!(
        out.get("host_name").is_none(),
        "None host_name must be skipped on the wire, got: {out}"
    );

    let json2 = r#"{ "id": "m1", "backend": "llama_cpp" }"#;
    let snap2: ModelStateSnapshot =
        serde_json::from_str(json2).expect("snapshot without host_name deserializes");
    assert!(snap2.host_name.is_none());
}

/// `partition_models_by_host` groups models under their attributed host and
/// collects hostless / unmatched models into the unassigned bucket.
#[test]
fn test_partition_models_by_host_groups_and_collects_unassigned() {
    let mk = |id: &str, host: Option<&str>| ModelStateSnapshot {
        id: id.into(),
        backend: "llama_cpp".into(),
        state: ModelState::Ready,
        host_name: host.map(str::to_string),
        ..Default::default()
    };
    let models = vec![
        mk("a", Some("gpu-box")),
        mk("b", Some("cpu-box")),
        mk("c", Some("gpu-box")),
        mk("d", None),               // hostless → unassigned
        mk("e", Some("ghost-host")), // matches no host → unassigned
    ];
    let host_names = vec!["gpu-box".to_string(), "cpu-box".to_string()];

    let (by_host, unassigned) = partition_models_by_host(models, &host_names);

    let gpu_box: Vec<&str> = by_host["gpu-box"].iter().map(|m| m.id.as_str()).collect();
    assert_eq!(gpu_box, vec!["a", "c"], "order within a host is preserved");
    let cpu_box: Vec<&str> = by_host["cpu-box"].iter().map(|m| m.id.as_str()).collect();
    assert_eq!(cpu_box, vec!["b"]);
    let unassigned_ids: Vec<&str> = unassigned.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(unassigned_ids, vec!["d", "e"]);
}

/// Every known host gets a (possibly empty) bucket so the dashboard can
/// render a card per host without a missing-key branch.
#[test]
fn test_partition_models_by_host_empty_buckets_for_idle_hosts() {
    let host_names = vec!["gpu-box".to_string(), "idle-box".to_string()];
    let (by_host, unassigned) = partition_models_by_host(Vec::new(), &host_names);

    assert_eq!(by_host.len(), 2);
    assert!(by_host["gpu-box"].is_empty());
    assert!(by_host["idle-box"].is_empty());
    assert!(unassigned.is_empty());
}

/// `host_gpus_to_device_stats` converts the SSE `HostGpu` shape into the
/// `GpuDeviceStats` shape the GPU-allocation chip resolver expects.
#[test]
fn test_host_gpus_to_device_stats_conversion() {
    let host_gpus = vec![HostGpu {
        index: 1,
        name: "Radeon Pro W7900".into(),
        driver_version: String::new(),
        vram_total_bytes: 32 * 1024 * 1024 * 1024,
        vram_used_bytes: 2 * 1024 * 1024 * 1024,
        utilization_percent: 55.0,
        temperature_c: 61.0,
        power_w: 120.0,
        fan_percent: 40.0,
    }];

    let stats = host_gpus_to_device_stats(&host_gpus);
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].device_id, "GPU1");
    assert_eq!(stats[0].name, "Radeon Pro W7900");
    assert_eq!(stats[0].utilization_pct, Some(55));
    let vram = stats[0].vram.as_ref().expect("vram present when total > 0");
    assert_eq!(vram.total_mib, 32 * 1024);
    assert_eq!(vram.used_mib, 2 * 1024);

    // Zero total VRAM → no vram info (unknown), never a divide-by-zero row.
    let no_vram = host_gpus_to_device_stats(&[HostGpu {
        vram_total_bytes: 0,
        ..host_gpus[0].clone()
    }]);
    assert!(no_vram[0].vram.is_none());
}

/// `HostGpu` round-trips the additive `fan_percent` field (0-100 fan duty
/// cycle) through serde, and payloads from older backends that lack the
/// key default to 0 — same back-compat contract as the other `hosts[]`
/// fields.
#[test]
fn test_host_gpu_fan_percent_round_trip() {
    let mut gpu = HostGpu {
        name: "Radeon AI PRO R9700".to_string(),
        power_w: 47.0,
        temperature_c: 43.0,
        ..Default::default()
    };
    gpu.fan_percent = 40.1;

    let json = serde_json::to_string(&gpu).expect("HostGpu serializes");
    assert!(
        json.contains("\"fan_percent\":40.1"),
        "fan_percent must serialize: {json}"
    );
    let back: HostGpu = serde_json::from_str(&json).expect("HostGpu deserializes");
    assert_eq!(back.fan_percent, 40.1);
    assert_eq!(back.power_w, 47.0);

    // Old backend without the field — serde(default) gives 0, not an
    // error.
    let old: HostGpu = serde_json::from_value(serde_json::json!({
        "name": "legacy",
        "power_w": 30.0
    }))
    .expect("HostGpu without fan_percent must deserialize via #[serde(default)]");
    assert_eq!(old.fan_percent, 0.0);
    assert_eq!(old.power_w, 30.0);
}
