//! Parse vLLM-specific flags from grouped `args` entries.
//!
//! Stored `args` are `Vec<String>` where each entry is one "line" in
//! shell-like form (e.g. `"--quantization fp8"`, `"--enable-prefix-caching"`).
//! This module extracts the 8 managed vLLM flags into a [`VllmConfig`] and
//! returns the remaining entries as the stripped args list.

use crate::config::types::VllmConfig;

// ── Managed flag names (canonical `--flag` form) ──────────────────────────

/// String-valued managed flags.
const STRING_FLAGS: &[&str] = &["quantization", "kv-cache-dtype"];

/// u32-valued managed flags.
const U32_FLAGS: &[&str] = &[
    "tensor-parallel-size",
    "max-model-len",
    "max-num-batched-tokens",
];

/// f64-valued managed flags.
const F64_FLAGS: &[&str] = &["gpu-memory-utilization"];

/// Boolean managed flags (presence = true).
const BOOL_FLAGS: &[&str] = &["enable-prefix-caching", "trust-remote-code"];

/// Check whether a flag name (without `--` prefix) is a managed vLLM flag.
fn is_managed(flag: &str) -> bool {
    STRING_FLAGS.contains(&flag)
        || U32_FLAGS.contains(&flag)
        || F64_FLAGS.contains(&flag)
        || BOOL_FLAGS.contains(&flag)
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Extract managed vLLM flags from grouped `args` and return the config +
/// remaining args.
///
/// Returns `(VllmConfig, Vec<String>)` where:
/// - `VllmConfig` contains the extracted flag values (defaults where not found)
/// - `Vec<String>` contains the entries that were NOT managed flags, in original order
///
/// Handles three input forms:
/// 1. **Grouped:** `"--quantization fp8"` (flag + value in one entry)
/// 2. **Inline:** `"--quantization=fp8"` (`--flag=value`)
/// 3. **Flattened:** `"--quantization"` followed by `"fp8"` as separate entries
///
/// If a numeric value fails to parse, the entry (and its value) is kept
/// untouched in the remaining args.
///
/// Comment lines (starting with `#`) are preserved verbatim.
pub fn extract_vllm_args(args: &[String]) -> (VllmConfig, Vec<String>) {
    let mut config = VllmConfig::default();
    let mut remaining: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;

    while i < args.len() {
        let entry = &args[i];
        let trimmed = entry.trim();

        // Comment lines — keep verbatim
        if trimmed.starts_with('#') {
            remaining.push(entry.clone());
            i += 1;
            continue;
        }

        // Try to parse as --flag=value
        if let Some((flag, value)) = parse_inline_flag(trimmed) {
            if is_managed(flag) {
                if try_apply_flag(&mut config, flag, Some(&value)) {
                    // Successfully extracted — drop this entry
                    i += 1;
                    continue;
                }
                // Parse failed — keep entry
                remaining.push(entry.clone());
                i += 1;
                continue;
            }
            // Not managed — keep
            remaining.push(entry.clone());
            i += 1;
            continue;
        }

        // Try to parse as --flag value (grouped) or bare --flag (flattened)
        if let Some((flag, value_in_entry)) = parse_grouped_flag(trimmed) {
            if is_managed(flag) {
                if value_in_entry.is_some() {
                    // Grouped form: --flag value
                    if try_apply_flag(&mut config, flag, value_in_entry.as_deref()) {
                        // Successfully extracted — drop this entry
                        i += 1;
                        continue;
                    }
                    // Parse failed — keep entry
                    remaining.push(entry.clone());
                    i += 1;
                    continue;
                } else {
                    // Bare flag — could be boolean or flattened value-flag
                    if is_boolean_flag(flag) {
                        // Boolean flag — set true, drop entry
                        apply_boolean_flag(&mut config, flag, true);
                        i += 1;
                        continue;
                    } else {
                        // Value flag in flattened form — look at next entry
                        let next = args.get(i + 1);
                        if let Some(next_entry) = next {
                            let next_trimmed = next_entry.trim();
                            // Next entry must not start with -- (not another flag)
                            // and must be non-empty
                            if !next_trimmed.is_empty() && !next_trimmed.starts_with("--") {
                                if try_apply_flag(&mut config, flag, Some(next_trimmed)) {
                                    // Successfully extracted — drop both entries
                                    i += 2;
                                    continue;
                                }
                                // Parse failed — keep both entries
                                remaining.push(entry.clone());
                                remaining.push(next_entry.clone());
                                i += 2;
                                continue;
                            }
                        }
                        // No valid next entry — keep the bare flag
                        remaining.push(entry.clone());
                        i += 1;
                        continue;
                    }
                }
            }
            // Not managed — keep
            remaining.push(entry.clone());
            i += 1;
            continue;
        }

        // Doesn't look like a flag at all — keep
        remaining.push(entry.clone());
        i += 1;
    }

    (config, remaining)
}

// ── Internal helpers ───────────────────────────────────────────────────────

/// Parse `--flag=value` form. Returns `(flag_name_without_dashes, value)`.
fn parse_inline_flag(entry: &str) -> Option<(&str, String)> {
    if !entry.starts_with("--") {
        return None;
    }
    let after_dashes = &entry[2..];
    let eq_pos = after_dashes.find('=')?;
    let flag = &after_dashes[..eq_pos];
    let value = after_dashes[eq_pos + 1..].to_string();
    Some((flag, value))
}

/// Parse `--flag` or `--flag value` form.
/// Returns `(flag_name_without_dashes, Option<value>)`.
/// Returns `None` if the entry doesn't start with `--` (or is `--` itself).
fn parse_grouped_flag(entry: &str) -> Option<(&str, Option<String>)> {
    if !entry.starts_with("--") {
        return None;
    }
    let after_dashes = &entry[2..];
    // Reject bare `--`
    if after_dashes.is_empty() {
        return None;
    }
    // Reject `--flag=value` (handled by parse_inline_flag)
    if after_dashes.contains('=') {
        return None;
    }
    let parts: Vec<&str> = after_dashes.splitn(2, char::is_whitespace).collect();
    let flag = parts[0];
    let value = parts
        .get(1)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    Some((flag, value))
}

/// Check if a flag name is a boolean flag.
fn is_boolean_flag(flag: &str) -> bool {
    BOOL_FLAGS.contains(&flag)
}

/// Apply a boolean flag value to the config.
fn apply_boolean_flag(config: &mut VllmConfig, flag: &str, value: bool) {
    match flag {
        "enable-prefix-caching" => config.enable_prefix_caching = value,
        "trust-remote-code" => config.trust_remote_code = value,
        _ => {}
    }
}

/// Try to apply a flag value to the config.
/// Returns `true` if the value was successfully parsed and applied.
fn try_apply_flag(config: &mut VllmConfig, flag: &str, value: Option<&str>) -> bool {
    let value = match value {
        Some(v) => v,
        None => return false,
    };

    // String flags
    if STRING_FLAGS.contains(&flag) {
        match flag {
            "quantization" => config.quantization = Some(value.to_string()),
            "kv-cache-dtype" => config.kv_cache_dtype = Some(value.to_string()),
            _ => return false,
        }
        return true;
    }

    // u32 flags
    if U32_FLAGS.contains(&flag) {
        let Ok(n) = value.parse::<u32>() else {
            return false;
        };
        match flag {
            "tensor-parallel-size" => config.tensor_parallel_size = Some(n),
            "max-model-len" => config.max_model_len = Some(n),
            "max-num-batched-tokens" => config.max_num_batched_tokens = Some(n),
            _ => return false,
        }
        return true;
    }

    // f64 flags
    if F64_FLAGS.contains(&flag) {
        let Ok(f) = value.parse::<f64>() else {
            return false;
        };
        match flag {
            "gpu-memory-utilization" => config.gpu_memory_utilization = Some(f),
            _ => return false,
        }
        return true;
    }

    // Boolean flags — parse explicit true/false values
    if BOOL_FLAGS.contains(&flag) {
        match value {
            "true" | "1" => {
                apply_boolean_flag(config, flag, true);
                return true;
            }
            "false" | "0" => {
                apply_boolean_flag(config, flag, false);
                return true;
            }
            _ => return false,
        }
    }

    false
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Grouped form: `--flag value` in single entries.
    /// Managed flags extracted, unmanaged `--attention-backend` preserved.
    #[test]
    fn test_grouped_form() {
        let args = vec![
            "--quantization fp8".to_string(),
            "--kv-cache-dtype fp8".to_string(),
            "--tensor-parallel-size 2".to_string(),
            "--gpu-memory-utilization 0.92".to_string(),
            "--attention-backend ROCM_AITER_UNIFIED_ATTN".to_string(),
            "--max-num-batched-tokens 2560".to_string(),
            "--enable-prefix-caching".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);

        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert_eq!(config.kv_cache_dtype, Some("fp8".to_string()));
        assert_eq!(config.tensor_parallel_size, Some(2));
        assert_eq!(config.gpu_memory_utilization, Some(0.92));
        assert_eq!(config.max_num_batched_tokens, Some(2560));
        assert!(config.enable_prefix_caching);
        assert_eq!(config.max_model_len, None);
        assert!(!config.trust_remote_code);

        // Only the unmanaged flag should remain
        assert_eq!(
            remaining,
            vec!["--attention-backend ROCM_AITER_UNIFIED_ATTN"]
        );
    }

    /// `--flag=value` inline form.
    #[test]
    fn test_inline_equals_form() {
        let args = vec![
            "--quantization=fp8".to_string(),
            "--kv-cache-dtype=auto".to_string(),
            "--tensor-parallel-size=4".to_string(),
            "--gpu-memory-utilization=0.8".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);

        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert_eq!(config.kv_cache_dtype, Some("auto".to_string()));
        assert_eq!(config.tensor_parallel_size, Some(4));
        assert_eq!(config.gpu_memory_utilization, Some(0.8));
        assert!(remaining.is_empty());
    }

    /// Flattened form: flag and value as separate Vec entries.
    #[test]
    fn test_flattened_form() {
        let args = vec![
            "--quantization".to_string(),
            "fp8".to_string(),
            "--tensor-parallel-size".to_string(),
            "2".to_string(),
            "--max-model-len".to_string(),
            "8192".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);

        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert_eq!(config.tensor_parallel_size, Some(2));
        assert_eq!(config.max_model_len, Some(8192));
        assert!(remaining.is_empty());
    }

    /// Unparseable numeric value — entry preserved in remaining args.
    #[test]
    fn test_unparseable_numeric_preserved() {
        let args = vec![
            "--tensor-parallel-size notanumber".to_string(),
            "--max-model-len".to_string(),
            "abc".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);

        assert_eq!(config.tensor_parallel_size, None);
        assert_eq!(config.max_model_len, None);
        // Both entries preserved
        assert_eq!(
            remaining,
            vec![
                "--tensor-parallel-size notanumber",
                "--max-model-len",
                "abc",
            ]
        );
    }

    /// Flag followed by another flag — the next entry is not consumed as a value.
    #[test]
    fn test_flag_followed_by_flag_not_eaten() {
        let args = vec![
            "--tensor-parallel-size".to_string(),
            "--enable-prefix-caching".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);

        // tensor_parallel_size not set (next entry is a flag, not a value)
        assert_eq!(config.tensor_parallel_size, None);
        // enable-prefix-caching is boolean, so it's extracted
        assert!(config.enable_prefix_caching);
        // The bare --tensor-parallel-size is kept since it had no valid value
        assert_eq!(remaining, vec!["--tensor-parallel-size"]);
    }

    /// Unmanaged args preserve their order.
    #[test]
    fn test_unmanaged_args_order_preserved() {
        let args = vec![
            "--unmanaged-flag 1".to_string(),
            "--quantization fp8".to_string(),
            "--another-flag 2".to_string(),
            "--tensor-parallel-size 2".to_string(),
            "--last-flag 3".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);

        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert_eq!(config.tensor_parallel_size, Some(2));
        assert_eq!(
            remaining,
            vec!["--unmanaged-flag 1", "--another-flag 2", "--last-flag 3",]
        );
    }

    /// Comment lines are preserved verbatim.
    #[test]
    fn test_comment_lines_preserved() {
        let args = vec![
            "# This is a comment".to_string(),
            "--quantization fp8".to_string(),
            "# Another comment".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);

        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert_eq!(remaining, vec!["# This is a comment", "# Another comment"]);
    }

    /// Boolean flags: trust-remote-code.
    #[test]
    fn test_boolean_trust_remote_code() {
        let args = vec![
            "--trust-remote-code".to_string(),
            "--quantization fp8".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);

        assert!(config.trust_remote_code);
        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert!(remaining.is_empty());
    }

    /// Mixed forms: grouped, inline, and flattened in the same args.
    #[test]
    fn test_mixed_forms() {
        let args = vec![
            "--quantization=fp8".to_string(),
            "--kv-cache-dtype auto".to_string(),
            "--tensor-parallel-size".to_string(),
            "4".to_string(),
            "--gpu-memory-utilization=0.9".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);

        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert_eq!(config.kv_cache_dtype, Some("auto".to_string()));
        assert_eq!(config.tensor_parallel_size, Some(4));
        assert_eq!(config.gpu_memory_utilization, Some(0.9));
        assert!(remaining.is_empty());
    }

    /// Empty args returns empty config and empty remaining.
    #[test]
    fn test_empty_args() {
        let args: Vec<String> = vec![];
        let (config, remaining) = extract_vllm_args(&args);
        assert!(config.is_empty());
        assert!(remaining.is_empty());
    }

    /// All boolean flags extracted.
    #[test]
    fn test_all_boolean_flags() {
        let args = vec![
            "--enable-prefix-caching".to_string(),
            "--trust-remote-code".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);
        assert!(config.enable_prefix_caching);
        assert!(config.trust_remote_code);
        assert!(remaining.is_empty());
    }

    /// Flattened form with boolean flag between value flags.
    #[test]
    fn test_flattened_with_boolean_in_middle() {
        let args = vec![
            "--quantization".to_string(),
            "fp8".to_string(),
            "--enable-prefix-caching".to_string(),
            "--tensor-parallel-size".to_string(),
            "2".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);
        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert!(config.enable_prefix_caching);
        assert_eq!(config.tensor_parallel_size, Some(2));
        assert!(remaining.is_empty());
    }

    /// Flattened value flag at end of list with no following value.
    #[test]
    fn test_flattened_value_flag_at_end_no_value() {
        let args = vec![
            "--quantization fp8".to_string(),
            "--tensor-parallel-size".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);
        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert_eq!(config.tensor_parallel_size, None);
        assert_eq!(remaining, vec!["--tensor-parallel-size"]);
    }

    /// Boolean flags with explicit =false and =true values.
    #[test]
    fn test_boolean_flags_explicit_values() {
        let args = vec![
            "--enable-prefix-caching=false".to_string(),
            "--trust-remote-code=true".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);
        assert!(!config.enable_prefix_caching);
        assert!(config.trust_remote_code);
        assert!(remaining.is_empty());
    }

    /// Boolean flags with =0 and =1 numeric values.
    #[test]
    fn test_boolean_flags_numeric_values() {
        let args = vec![
            "--enable-prefix-caching=0".to_string(),
            "--trust-remote-code=1".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);
        assert!(!config.enable_prefix_caching);
        assert!(config.trust_remote_code);
        assert!(remaining.is_empty());
    }

    /// Boolean flag with unrecognised value — preserved in remaining.
    #[test]
    fn test_boolean_flag_unrecognised_value_preserved() {
        let args = vec![
            "--enable-prefix-caching=yes".to_string(),
            "--quantization fp8".to_string(),
        ];
        let (config, remaining) = extract_vllm_args(&args);
        assert!(!config.enable_prefix_caching);
        assert_eq!(config.quantization, Some("fp8".to_string()));
        assert_eq!(remaining, vec!["--enable-prefix-caching=yes"]);
    }
}
