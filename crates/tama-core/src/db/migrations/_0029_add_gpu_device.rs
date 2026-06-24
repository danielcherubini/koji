/// v29 - Add gpu_device column to model_configs.
/// Stores the GPU device name (e.g. "ROCm0", "CUDA1") for per-model GPU placement.
/// Passed as `--device` to llama.cpp backends.
pub const MIGRATION: (i32, bool, &str) = (
    29,
    false,
    r#"ALTER TABLE model_configs ADD COLUMN gpu_device TEXT;"#,
);
