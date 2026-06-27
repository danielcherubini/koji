pub mod detect;
pub mod discover;
pub mod system;
pub mod vram;

// Re-export all public items for backward compatibility
pub use detect::{
    detect_amdgpu_targets, detect_build_prerequisites, detect_cuda_version,
    parse_rocminfo_gfx_names, suggest_context_sizes, BuildPrerequisites, ContextSuggestion,
    GpuType, DEFAULT_CUDA_VERSION,
};
pub use discover::{discover_devices_via_binary, parse_llama_list_devices_output, GpuDeviceInfo};
pub use system::{
    collect_system_metrics, collect_system_metrics_with, GpuDeviceStats, MetricCurrent,
    MetricHistoryPoint, MetricSample, MetricsSnapshot, ModelStatus, SystemMetrics,
};
pub use vram::{query_vram, query_vram_per_device, VramInfo};
