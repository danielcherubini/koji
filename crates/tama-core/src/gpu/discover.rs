use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;

/// GPU device information discovered from a backend's `--list-devices` output.
#[derive(Debug, Clone, Serialize)]
pub struct GpuDeviceInfo {
    /// Device identifier (e.g. "CUDA0", "ROCm0")
    pub device_id: String,
    /// Human-readable device name (e.g. "NVIDIA GeForce RTX 4090")
    pub name: String,
    /// Vendor prefix (e.g. "CUDA", "ROCm", "Metal")
    pub vendor: String,
    /// Total VRAM in MiB, if reported
    pub vram_total_mib: Option<u64>,
    /// Free VRAM in MiB, if reported
    pub vram_free_mib: Option<u64>,
}

/// Parse the output of `<backend-binary> --list-devices` into a list of GPU devices.
///
/// Expected format (one device per line):
/// ```text
/// Available devices:
///   CPU0: CPU
///   CUDA0: NVIDIA GeForce RTX 4090 (24576 MiB, 9828 MiB free)
///   CUDA1: NVIDIA A100 (40960 MiB, 40960 MiB free)
///   ROCm0: AMD Radeon RX 7900 XT (24576 MiB, 24000 MiB free)
/// ```
///
/// Lines that don't match the expected format are silently skipped.
/// CPU devices are filtered out. VRAM info is optional.
pub fn parse_llama_list_devices_output(output: &str) -> Vec<GpuDeviceInfo> {
    let mut devices = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Split on the first colon to get device_id and the rest
        let Some((id_part, rest)) = trimmed.split_once(':') else {
            continue;
        };

        let device_id = id_part.trim();
        let rest = rest.trim();

        // Must contain at least one digit (e.g. "CUDA0", "ROCm0") to be a device line
        if !device_id.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }

        // Skip CPU devices
        if device_id.to_uppercase().starts_with("CPU") {
            continue;
        }

        // Extract vendor from device_id (everything before the trailing digit(s))
        let vendor = device_id
            .chars()
            .take_while(|c| c.is_alphabetic())
            .collect();

        // Parse optional VRAM info: "(NNNN MiB, NNNN MiB free)"
        let (name, vram_total_mib, vram_free_mib) =
            if let Some((name_part, vram_part)) = rest.split_once('(') {
                let name = name_part.trim().to_string();
                let (total, free) = parse_vram_info(vram_part.trim());
                (name, total, free)
            } else {
                (rest.to_string(), None, None)
            };

        devices.push(GpuDeviceInfo {
            device_id: device_id.to_string(),
            name,
            vendor,
            vram_total_mib,
            vram_free_mib,
        });
    }

    devices
}

/// Parse VRAM info from a string like "24576 MiB, 9828 MiB free)" or "40960 MiB)".
fn parse_vram_info(vram_str: &str) -> (Option<u64>, Option<u64>) {
    // Remove trailing ')' if present
    let cleaned = vram_str.trim_end_matches(')');

    // Try to parse "NNNN MiB, NNNN MiB free"
    if let Some(comma_pos) = cleaned.find(',') {
        let total_part = &cleaned[..comma_pos];
        let free_part = &cleaned[comma_pos + 1..];

        let total = parse_mib_value(total_part);
        let free = parse_mib_value(free_part);

        (total, free)
    } else {
        // Just total, no free (e.g. "40960 MiB")
        let total = parse_mib_value(cleaned);
        (total, None)
    }
}

/// Parse a value like "24576 MiB" or "9828 MiB free" into a u64.
fn parse_mib_value(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    // Extract the number (everything before "MiB" or end of string)
    let num_str = trimmed.split_whitespace().next().unwrap_or(trimmed);
    num_str.parse().ok()
}

/// Discover GPU devices by running `<binary> --list-devices`.
///
/// Spawns a blocking subprocess call, captures stdout, and parses the output.
pub fn discover_devices_via_binary(binary_path: &Path) -> Result<Vec<GpuDeviceInfo>> {
    let output = std::process::Command::new(binary_path)
        .arg("--list-devices")
        .output()
        .with_context(|| {
            format!(
                "Failed to execute '{} --list-devices'",
                binary_path.display()
            )
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Some backends write device list to stderr
    let combined = if !stdout.is_empty() {
        stdout
    } else if !stderr.is_empty() {
        stderr
    } else {
        return Ok(Vec::new());
    };

    Ok(parse_llama_list_devices_output(&combined))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_llama_list_devices_empty() {
        let result = parse_llama_list_devices_output("");
        assert!(result.is_empty(), "empty string should yield empty vec");
    }

    #[test]
    fn test_parse_llama_list_devices_single_cuda() {
        let output =
            "Available devices:\n  CUDA0: NVIDIA GeForce RTX 4090 (24576 MiB, 9828 MiB free)";
        let result = parse_llama_list_devices_output(output);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].device_id, "CUDA0");
        assert_eq!(result[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(result[0].vendor, "CUDA");
        assert_eq!(result[0].vram_total_mib, Some(24576));
        assert_eq!(result[0].vram_free_mib, Some(9828));
    }

    #[test]
    fn test_parse_llama_list_devices_multiple_vendors() {
        let output = "Available devices:\n  CUDA0: NVIDIA GeForce RTX 4090 (24576 MiB, 9828 MiB free)\n  ROCm0: AMD Radeon RX 7900 XT (24576 MiB, 24000 MiB free)";
        let result = parse_llama_list_devices_output(output);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].device_id, "CUDA0");
        assert_eq!(result[0].vendor, "CUDA");
        assert_eq!(result[1].device_id, "ROCm0");
        assert_eq!(result[1].vendor, "ROCm");
    }

    #[test]
    fn test_parse_llama_list_devices_skips_cpu() {
        let output = "Available devices:\n  CPU0: CPU\n  CUDA0: NVIDIA GeForce RTX 4090 (24576 MiB, 9828 MiB free)";
        let result = parse_llama_list_devices_output(output);
        assert_eq!(result.len(), 1, "CPU devices should be filtered out");
        assert_eq!(result[0].device_id, "CUDA0");
    }

    #[test]
    fn test_parse_llama_list_devices_skips_malformed_lines() {
        let output = "Available devices:\n  CUDA0: NVIDIA GeForce RTX 4090 (24576 MiB, 9828 MiB free)\n  garbage line without colon\n  another bad line\n\n  CUDA1: NVIDIA A100 (40960 MiB, 40960 MiB free)";
        let result = parse_llama_list_devices_output(output);
        assert_eq!(result.len(), 2, "malformed lines should be skipped");
        assert_eq!(result[0].device_id, "CUDA0");
        assert_eq!(result[1].device_id, "CUDA1");
    }

    #[test]
    fn test_parse_llama_list_devices_with_vram_info() {
        let output = "Available devices:\n  CUDA0: NVIDIA A100 (40960 MiB, 40960 MiB free)";
        let result = parse_llama_list_devices_output(output);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].vram_total_mib, Some(40960));
        assert_eq!(result[0].vram_free_mib, Some(40960));
    }

    #[test]
    fn test_parse_llama_list_devices_no_vram_info() {
        let output = "Available devices:\n  Metal0: Apple M2 Max";
        let result = parse_llama_list_devices_output(output);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].device_id, "Metal0");
        assert_eq!(result[0].name, "Apple M2 Max");
        assert_eq!(result[0].vendor, "Metal");
        assert_eq!(result[0].vram_total_mib, None);
        assert_eq!(result[0].vram_free_mib, None);
    }

    #[test]
    fn test_parse_llama_list_devices_extra_whitespace() {
        let output = "Available devices:\n\t\tCUDA0:\tNVIDIA GeForce RTX 4090\t(24576 MiB, 9828 MiB free)\n   \t  ROCm0:   AMD Radeon RX 7900 XT   (24576 MiB, 24000 MiB free)";
        let result = parse_llama_list_devices_output(output);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].device_id, "CUDA0");
        assert_eq!(result[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(result[0].vram_total_mib, Some(24576));
        assert_eq!(result[1].device_id, "ROCm0");
        assert_eq!(result[1].name, "AMD Radeon RX 7900 XT");
    }
}
