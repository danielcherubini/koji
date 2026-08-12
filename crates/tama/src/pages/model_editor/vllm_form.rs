use crate::pages::model_editor::types::{VllmSettings, VllmSpecForm};

/// Managed flag names that vLLM settings control.
const MANAGED_FLAGS: &[&str] = &[
    "--quantization",
    "--kv-cache-dtype",
    "--tensor-parallel-size",
    "--gpu-memory-utilization",
    "--max-model-len",
    "--max-num-batched-tokens",
    "--enable-prefix-caching",
    "--trust-remote-code",
    "--attention-backend",
    "--speculative-config",
];

/// Parse newline-joined args into a `VllmSettings` form.
///
/// Extracts known vLLM flags in all three stored forms, ignoring
/// unknown/unmanaged flags:
///   - grouped:  `--flag value` on one line
///   - inline:   `--flag=value`
///   - flattened: `--flag` and its value on separate lines (one token per line)
pub fn args_to_vllm_form(args: &str) -> VllmSettings {
    let mut form = VllmSettings::default();
    let lines: Vec<&str> = args.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        i += 1;
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // --flag=value (single line)
        if let Some((flag, value)) = trimmed.split_once('=') {
            if MANAGED_FLAGS.contains(&flag) && !is_boolean_flag(flag) {
                parse_flag_into_form(flag, value, &mut form);
                continue;
            }
        }

        // Exact flag match: boolean flag, or value on the next line (flattened).
        if MANAGED_FLAGS.contains(&trimmed) {
            if is_boolean_flag(trimmed) {
                parse_flag_into_form(trimmed, "", &mut form);
            } else if let Some(next) = lines.get(i) {
                let value = next.trim();
                if !value.is_empty() && !value.starts_with("--") {
                    parse_flag_into_form(trimmed, value, &mut form);
                    i += 1; // consume the value line
                }
            }
            continue;
        }

        // --flag value (same line)
        for &flag in MANAGED_FLAGS {
            if let Some(rest) = trimmed.strip_prefix(flag) {
                if rest.starts_with(' ') {
                    parse_flag_into_form(flag, rest.trim(), &mut form);
                    break;
                }
            }
        }
    }

    form
}

/// Parse a single managed flag value into the appropriate field.
fn parse_flag_into_form(flag: &str, value: &str, form: &mut VllmSettings) {
    let value = value.trim();
    match flag {
        // String fields: treat empty-after-trim as None; reject values with
        // internal whitespace to avoid token-splitting on round-trip.
        "--quantization" => {
            if !value.is_empty() && !value.contains(char::is_whitespace) {
                form.quantization = Some(value.to_string());
            } else {
                form.quantization = None;
            }
        }
        "--kv-cache-dtype" => {
            if !value.is_empty() && !value.contains(char::is_whitespace) {
                form.kv_cache_dtype = Some(value.to_string());
            } else {
                form.kv_cache_dtype = None;
            }
        }
        "--tensor-parallel-size" => {
            if let Ok(v) = value.parse::<u32>() {
                form.tensor_parallel_size = Some(v);
            }
        }
        "--gpu-memory-utilization" => {
            if let Ok(v) = value.parse::<f64>() {
                form.gpu_memory_utilization = Some(v);
            }
        }
        "--max-model-len" => {
            if let Ok(v) = value.parse::<u32>() {
                form.max_model_len = Some(v);
            }
        }
        "--max-num-batched-tokens" => {
            if let Ok(v) = value.parse::<u32>() {
                form.max_num_batched_tokens = Some(v);
            }
        }
        "--enable-prefix-caching" => {
            form.enable_prefix_caching = true;
        }
        "--trust-remote-code" => {
            form.trust_remote_code = true;
        }
        "--attention-backend" => {
            if !value.is_empty() && !value.contains(char::is_whitespace) {
                form.attention_backend = Some(value.to_string());
            } else {
                form.attention_backend = None;
            }
        }
        "--speculative-config" => {
            // Strip surrounding quotes from JSON value (common shell pattern)
            let json = value.trim().trim_matches(|c| c == '\'' || c == '"');
            if !json.is_empty() {
                if let Ok(spec) = serde_json::from_str::<VllmSpecForm>(json) {
                    form.spec_decoding = spec;
                }
                // On parse failure, do nothing (preserves malformed JSON in args)
            }
        }
        _ => {}
    }
}

/// Check if a flag is a boolean flag (no value expected).
fn is_boolean_flag(flag: &str) -> bool {
    matches!(flag, "--enable-prefix-caching" | "--trust-remote-code")
}

/// Check if a value for a given managed flag can be parsed successfully.
fn can_parse_managed_value(flag: &str, value: &str) -> bool {
    match flag {
        "--tensor-parallel-size" | "--max-model-len" | "--max-num-batched-tokens" => {
            value.parse::<u32>().is_ok()
        }
        "--gpu-memory-utilization" => value.parse::<f64>().is_ok(),
        "--attention-backend" => !value.is_empty() && !value.contains(char::is_whitespace),
        "--speculative-config" => {
            let json = value.trim().trim_matches(|c| c == '\'' || c == '"');
            !json.is_empty() && serde_json::from_str::<VllmSpecForm>(json).is_ok()
        }
        // String flags and boolean flags are always parseable
        _ => true,
    }
}

/// Classify a line as a managed vLLM flag for stripping.
///
/// Returns `Some(consumes_next_line)` when the line is a managed flag whose
/// value parses successfully — unparseable values return `None` so they are
/// preserved rather than silently dropped. `consumes_next_line` is true when
/// the value lives on the following line (flattened one-token-per-line format).
fn classify_managed_line(lines: &[&str], idx: usize) -> Option<bool> {
    let trimmed = lines[idx].trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    // --flag=value (single line)
    if let Some((flag, value)) = trimmed.split_once('=') {
        if MANAGED_FLAGS.contains(&flag) && !is_boolean_flag(flag) {
            return can_parse_managed_value(flag, value).then_some(false);
        }
    }

    // Exact flag match: boolean flag, or value on the next line (flattened).
    if MANAGED_FLAGS.contains(&trimmed) {
        if is_boolean_flag(trimmed) {
            return Some(false);
        }
        if let Some(next) = lines.get(idx + 1) {
            let value = next.trim();
            if !value.is_empty()
                && !value.starts_with("--")
                && can_parse_managed_value(trimmed, value)
            {
                return Some(true);
            }
        }
        return None;
    }

    // --flag value (same line)
    for &flag in MANAGED_FLAGS {
        if let Some(rest) = trimmed.strip_prefix(flag) {
            if rest.starts_with(' ') {
                return can_parse_managed_value(flag, rest.trim()).then_some(false);
            }
        }
    }
    None
}

/// Remove vLLM-managed flag lines from existing args, preserving user free-form args.
///
/// Walks lines and drops managed flag lines (detected via `classify_managed_line`).
/// Unparseable managed flag values are preserved rather than silently dropped.
pub fn strip_managed_flags(existing: &str) -> String {
    let lines: Vec<&str> = existing.lines().collect();
    let mut kept_lines: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match classify_managed_line(&lines, i) {
            Some(consumes_next) => {
                i += if consumes_next { 2 } else { 1 };
            }
            None => {
                kept_lines.push(lines[i]);
                i += 1;
            }
        }
    }

    // Rejoin and clean up trailing newlines
    kept_lines.join("\n")
}

/// Merge two `VllmSettings` structs, preferring non-default values from `existing`.
///
/// `existing` represents the typed column values loaded from the API.
/// `extracted` represents values parsed from the args string.
/// Per-field: if `existing` has a non-default value, it wins; otherwise `extracted` fills the gap.
pub fn merge_vllm_settings(existing: &VllmSettings, extracted: &VllmSettings) -> VllmSettings {
    VllmSettings {
        quantization: existing
            .quantization
            .clone()
            .or_else(|| extracted.quantization.clone()),
        kv_cache_dtype: existing
            .kv_cache_dtype
            .clone()
            .or_else(|| extracted.kv_cache_dtype.clone()),
        tensor_parallel_size: existing
            .tensor_parallel_size
            .or(extracted.tensor_parallel_size),
        gpu_memory_utilization: existing
            .gpu_memory_utilization
            .or(extracted.gpu_memory_utilization),
        max_model_len: existing.max_model_len.or(extracted.max_model_len),
        max_num_batched_tokens: existing
            .max_num_batched_tokens
            .or(extracted.max_num_batched_tokens),
        // Boolean fields: true is the "user set" value, false is the default
        enable_prefix_caching: existing.enable_prefix_caching || extracted.enable_prefix_caching,
        trust_remote_code: existing.trust_remote_code || extracted.trust_remote_code,
        attention_backend: existing
            .attention_backend
            .clone()
            .or_else(|| extracted.attention_backend.clone()),
        spec_decoding: merge_vllm_spec_settings(&existing.spec_decoding, &extracted.spec_decoding),
    }
}

/// Merge two `VllmSpecForm` structs, preferring non-default values from `existing`.
fn merge_vllm_spec_settings(existing: &VllmSpecForm, extracted: &VllmSpecForm) -> VllmSpecForm {
    VllmSpecForm {
        method: existing.method.clone().or_else(|| extracted.method.clone()),
        model: existing.model.clone().or_else(|| extracted.model.clone()),
        num_speculative_tokens: existing
            .num_speculative_tokens
            .or(extracted.num_speculative_tokens),
        rejection_sample_method: existing
            .rejection_sample_method
            .clone()
            .or_else(|| extracted.rejection_sample_method.clone()),
        draft_tensor_parallel_size: existing
            .draft_tensor_parallel_size
            .or(extracted.draft_tensor_parallel_size),
        draft_sample_method: existing
            .draft_sample_method
            .clone()
            .or_else(|| extracted.draft_sample_method.clone()),
        disable_padded_drafter_batch: existing
            .disable_padded_drafter_batch
            .or(extracted.disable_padded_drafter_batch),
    }
}

/// Normalize and validate vLLM speculative decoding settings before saving.
///
/// - **None / empty method**: clears the entire `spec_decoding` to default.
/// - **mtp / ngram**: clears `model` (no drafter needed).
/// - **dflash / eagle3 / draft_model**: requires `model` to be set — returns `Err` if missing.
/// - **unknown method**: passthrough (no changes).
/// - **num_speculative_tokens**: defaults to 5 if method is set and tokens not specified;
///   treats 0 as unset (also defaults to 5).
/// - **draft_tensor_parallel_size**: treats 0 as unset (clears to None).
pub fn normalize_vllm_spec(vllm: &mut VllmSettings) -> Result<(), String> {
    let spec = &vllm.spec_decoding;
    let method = spec.method.as_deref();

    match method {
        None | Some("") => {
            // Disabled: clear entire spec_decoding
            vllm.spec_decoding = VllmSpecForm::default();
        }
        Some("mtp") | Some("ngram") => {
            // No drafter needed: clear model field
            vllm.spec_decoding.model = None;
        }
        Some("dflash") | Some("eagle3") | Some("draft_model")
            if spec.model.as_deref().is_none_or(|m| m.is_empty()) => {
            // Drafter required but model not set
            return Err(
                "Drafter model required for this speculative decoding method.".into(),
            );
        }
        Some("dflash") | Some("eagle3") | Some("draft_model") => {
            // Drafter required and model is set — OK
        }
        _ => {}
    }

    // Default num_speculative_tokens to 5 if method is set and tokens not specified
    // Also treat 0 as unset (vLLM rejects num_speculative_tokens: 0)
    if vllm
        .spec_decoding
        .method
        .as_deref()
        .is_some_and(|m| !m.is_empty())
    {
        match vllm.spec_decoding.num_speculative_tokens {
            None | Some(0) => {
                vllm.spec_decoding.num_speculative_tokens = Some(5);
            }
            _ => {}
        }
    }

    // Treat draft_tensor_parallel_size 0 as unset
    if vllm.spec_decoding.draft_tensor_parallel_size == Some(0) {
        vllm.spec_decoding.draft_tensor_parallel_size = None;
    }

    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── args_to_vllm_form: --flag value form ─────────────────────────────

    #[test]
    fn test_args_to_vllm_form_quantization_space() {
        let args = "--quantization fp8\n--other-flag value";
        let form = args_to_vllm_form(args);
        assert_eq!(form.quantization, Some("fp8".to_string()));
    }

    #[test]
    fn test_args_to_vllm_form_kv_cache_dtype_space() {
        let args = "--kv-cache-dtype fp8";
        let form = args_to_vllm_form(args);
        assert_eq!(form.kv_cache_dtype, Some("fp8".to_string()));
    }

    #[test]
    fn test_args_to_vllm_form_tensor_parallel() {
        let args = "--tensor-parallel-size 2";
        let form = args_to_vllm_form(args);
        assert_eq!(form.tensor_parallel_size, Some(2));
    }

    #[test]
    fn test_args_to_vllm_form_gpu_memory_utilization() {
        let args = "--gpu-memory-utilization 0.9";
        let form = args_to_vllm_form(args);
        assert_eq!(form.gpu_memory_utilization, Some(0.9));
    }

    #[test]
    fn test_args_to_vllm_form_max_model_len() {
        let args = "--max-model-len 4096";
        let form = args_to_vllm_form(args);
        assert_eq!(form.max_model_len, Some(4096));
    }

    #[test]
    fn test_args_to_vllm_form_max_num_batched_tokens() {
        let args = "--max-num-batched-tokens 2048";
        let form = args_to_vllm_form(args);
        assert_eq!(form.max_num_batched_tokens, Some(2048));
    }

    #[test]
    fn test_args_to_vllm_form_enable_prefix_caching() {
        let args = "--enable-prefix-caching";
        let form = args_to_vllm_form(args);
        assert!(form.enable_prefix_caching);
    }

    #[test]
    fn test_args_to_vllm_form_trust_remote_code() {
        let args = "--trust-remote-code";
        let form = args_to_vllm_form(args);
        assert!(form.trust_remote_code);
    }

    // ── args_to_vllm_form: --flag=value form ─────────────────────────────

    #[test]
    fn test_args_to_vllm_form_quantization_equals() {
        let args = "--quantization=awq";
        let form = args_to_vllm_form(args);
        assert_eq!(form.quantization, Some("awq".to_string()));
    }

    #[test]
    fn test_args_to_vllm_form_kv_cache_dtype_equals() {
        let args = "--kv-cache-dtype=bf16";
        let form = args_to_vllm_form(args);
        assert_eq!(form.kv_cache_dtype, Some("bf16".to_string()));
    }

    #[test]
    fn test_args_to_vllm_form_gpu_memory_equals() {
        let args = "--gpu-memory-utilization=0.75";
        let form = args_to_vllm_form(args);
        assert_eq!(form.gpu_memory_utilization, Some(0.75));
    }

    // ── args_to_vllm_form: unknown flags ignored ────────────────────────

    #[test]
    fn test_args_to_vllm_form_ignores_unknown_flags() {
        let args = "--unknown-flag value\n--another-arg foo";
        let form = args_to_vllm_form(args);
        assert_eq!(form, VllmSettings::default());
    }

    // ── VllmSettings defaults ───────────────────────────────────────────

    #[test]
    fn test_vllm_settings_defaults() {
        let form = VllmSettings::default();
        assert_eq!(form.quantization, None);
        assert_eq!(form.kv_cache_dtype, None);
        assert_eq!(form.tensor_parallel_size, None);
        assert_eq!(form.gpu_memory_utilization, None);
        assert_eq!(form.max_model_len, None);
        assert_eq!(form.max_num_batched_tokens, None);
        assert!(!form.enable_prefix_caching);
        assert!(!form.trust_remote_code);
        assert_eq!(form.attention_backend, None);
        assert_eq!(form.spec_decoding, VllmSpecForm::default());
    }

    // ── args_to_vllm_form: boolean flags default false ─────────────────

    #[test]
    fn test_args_to_vllm_form_boolean_flags_default_false() {
        let args = "--kv-cache-dtype fp8"; // no boolean flags present
        let form = args_to_vllm_form(args);
        assert!(!form.enable_prefix_caching);
        assert!(!form.trust_remote_code);
    }

    #[test]
    fn test_args_to_vllm_form_empty_string() {
        let form = args_to_vllm_form("");
        assert_eq!(form, VllmSettings::default());
    }

    #[test]
    fn test_args_to_vllm_form_comments_ignored() {
        let args = "# This is a comment\n--quantization fp8";
        let form = args_to_vllm_form(args);
        assert_eq!(form.quantization, Some("fp8".to_string()));
    }

    // ── Flattened format (one token per line, as stored in the DB) ───────

    #[test]
    fn test_args_to_vllm_form_flattened() {
        // DB stores args as Vec<String> with one token per entry; the editor
        // joins with newlines, so flag and value land on separate lines.
        let args = "/mnt/models/Qwen/Qwen3.6-27B-FP8\n\
                    --quantization\nfp8\n\
                    --kv-cache-dtype\nbf16\n\
                    --tensor-parallel-size\n2\n\
                    --gpu-memory-utilization\n0.92\n\
                    --max-num-batched-tokens\n2560\n\
                    --attention-backend\nROCM_AITER_UNIFIED_ATTN\n\
                    --enable-prefix-caching";
        let form = args_to_vllm_form(args);
        assert_eq!(form.quantization, Some("fp8".to_string()));
        assert_eq!(form.kv_cache_dtype, Some("bf16".to_string()));
        assert_eq!(form.tensor_parallel_size, Some(2));
        assert_eq!(form.gpu_memory_utilization, Some(0.92));
        assert_eq!(form.max_num_batched_tokens, Some(2560));
        assert_eq!(
            form.attention_backend,
            Some("ROCM_AITER_UNIFIED_ATTN".to_string())
        );
        assert!(form.enable_prefix_caching);
        assert!(!form.trust_remote_code);
    }

    #[test]
    fn test_args_to_vllm_form_flattened_flag_without_value() {
        // A managed flag followed by another flag must not eat the next flag
        let args = "--quantization\n--enable-prefix-caching";
        let form = args_to_vllm_form(args);
        assert_eq!(form.quantization, None);
        assert!(form.enable_prefix_caching);
    }

    // ── strip_managed_flags: grouped format ─────────────────────────────

    #[test]
    fn test_strip_managed_flags_removes_grouped_flags() {
        let args = "--batch-size 512\n--quantization fp8\n--some-other-flag";
        let result = strip_managed_flags(args);
        assert!(result.contains("--batch-size 512"));
        assert!(result.contains("--some-other-flag"));
        assert!(!result.contains("--quantization"));
        assert!(!result.contains("fp8"));
    }

    #[test]
    fn test_strip_managed_flags_preserves_free_form_args() {
        let args = "# My custom comment\n--my-custom-flag value123\n--batch 512";
        let result = strip_managed_flags(args);
        assert!(result.contains("# My custom comment"));
        assert!(result.contains("--my-custom-flag value123"));
        assert!(result.contains("--batch 512"));
    }

    #[test]
    fn test_strip_managed_flags_empty_input() {
        let result = strip_managed_flags("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_managed_flags_boolean_flags() {
        let args = "--enable-prefix-caching\n--trust-remote-code\n--custom-flag";
        let result = strip_managed_flags(args);
        assert!(!result.contains("--enable-prefix-caching"));
        assert!(!result.contains("--trust-remote-code"));
        assert!(result.contains("--custom-flag"));
    }

    #[test]
    fn test_strip_managed_flags_all_managed() {
        let args = "--quantization fp8\n--trust-remote-code";
        let result = strip_managed_flags(args);
        assert_eq!(result, "");
    }

    // ── strip_managed_flags: inline format (--flag=value) ────────────────

    #[test]
    fn test_strip_managed_flags_inline_format() {
        let args = "--quantization=awq\n--kv-cache-dtype=fp8\n--custom value";
        let result = strip_managed_flags(args);
        assert!(!result.contains("--quantization"));
        assert!(!result.contains("--kv-cache-dtype"));
        assert!(result.contains("--custom value"));
    }

    // ── strip_managed_flags: flattened format ────────────────────────────

    #[test]
    fn test_strip_managed_flags_flattened() {
        let args = "/mnt/models/org/repo\n--quantization\nfp8\n--attention-backend\nROCM_AITER_UNIFIED_ATTN\n--enable-prefix-caching";
        let result = strip_managed_flags(args);
        // Old flattened flag+value pair is gone
        assert!(!result.contains("--quantization"));
        assert!(!result.contains("fp8"));
        // --attention-backend is now managed and stripped
        assert!(!result.contains("--attention-backend"));
        assert!(!result.contains("ROCM_AITER_UNIFIED_ATTN"));
        // Unmanaged lines preserved
        assert!(result.contains("/mnt/models/org/repo"));
        // Boolean flag stripped
        assert!(!result.contains("--enable-prefix-caching"));
    }

    // ── strip_managed_flags: preserves unparseable managed flags ─────────

    #[test]
    fn test_strip_managed_flags_preserves_unparseable_numeric_flag() {
        // --max-model-len with non-numeric value should be preserved, not stripped
        let args = "--batch-size 512\n--max-model-len abc\n--other-flag";
        let result = strip_managed_flags(args);
        assert!(result.contains("--batch-size 512"));
        assert!(result.contains("--max-model-len abc"));
        assert!(result.contains("--other-flag"));
    }

    #[test]
    fn test_strip_managed_flags_preserves_unparseable_gpu_memory() {
        // --gpu-memory-utilization with non-numeric value should be preserved
        let args = "--batch-size 512\n--gpu-memory-utilization notanumber";
        let result = strip_managed_flags(args);
        assert!(result.contains("--batch-size 512"));
        assert!(result.contains("--gpu-memory-utilization notanumber"));
    }

    #[test]
    fn test_strip_managed_flags_preserves_unparseable_flattened() {
        let args = "--max-model-len\nabc";
        let result = strip_managed_flags(args);
        assert!(result.contains("--max-model-len"));
        assert!(result.contains("abc"));
    }

    // ── strip_managed_flags: mixed formats ───────────────────────────────

    #[test]
    fn test_strip_managed_flags_mixed_formats() {
        let args = "--quantization fp8\n--kv-cache-dtype=auto\n--tensor-parallel-size\n2\n--custom-flag value\n--enable-prefix-caching";
        let result = strip_managed_flags(args);
        assert!(!result.contains("--quantization"));
        assert!(!result.contains("--kv-cache-dtype"));
        assert!(!result.contains("--tensor-parallel-size"));
        assert!(!result.contains("--enable-prefix-caching"));
        assert!(result.contains("--custom-flag value"));
    }

    // ── Round-trip: args → form → strip → reparse stable ────────────────

    #[test]
    fn test_roundtrip_args_strip_reparse_stable() {
        let original = "--quantization fp8\n--kv-cache-dtype auto\n--tensor-parallel-size 2\n--gpu-memory-utilization 0.85\n--max-model-len 8192\n--enable-prefix-caching\n--trust-remote-code";
        let form = args_to_vllm_form(original);
        let stripped = strip_managed_flags(original);

        // After stripping all managed flags, nothing should remain
        assert_eq!(stripped, "");

        // Verify form was parsed correctly
        assert_eq!(form.quantization, Some("fp8".to_string()));
        assert_eq!(form.kv_cache_dtype, Some("auto".to_string()));
        assert_eq!(form.tensor_parallel_size, Some(2));
        assert_eq!(form.gpu_memory_utilization, Some(0.85));
        assert_eq!(form.max_model_len, Some(8192));
        assert!(form.enable_prefix_caching);
        assert!(form.trust_remote_code);
    }

    #[test]
    fn test_roundtrip_preserves_unmanaged_after_strip() {
        let original = "--batch-size 512\n--quantization fp8\n--my-custom-flag";
        let form = args_to_vllm_form(original);
        let stripped = strip_managed_flags(original);

        assert!(stripped.contains("--batch-size 512"));
        assert!(stripped.contains("--my-custom-flag"));
        assert!(!stripped.contains("--quantization"));

        // Parsed form still captures the managed flag
        assert_eq!(form.quantization, Some("fp8".to_string()));
    }

    // ── --attention-backend: now a managed flag ──────────────────────────

    #[test]
    fn test_args_to_vllm_form_attention_backend_grouped() {
        let args = "--attention-backend ROCM_AITER_UNIFIED_ATTN";
        let form = args_to_vllm_form(args);
        assert_eq!(
            form.attention_backend,
            Some("ROCM_AITER_UNIFIED_ATTN".to_string())
        );
    }

    #[test]
    fn test_args_to_vllm_form_attention_backend_inline() {
        let args = "--attention-backend=FLASH_ATTN";
        let form = args_to_vllm_form(args);
        assert_eq!(form.attention_backend, Some("FLASH_ATTN".to_string()));
    }

    #[test]
    fn test_args_to_vllm_form_attention_backend_flattened() {
        let args = "--attention-backend\nROCM_AITER_UNIFIED_ATTN";
        let form = args_to_vllm_form(args);
        assert_eq!(
            form.attention_backend,
            Some("ROCM_AITER_UNIFIED_ATTN".to_string())
        );
    }

    #[test]
    fn test_args_to_vllm_form_attention_backend_whitespace_rejected() {
        // Values with whitespace are rejected (would be ambiguous on round-trip)
        let args = "--attention-backend some value with spaces";
        let form = args_to_vllm_form(args);
        assert_eq!(form.attention_backend, None);
    }

    // ── --speculative-config: JSON parsing ──────────────────────────────

    #[test]
    fn test_args_to_vllm_form_speculative_config_grouped() {
        let args = r#"--speculative-config {"method":"mtp","num_speculative_tokens":8}"#;
        let form = args_to_vllm_form(args);
        assert_eq!(form.spec_decoding.method, Some("mtp".to_string()));
        assert_eq!(form.spec_decoding.num_speculative_tokens, Some(8));
    }

    #[test]
    fn test_args_to_vllm_form_speculative_config_inline() {
        let args = r#"--speculative-config={"method":"eagle","num_speculative_tokens":4}"#;
        let form = args_to_vllm_form(args);
        assert_eq!(form.spec_decoding.method, Some("eagle".to_string()));
        assert_eq!(form.spec_decoding.num_speculative_tokens, Some(4));
    }

    #[test]
    fn test_args_to_vllm_form_speculative_config_flattened() {
        let args = r#"--speculative-config
{"method":"mtp","num_speculative_tokens":16}"#;
        let form = args_to_vllm_form(args);
        assert_eq!(form.spec_decoding.method, Some("mtp".to_string()));
        assert_eq!(form.spec_decoding.num_speculative_tokens, Some(16));
    }

    #[test]
    fn test_args_to_vllm_form_speculative_config_quoted_json() {
        // JSON value wrapped in single quotes (common shell pattern)
        let args = r#"--speculative-config '{"method":"mtp"}'"#;
        let form = args_to_vllm_form(args);
        assert_eq!(form.spec_decoding.method, Some("mtp".to_string()));
    }

    #[test]
    fn test_args_to_vllm_form_speculative_config_malformed_json_ignored() {
        // Malformed JSON should not crash and should leave spec_decoding default
        let args = r#"--speculative-config {not valid json}"#;
        let form = args_to_vllm_form(args);
        assert_eq!(form.spec_decoding.method, None);
    }

    #[test]
    fn test_args_to_vllm_form_speculative_config_all_fields() {
        let args = r#"--speculative-config {"method":"eagle","model":"draft.gguf","num_speculative_tokens":8,"rejection_sample_method":"top_k","draft_tensor_parallel_size":2,"draft_sample_method":"greedy","disable_padded_drafter_batch":true}"#;
        let form = args_to_vllm_form(args);
        assert_eq!(form.spec_decoding.method, Some("eagle".to_string()));
        assert_eq!(form.spec_decoding.model, Some("draft.gguf".to_string()));
        assert_eq!(form.spec_decoding.num_speculative_tokens, Some(8));
        assert_eq!(
            form.spec_decoding.rejection_sample_method,
            Some("top_k".to_string())
        );
        assert_eq!(form.spec_decoding.draft_tensor_parallel_size, Some(2));
        assert_eq!(
            form.spec_decoding.draft_sample_method,
            Some("greedy".to_string())
        );
        assert_eq!(form.spec_decoding.disable_padded_drafter_batch, Some(true));
    }

    // ── strip_managed_flags: --attention-backend ────────────────────────

    #[test]
    fn test_strip_managed_flags_removes_attention_backend_grouped() {
        let args = "--attention-backend ROCM_AITER_UNIFIED_ATTN\n--custom-flag";
        let result = strip_managed_flags(args);
        assert!(!result.contains("--attention-backend"));
        assert!(!result.contains("ROCM_AITER_UNIFIED_ATTN"));
        assert!(result.contains("--custom-flag"));
    }

    #[test]
    fn test_strip_managed_flags_removes_attention_backend_flattened() {
        let args = "--attention-backend\nROCM_AITER_UNIFIED_ATTN\n--custom-flag";
        let result = strip_managed_flags(args);
        assert!(!result.contains("--attention-backend"));
        assert!(!result.contains("ROCM_AITER_UNIFIED_ATTN"));
        assert!(result.contains("--custom-flag"));
    }

    // ── strip_managed_flags: --speculative-config ───────────────────────

    #[test]
    fn test_strip_managed_flags_removes_speculative_config_grouped() {
        let args = "--speculative-config {\"method\":\"mtp\"}\n--custom-flag";
        let result = strip_managed_flags(args);
        assert!(!result.contains("--speculative-config"));
        assert!(!result.contains("\"method\""));
        assert!(result.contains("--custom-flag"));
    }

    #[test]
    fn test_strip_managed_flags_removes_speculative_config_flattened() {
        let args = r#"--speculative-config
{"method":"mtp","num_speculative_tokens":8}
--custom-flag"#;
        let result = strip_managed_flags(args);
        assert!(!result.contains("--speculative-config"));
        assert!(!result.contains("\"method\""));
        assert!(result.contains("--custom-flag"));
    }

    #[test]
    fn test_strip_managed_flags_preserves_malformed_speculative_config() {
        // Malformed JSON should NOT be stripped (preserved for safety)
        let args = "--speculative-config {not valid json}\n--custom-flag";
        let result = strip_managed_flags(args);
        assert!(result.contains("--speculative-config"));
        assert!(result.contains("not valid json"));
        assert!(result.contains("--custom-flag"));
    }

    // ── merge_vllm_settings: spec_decoding and attention_backend ────────

    #[test]
    fn test_merge_vllm_settings_attention_backend_existing_wins() {
        let existing = VllmSettings {
            attention_backend: Some("FLASH_ATTN".to_string()),
            ..Default::default()
        };
        let extracted = VllmSettings {
            attention_backend: Some("ROCM_AITER_UNIFIED_ATTN".to_string()),
            ..Default::default()
        };
        let merged = merge_vllm_settings(&existing, &extracted);
        assert_eq!(merged.attention_backend, Some("FLASH_ATTN".to_string()));
    }

    #[test]
    fn test_merge_vllm_settings_attention_backend_extracted_fills() {
        let existing = VllmSettings::default();
        let extracted = VllmSettings {
            attention_backend: Some("FLASH_ATTN".to_string()),
            ..Default::default()
        };
        let merged = merge_vllm_settings(&existing, &extracted);
        assert_eq!(merged.attention_backend, Some("FLASH_ATTN".to_string()));
    }

    #[test]
    fn test_merge_vllm_settings_spec_decoding_existing_wins() {
        use crate::pages::model_editor::types::VllmSpecForm;
        let existing = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("mtp".to_string()),
                num_speculative_tokens: Some(8),
                ..Default::default()
            },
            ..Default::default()
        };
        let extracted = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("eagle".to_string()),
                num_speculative_tokens: Some(4),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge_vllm_settings(&existing, &extracted);
        // Existing non-default values preserved
        assert_eq!(merged.spec_decoding.method, Some("mtp".to_string()));
        assert_eq!(merged.spec_decoding.num_speculative_tokens, Some(8));
    }

    #[test]
    fn test_merge_vllm_settings_spec_decoding_extracted_fills_gaps() {
        use crate::pages::model_editor::types::VllmSpecForm;
        let existing = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("mtp".to_string()),
                // num_speculative_tokens is None (default)
                ..Default::default()
            },
            ..Default::default()
        };
        let extracted = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("eagle".to_string()),
                num_speculative_tokens: Some(16),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge_vllm_settings(&existing, &extracted);
        // Existing method preserved, extracted num_speculative_tokens fills gap
        assert_eq!(merged.spec_decoding.method, Some("mtp".to_string()));
        assert_eq!(merged.spec_decoding.num_speculative_tokens, Some(16));
    }

    #[test]
    fn test_merge_vllm_settings_spec_decoding_both_defaults() {
        let existing = VllmSettings::default();
        let extracted = VllmSettings::default();
        let merged = merge_vllm_settings(&existing, &extracted);
        assert_eq!(merged.spec_decoding, VllmSpecForm::default());
    }

    #[test]
    fn test_merge_vllm_settings_existing_wins_for_option_fields() {
        let existing = VllmSettings {
            quantization: Some("fp8".to_string()),
            kv_cache_dtype: Some("bf16".to_string()),
            tensor_parallel_size: Some(2),
            gpu_memory_utilization: Some(0.9),
            max_model_len: Some(8192),
            max_num_batched_tokens: Some(4096),
            ..Default::default()
        };
        let extracted = VllmSettings {
            quantization: Some("awq".to_string()),
            kv_cache_dtype: Some("auto".to_string()),
            tensor_parallel_size: Some(4),
            gpu_memory_utilization: Some(0.75),
            max_model_len: Some(4096),
            max_num_batched_tokens: Some(2048),
            ..Default::default()
        };
        let merged = merge_vllm_settings(&existing, &extracted);

        // All existing values should be preserved
        assert_eq!(merged.quantization, Some("fp8".to_string()));
        assert_eq!(merged.kv_cache_dtype, Some("bf16".to_string()));
        assert_eq!(merged.tensor_parallel_size, Some(2));
        assert_eq!(merged.gpu_memory_utilization, Some(0.9));
        assert_eq!(merged.max_model_len, Some(8192));
        assert_eq!(merged.max_num_batched_tokens, Some(4096));
    }

    #[test]
    fn test_merge_vllm_settings_extracted_fills_gaps() {
        let existing = VllmSettings {
            quantization: Some("fp8".to_string()),
            // Rest are defaults (None)
            ..Default::default()
        };
        let extracted = VllmSettings {
            quantization: Some("awq".to_string()),
            kv_cache_dtype: Some("auto".to_string()),
            tensor_parallel_size: Some(4),
            gpu_memory_utilization: Some(0.75),
            enable_prefix_caching: true,
            ..Default::default()
        };
        let merged = merge_vllm_settings(&existing, &extracted);

        // Existing value preserved
        assert_eq!(merged.quantization, Some("fp8".to_string()));
        // Gaps filled from extracted
        assert_eq!(merged.kv_cache_dtype, Some("auto".to_string()));
        assert_eq!(merged.tensor_parallel_size, Some(4));
        assert_eq!(merged.gpu_memory_utilization, Some(0.75));
        assert!(merged.enable_prefix_caching);
    }

    #[test]
    fn test_merge_vllm_settings_boolean_fields_or_logic() {
        let existing = VllmSettings {
            enable_prefix_caching: true,
            trust_remote_code: false,
            ..Default::default()
        };
        let extracted = VllmSettings {
            enable_prefix_caching: false,
            trust_remote_code: true,
            ..Default::default()
        };
        let merged = merge_vllm_settings(&existing, &extracted);

        // true || false = true (existing wins)
        assert!(merged.enable_prefix_caching);
        // false || true = true (extracted fills)
        assert!(merged.trust_remote_code);
    }

    #[test]
    fn test_merge_vllm_settings_both_defaults() {
        let existing = VllmSettings::default();
        let extracted = VllmSettings::default();
        let merged = merge_vllm_settings(&existing, &extracted);
        assert_eq!(merged, VllmSettings::default());
    }

    #[test]
    fn test_merge_vllm_settings_steady_state_no_data_loss() {
        // Simulates the steady-state scenario: args are empty (stripped),
        // so extracted is all defaults. Existing typed values must survive.
        let existing = VllmSettings {
            quantization: Some("fp8".to_string()),
            tensor_parallel_size: Some(2),
            gpu_memory_utilization: Some(0.92),
            enable_prefix_caching: true,
            ..Default::default()
        };
        let extracted = VllmSettings::default(); // args were stripped
        let merged = merge_vllm_settings(&existing, &extracted);

        // All typed values preserved — no data loss!
        assert_eq!(merged.quantization, Some("fp8".to_string()));
        assert_eq!(merged.tensor_parallel_size, Some(2));
        assert_eq!(merged.gpu_memory_utilization, Some(0.92));
        assert!(merged.enable_prefix_caching);
    }

    // ── Integration: round-trip tests ──────────────────────────────────

    /// Round-trip: args with `--speculative-config` → form → strip → reparse stable.
    /// The JSON must be parsed into the form, stripped from args, and not reappear.
    #[test]
    fn test_roundtrip_speculative_config() {
        let original = r#"--speculative-config {"method":"mtp","num_speculative_tokens":8}"#;

        // Parse args into form
        let form = args_to_vllm_form(original);
        assert_eq!(form.spec_decoding.method, Some("mtp".to_string()));
        assert_eq!(form.spec_decoding.num_speculative_tokens, Some(8));

        // Strip managed flags from args
        let stripped = strip_managed_flags(original);

        // After stripping, the speculative-config should be gone
        assert!(!stripped.contains("--speculative-config"));
        assert!(!stripped.contains("\"method\""));

        // Reparsing the stripped args should yield defaults for spec_decoding
        let re_form = args_to_vllm_form(&stripped);
        assert_eq!(re_form.spec_decoding.method, None);
        assert_eq!(re_form.spec_decoding.num_speculative_tokens, None);
    }

    /// Mixed args: `--quantization fp8` + `--speculative-config {...}` → both parsed correctly.
    #[test]
    fn test_parse_mixed_quantization_and_speculative_config() {
        let original = r#"--quantization fp8
--speculative-config {"method":"eagle","model":"draft.gguf","num_speculative_tokens":4}"#;

        let form = args_to_vllm_form(original);

        // Both flags parsed correctly
        assert_eq!(form.quantization, Some("fp8".to_string()));
        assert_eq!(form.spec_decoding.method, Some("eagle".to_string()));
        assert_eq!(form.spec_decoding.model, Some("draft.gguf".to_string()));
        assert_eq!(form.spec_decoding.num_speculative_tokens, Some(4));

        // Both stripped from args
        let stripped = strip_managed_flags(original);
        assert!(!stripped.contains("--quantization"));
        assert!(!stripped.contains("--speculative-config"));
        assert!(!stripped.contains("fp8"));
        assert!(!stripped.contains("\"method\""));
    }

    /// `--attention-backend` round-trip: args → form → strip → reparse stable.
    #[test]
    fn test_roundtrip_attention_backend() {
        let original = "--attention-backend ROCM_AITER_UNIFIED_ATTN";

        // Parse args into form
        let form = args_to_vllm_form(original);
        assert_eq!(
            form.attention_backend,
            Some("ROCM_AITER_UNIFIED_ATTN".to_string())
        );

        // Strip managed flags from args
        let stripped = strip_managed_flags(original);

        // After stripping, the attention-backend should be gone
        assert!(!stripped.contains("--attention-backend"));
        assert!(!stripped.contains("ROCM_AITER_UNIFIED_ATTN"));

        // Reparsing the stripped args should yield defaults for attention_backend
        let re_form = args_to_vllm_form(&stripped);
        assert_eq!(re_form.attention_backend, None);
    }

    /// Malformed JSON: `--speculative-config '{bad'` → preserved in args (not stripped).
    /// Unparseable managed flag values are preserved rather than silently dropped.
    #[test]
    fn test_malformed_speculative_config_preserved_in_args() {
        let original = "--speculative-config '{bad'";

        // Parse args — malformed JSON should leave spec_decoding at defaults
        let form = args_to_vllm_form(original);
        assert_eq!(form.spec_decoding.method, None);

        // Strip managed flags — malformed JSON should NOT be stripped
        let stripped = strip_managed_flags(original);
        assert!(stripped.contains("--speculative-config"));
        assert!(stripped.contains("bad"));
    }

    // ── VllmSpecForm: deny_unknown_fields ──────────────────────────────

    /// Unknown keys in `--speculative-config` JSON cause parse failure,
    /// so the flag is preserved in args (not silently stripped).
    #[test]
    fn test_unknown_keys_in_speculative_config_preserved() {
        let original = r#"--speculative-config {"method":"mtp","unknown_key":1}"#;

        // Parse fails because VllmSpecForm denies unknown fields
        let form = args_to_vllm_form(original);
        assert_eq!(form.spec_decoding.method, None);

        // Flag is preserved in args (not stripped)
        let stripped = strip_managed_flags(original);
        assert!(stripped.contains("--speculative-config"));
        assert!(stripped.contains("unknown_key"));
    }

    // ── normalize_vllm_spec tests ──────────────────────────────────────

    /// None method: clears entire spec_decoding to default.
    #[test]
    fn test_normalize_vllm_spec_none_clears_all() {
        let mut vllm = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: None,
                num_speculative_tokens: Some(8),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = normalize_vllm_spec(&mut vllm);
        assert!(result.is_ok());
        assert_eq!(vllm.spec_decoding, VllmSpecForm::default());
    }

    /// Empty string method: clears entire spec_decoding to default.
    #[test]
    fn test_normalize_vllm_spec_empty_string_clears_all() {
        let mut vllm = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("".to_string()),
                num_speculative_tokens: Some(8),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = normalize_vllm_spec(&mut vllm);
        assert!(result.is_ok());
        assert_eq!(vllm.spec_decoding, VllmSpecForm::default());
    }

    /// mtp method: clears model field, defaults num_speculative_tokens to 5.
    #[test]
    fn test_normalize_vllm_spec_mtp_clears_model() {
        let mut vllm = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("mtp".to_string()),
                model: Some("should-be-cleared.gguf".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = normalize_vllm_spec(&mut vllm);
        assert!(result.is_ok());
        assert_eq!(vllm.spec_decoding.model, None);
        assert_eq!(vllm.spec_decoding.num_speculative_tokens, Some(5));
    }

    /// ngram method: clears model field, defaults num_speculative_tokens to 5.
    #[test]
    fn test_normalize_vllm_spec_ngram_clears_model() {
        let mut vllm = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("ngram".to_string()),
                model: Some("should-be-cleared.gguf".to_string()),
                num_speculative_tokens: Some(3),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = normalize_vllm_spec(&mut vllm);
        assert!(result.is_ok());
        assert_eq!(vllm.spec_decoding.model, None);
        // Existing num_speculative_tokens preserved
        assert_eq!(vllm.spec_decoding.num_speculative_tokens, Some(3));
    }

    /// eagle3 without model: returns Err.
    #[test]
    fn test_normalize_vllm_spec_eagle3_no_model_error() {
        let mut vllm = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("eagle3".to_string()),
                model: None,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = normalize_vllm_spec(&mut vllm);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Drafter model required"));
    }

    /// eagle3 with model: OK, defaults num_speculative_tokens to 5.
    #[test]
    fn test_normalize_vllm_spec_eagle3_with_model_ok() {
        let mut vllm = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("eagle3".to_string()),
                model: Some("draft-model.gguf".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = normalize_vllm_spec(&mut vllm);
        assert!(result.is_ok());
        assert_eq!(
            vllm.spec_decoding.model,
            Some("draft-model.gguf".to_string())
        );
        assert_eq!(vllm.spec_decoding.num_speculative_tokens, Some(5));
    }

    /// Unknown method: passthrough, no changes.
    #[test]
    fn test_normalize_vllm_spec_unknown_method_passthrough() {
        let mut vllm = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("unknown_method".to_string()),
                model: Some("some-model".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = normalize_vllm_spec(&mut vllm);
        assert!(result.is_ok());
        // No changes made
        assert_eq!(
            vllm.spec_decoding.method,
            Some("unknown_method".to_string())
        );
        assert_eq!(vllm.spec_decoding.model, Some("some-model".to_string()));
    }

    /// num_speculative_tokens = 0 treated as unset, defaults to 5.
    #[test]
    fn test_normalize_vllm_spec_zero_tokens_defaults_to_5() {
        let mut vllm = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("mtp".to_string()),
                num_speculative_tokens: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = normalize_vllm_spec(&mut vllm);
        assert!(result.is_ok());
        assert_eq!(vllm.spec_decoding.num_speculative_tokens, Some(5));
    }

    /// draft_tensor_parallel_size = 0 treated as unset, cleared to None.
    #[test]
    fn test_normalize_vllm_spec_zero_draft_tp_cleared() {
        let mut vllm = VllmSettings {
            spec_decoding: VllmSpecForm {
                method: Some("eagle3".to_string()),
                model: Some("draft.gguf".to_string()),
                draft_tensor_parallel_size: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = normalize_vllm_spec(&mut vllm);
        assert!(result.is_ok());
        assert_eq!(vllm.spec_decoding.draft_tensor_parallel_size, None);
    }
}
