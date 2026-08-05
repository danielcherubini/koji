use serde::{Deserialize, Serialize};

/// vLLM-specific settings for transformers-format models.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VllmSettings {
    pub quantization: Option<String>, // none/fp8/awq (free-form allowed)
    pub kv_cache_dtype: Option<String>, // auto/fp8/bf16
    pub tensor_parallel_size: Option<u32>, // default 1
    pub gpu_memory_utilization: Option<f64>, // 0.0–1.0
    pub max_model_len: Option<u32>,
    pub max_num_batched_tokens: Option<u32>,
    pub enable_prefix_caching: bool,
    pub trust_remote_code: bool,
}

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
                parse_managed_flag(flag, value, &mut form);
                continue;
            }
        }

        // Exact flag match: boolean flag, or value on the next line (flattened).
        if MANAGED_FLAGS.contains(&trimmed) {
            if is_boolean_flag(trimmed) {
                parse_managed_flag(trimmed, "", &mut form);
            } else if let Some(next) = lines.get(i) {
                let value = next.trim();
                if !value.is_empty() && !value.starts_with("--") {
                    parse_managed_flag(trimmed, value, &mut form);
                    i += 1; // consume the value line
                }
            }
            continue;
        }

        // --flag value (same line)
        for &flag in MANAGED_FLAGS {
            if let Some(rest) = trimmed.strip_prefix(flag) {
                if rest.starts_with(' ') {
                    parse_managed_flag(flag, rest.trim(), &mut form);
                    break;
                }
            }
        }
    }

    form
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

/// Check if a value for a given managed flag can be parsed successfully.
fn can_parse_managed_value(flag: &str, value: &str) -> bool {
    match flag {
        "--tensor-parallel-size" | "--max-model-len" | "--max-num-batched-tokens" => {
            value.parse::<u32>().is_ok()
        }
        "--gpu-memory-utilization" => value.parse::<f64>().is_ok(),
        // String flags and boolean flags are always parseable
        _ => true,
    }
}

/// Remove vLLM-managed flag lines from existing args, then append current
/// `VllmSettings` as flags. Preserves user free-form args.
pub fn vllm_form_to_args(form: &VllmSettings, existing: &str) -> String {
    // Step 1: Remove managed flags from existing args (grouped, inline, and
    // flattened forms), but only when their values parse — unparseable lines
    // are preserved rather than silently dropped.
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

    // Step 2: Build args from kept lines + current form values
    let mut result = kept_lines.join("\n");
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&build_vllm_flags(form));

    // Clean up: remove trailing empty lines, but ensure final newline
    let result = result.trim_end_matches('\n').to_string();
    if !result.is_empty() {
        format!("{result}\n")
    } else {
        String::new()
    }
}

/// Build a string of vLLM-managed flags from the form.
fn build_vllm_flags(form: &VllmSettings) -> String {
    let mut parts = Vec::new();

    if let Some(ref v) = form.quantization {
        parts.push(format!("--quantization {v}"));
    }
    if let Some(ref v) = form.kv_cache_dtype {
        parts.push(format!("--kv-cache-dtype {v}"));
    }
    if let Some(v) = form.tensor_parallel_size {
        parts.push(format!("--tensor-parallel-size {v}"));
    }
    if let Some(v) = form.gpu_memory_utilization {
        parts.push(format!("--gpu-memory-utilization {v}"));
    }
    if let Some(v) = form.max_model_len {
        parts.push(format!("--max-model-len {v}"));
    }
    if let Some(v) = form.max_num_batched_tokens {
        parts.push(format!("--max-num-batched-tokens {v}"));
    }
    if form.enable_prefix_caching {
        parts.push("--enable-prefix-caching".to_string());
    }
    if form.trust_remote_code {
        parts.push("--trust-remote-code".to_string());
    }

    parts.join("\n")
}

/// Parse a single managed flag value into the appropriate field.
fn parse_managed_flag(flag: &str, value: &str, form: &mut VllmSettings) {
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
        _ => {}
    }
}

/// Check if a flag is a boolean flag (no value expected).
fn is_boolean_flag(flag: &str) -> bool {
    matches!(flag, "--enable-prefix-caching" | "--trust-remote-code")
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

    // ── vllm_form_to_args: replaces managed flags ───────────────────────

    #[test]
    fn test_vllm_form_to_args_replaces_managed_flags() {
        let existing = "--batch-size 512\n--quantization fp8\n--some-other-flag";
        let form = VllmSettings {
            quantization: Some("awq".to_string()),
            ..Default::default()
        };
        let result = vllm_form_to_args(&form, existing);
        assert!(result.contains("--batch-size 512"));
        assert!(result.contains("--some-other-flag"));
        assert!(result.contains("--quantization awq"));
        assert!(!result.contains("fp8"));
    }

    #[test]
    fn test_vllm_form_to_args_preserves_free_form_args() {
        let existing = "# My custom comment\n--my-custom-flag value123\n--batch 512";
        let form = VllmSettings::default();
        let result = vllm_form_to_args(&form, existing);
        assert!(result.contains("# My custom comment"));
        assert!(result.contains("--my-custom-flag value123"));
        assert!(result.contains("--batch 512"));
    }

    #[test]
    fn test_vllm_form_to_args_empty_existing() {
        let existing = "";
        let form = VllmSettings {
            quantization: Some("fp8".to_string()),
            enable_prefix_caching: true,
            ..Default::default()
        };
        let result = vllm_form_to_args(&form, existing);
        assert!(result.contains("--quantization fp8"));
        assert!(result.contains("--enable-prefix-caching"));
    }

    // ── Round-trip: args → form → args stable for vLLM flags ────────────

    #[test]
    fn test_roundtrip_args_to_form_to_args_stable() {
        let original = "--quantization fp8\n--kv-cache-dtype auto\n--tensor-parallel-size 2\n--gpu-memory-utilization 0.85\n--max-model-len 8192\n--enable-prefix-caching\n--trust-remote-code";
        let form = args_to_vllm_form(original);
        let restored = vllm_form_to_args(&form, original);

        // Parse both back and compare the form values (order may differ)
        let form2 = args_to_vllm_form(&restored);
        assert_eq!(form.quantization, form2.quantization);
        assert_eq!(form.kv_cache_dtype, form2.kv_cache_dtype);
        assert_eq!(form.tensor_parallel_size, form2.tensor_parallel_size);
        assert_eq!(form.gpu_memory_utilization, form2.gpu_memory_utilization);
        assert_eq!(form.max_model_len, form2.max_model_len);
        assert_eq!(form.enable_prefix_caching, form2.enable_prefix_caching);
        assert_eq!(form.trust_remote_code, form2.trust_remote_code);
    }

    #[test]
    fn test_roundtrip_preserves_unmanaged_flags() {
        let original = "--batch-size 512\n--quantization fp8\n--my-custom-flag";
        let form = args_to_vllm_form(original);
        let restored = vllm_form_to_args(&form, original);

        assert!(restored.contains("--batch-size 512"));
        assert!(restored.contains("--my-custom-flag"));
        assert!(restored.contains("--quantization fp8"));
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
    }

    // ── parse_managed_flag: boolean flags default false ─────────────────

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

    // ── vllm_form_to_args: multiple managed flags ───────────────────────

    #[test]
    fn test_vllm_form_to_args_multiple_flags() {
        let existing = "--batch 512";
        let form = VllmSettings {
            quantization: Some("fp8".to_string()),
            kv_cache_dtype: Some("auto".to_string()),
            tensor_parallel_size: Some(2),
            gpu_memory_utilization: Some(0.9),
            max_model_len: Some(4096),
            max_num_batched_tokens: Some(2048),
            enable_prefix_caching: true,
            trust_remote_code: true,
        };
        let result = vllm_form_to_args(&form, existing);

        assert!(result.contains("--batch 512"));
        assert!(result.contains("--quantization fp8"));
        assert!(result.contains("--kv-cache-dtype auto"));
        assert!(result.contains("--tensor-parallel-size 2"));
        assert!(result.contains("--gpu-memory-utilization 0.9"));
        assert!(result.contains("--max-model-len 4096"));
        assert!(result.contains("--max-num-batched-tokens 2048"));
        assert!(result.contains("--enable-prefix-caching"));
        assert!(result.contains("--trust-remote-code"));
    }

    #[test]
    fn test_vllm_form_to_args_only_managed_flags() {
        // When existing has ONLY managed flags, result should only have form values
        let existing = "--quantization fp8\n--trust-remote-code";
        let form = VllmSettings {
            quantization: Some("awq".to_string()),
            trust_remote_code: false,
            ..Default::default()
        };
        let result = vllm_form_to_args(&form, existing);

        assert!(result.contains("--quantization awq"));
        // trust_remote_code was set to false, so the flag should not appear
        assert!(!result.contains("--trust-remote-code"));
    }

    // ── vllm_form_to_args: preserves unparseable managed flags ──────────

    #[test]
    fn test_vllm_form_to_args_preserves_unparseable_numeric_flag() {
        // --max-model-len with non-numeric value should be preserved, not stripped
        let existing = "--batch-size 512\n--max-model-len abc\n--other-flag";
        let form = VllmSettings::default();
        let result = vllm_form_to_args(&form, existing);
        assert!(result.contains("--batch-size 512"));
        assert!(result.contains("--max-model-len abc"));
        assert!(result.contains("--other-flag"));
    }

    #[test]
    fn test_vllm_form_to_args_preserves_unparseable_gpu_memory() {
        // --gpu-memory-utilization with non-numeric value should be preserved
        let existing = "--batch-size 512\n--gpu-memory-utilization notanumber";
        let form = VllmSettings::default();
        let result = vllm_form_to_args(&form, existing);
        assert!(result.contains("--batch-size 512"));
        assert!(result.contains("--gpu-memory-utilization notanumber"));
    }

    #[test]
    fn test_vllm_form_to_args_still_replaces_parseable_flags() {
        // Valid numeric values should still be replaced normally
        let existing = "--max-model-len 4096\n--tensor-parallel-size abc";
        let form = VllmSettings {
            max_model_len: Some(8192),
            ..Default::default()
        };
        let result = vllm_form_to_args(&form, existing);
        assert!(result.contains("--max-model-len 8192")); // replaced
        assert!(!result.contains("4096")); // old value gone
        assert!(result.contains("--tensor-parallel-size abc")); // preserved (unparseable)
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

    #[test]
    fn test_vllm_form_to_args_strips_flattened_flags() {
        let existing = "/mnt/models/org/repo\n--quantization\nfp8\n--attention-backend\nROCM_AITER_UNIFIED_ATTN\n--enable-prefix-caching";
        let form = VllmSettings {
            quantization: Some("awq".to_string()),
            ..Default::default()
        };
        let result = vllm_form_to_args(&form, existing);
        // Old flattened flag+value pair is gone; canonical grouped form appended
        assert!(!result.contains("fp8"));
        assert!(result.contains("--quantization awq"));
        // Unmanaged lines (positional path + free-form flag) preserved
        assert!(result.contains("/mnt/models/org/repo"));
        assert!(result.contains("--attention-backend\nROCM_AITER_UNIFIED_ATTN"));
        // Boolean flag stripped (form has it false)
        assert!(!result.contains("--enable-prefix-caching"));
    }

    #[test]
    fn test_vllm_form_to_args_preserves_unparseable_flattened() {
        let existing = "--max-model-len\nabc";
        let form = VllmSettings::default();
        let result = vllm_form_to_args(&form, existing);
        assert!(result.contains("--max-model-len\nabc"));
    }

    #[test]
    fn test_flattened_round_trip_is_canonical() {
        // Flattened input parses → re-emits as canonical grouped lines → re-parses identically
        let flattened = "--kv-cache-dtype\nfp8\n--tensor-parallel-size\n2";
        let form = args_to_vllm_form(flattened);
        let canonical = vllm_form_to_args(&form, "");
        assert!(canonical.contains("--kv-cache-dtype fp8"));
        assert!(canonical.contains("--tensor-parallel-size 2"));
        let form2 = args_to_vllm_form(&canonical);
        assert_eq!(form, form2);
    }
}
