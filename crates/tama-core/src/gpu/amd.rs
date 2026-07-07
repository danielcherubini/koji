use std::collections::HashMap;
use std::sync::OnceLock;

use super::types::GpuDeviceStats;
use super::vram::VramInfo;

/// Cached map of PCI bus address → GPU product name from rocm-smi.
/// Populated on first call to `query_amd_device_names` and reused thereafter
/// to avoid spawning rocm-smi on every metrics tick.
/// Uses PCI bus (e.g. "0000:03:00.0") as the key since sysfs card numbers
/// may not match rocm-smi's card indices.
static AMD_DEVICE_NAMES: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Cached map of PCI bus address → GPU hardware UUID from rocm-smi.
/// Populated on first call to `query_amd_device_uuids` and reused thereafter.
static AMD_DEVICE_UUIDS: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Query rocm-smi for GPU product names and cache the result.
/// Returns a map of PCI bus address (e.g. "0000:03:00.0") → product name (e.g. "Radeon AI PRO R9700").
fn query_amd_device_names() -> HashMap<String, String> {
    AMD_DEVICE_NAMES
        .get_or_init(|| {
            let output = std::process::Command::new("rocm-smi")
                .args(["--showbus", "--showproductname", "--json"])
                .output()
                .ok();

            let Some(output) = output else {
                return HashMap::new();
            };

            if !output.status.success() {
                return HashMap::new();
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let Ok(parsed): Result<HashMap<String, HashMap<String, String>>, serde_json::Error> =
                serde_json::from_str(&stdout)
            else {
                return HashMap::new();
            };

            parsed
                .into_values()
                .filter_map(|info| {
                    let pci_bus = info.get("PCI Bus")?.clone();
                    let series = info.get("Card Series")?.clone();
                    // Extract short name: "AMD Radeon AI PRO R9700" → "Radeon AI PRO R9700"
                    let name = series
                        .split_whitespace()
                        .skip_while(|w| *w == "AMD")
                        .collect::<Vec<_>>()
                        .join(" ");
                    Some((pci_bus, if name.is_empty() { series } else { name }))
                })
                .collect()
        })
        .clone()
}

/// Normalize an AMD GPU UUID from rocm-smi format to ROCR_VISIBLE_DEVICES format.
///
/// `rocm-smi --showuniqueid` returns `0xb3780db0a262809e` (0x-prefixed hex),
/// but `ROCR_VISIBLE_DEVICES` expects the `rocminfo` format `GPU-b3780db0a262809e`
/// (GPU- prefix, no 0x). Without this transformation, ROCR does not recognize
/// the UUID and hides all devices → "no ROCm-capable device is detected".
///
/// Values already in `GPU-...` format pass through unchanged.
pub(super) fn normalize_amd_uuid(raw: &str) -> String {
    if let Some(hex) = raw.strip_prefix("0x") {
        format!("GPU-{hex}")
    } else {
        raw.to_string()
    }
}

/// Query rocm-smi for GPU hardware UUIDs and cache the result.
/// Returns a map of PCI bus address (e.g. "0000:03:00.0") → UUID (e.g. "GPU-b3780db0a262809e").
fn query_amd_device_uuids() -> HashMap<String, String> {
    AMD_DEVICE_UUIDS
        .get_or_init(|| {
            let output = std::process::Command::new("rocm-smi")
                .args(["--showbus", "--showuniqueid", "--json"])
                .output()
                .ok();

            let Some(output) = output else {
                return HashMap::new();
            };

            if !output.status.success() {
                return HashMap::new();
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let Ok(parsed): Result<HashMap<String, HashMap<String, String>>, serde_json::Error> =
                serde_json::from_str(&stdout)
            else {
                return HashMap::new();
            };

            parsed
                .into_values()
                .filter_map(|info| {
                    let pci_bus = info.get("PCI Bus")?.clone();
                    let unique_id = info.get("Unique ID")?.clone();
                    // rocm-smi returns "0xb3780db0a262809e" but ROCR_VISIBLE_DEVICES
                    // expects the rocminfo format "GPU-b3780db0a262809e".
                    let uuid = normalize_amd_uuid(&unique_id);
                    Some((pci_bus, uuid))
                })
                .collect()
        })
        .clone()
}

/// Query all AMD GPU devices via sysfs.
/// Returns one `GpuDeviceStats` per detected GPU card.
pub(super) fn query_amd_devices() -> Vec<GpuDeviceStats> {
    let pattern = "/sys/class/drm/card*/device";
    let Ok(paths) = glob::glob(pattern) else {
        return vec![];
    };

    let mut devices = Vec::new();

    for card_path in paths.flatten() {
        // Extract card number from the path (e.g. /sys/class/drm/card1/device → 1)
        let card_name = card_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let card_num = card_name
            .strip_prefix("card")
            .and_then(|n| n.parse::<u32>().ok());
        let Some(card_num) = card_num else {
            continue;
        };

        // Verify this is an AMD device by checking the driver
        let driver_path = card_path.join("driver");
        let is_amd = std::fs::read_link(&driver_path)
            .ok()
            .map(|p| p.to_string_lossy().contains("amdgpu"))
            .unwrap_or(false);
        if !is_amd {
            continue;
        }

        // GPU name from cached rocm-smi query, keyed by PCI bus address.
        // Read PCI_SLOT_NAME from uevent to match against rocm-smi's PCI Bus field.
        let pci_bus = std::fs::read_to_string(card_path.join("uevent"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("PCI_SLOT_NAME="))
                    .map(String::from)
            });
        let name = pci_bus
            .as_ref()
            .and_then(|pci| query_amd_device_names().get(pci).cloned())
            .unwrap_or_else(|| {
                // Fallback: try sysfs name file (not available on all systems)
                std::fs::read_to_string(card_path.join("name"))
                    .ok()
                    .and_then(|s| {
                        let trimmed = s.trim().to_string();
                        if let (Some(start), Some(end)) = (trimmed.find('['), trimmed.find(']')) {
                            Some(trimmed[start + 1..end].to_string())
                        } else if !trimmed.is_empty() {
                            Some(trimmed)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| "AMD GPU".to_string())
            });

        // Look up UUID from cached rocm-smi query, keyed by PCI bus address.
        let uuid = pci_bus
            .as_ref()
            .and_then(|pci| query_amd_device_uuids().get(pci).cloned());

        let mut stats = GpuDeviceStats {
            device_id: format!("amd{card_num}"),
            vendor: "amd".to_string(),
            name,
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
            pci_bus: pci_bus.clone(),
            uuid,
        };

        // GPU utilization from gpu_busy_percent
        if let Ok(contents) = std::fs::read_to_string(card_path.join("gpu_busy_percent")) {
            if let Ok(pct) = contents.trim().parse::<u8>() {
                stats.utilization_pct = Some(pct);
            }
        }

        // VRAM from mem_info_vram_used / mem_info_vram_total
        let used_bytes = std::fs::read_to_string(card_path.join("mem_info_vram_used"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        let total_bytes = std::fs::read_to_string(card_path.join("mem_info_vram_total"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        if let (Some(u), Some(t)) = (used_bytes, total_bytes) {
            stats.vram = Some(VramInfo {
                used_mib: u / (1024 * 1024),
                total_mib: t / (1024 * 1024),
            });
        }

        // Temperature: try temp1_input or temp2_input (hwmon path varies)
        // Values are in millidegrees Celsius
        let temp_path = card_path.parent().unwrap();
        for temp_file in &["temp1_input", "temp2_input"] {
            if let Ok(contents) = std::fs::read_to_string(temp_path.join(temp_file)) {
                if let Ok(temp_milli) = contents.trim().parse::<i64>() {
                    stats.temperature_c = Some((temp_milli / 1000) as u8);
                    break;
                }
            }
        }

        // Also try hwmon subdirectory for temperature
        if stats.temperature_c.is_none() {
            let hwmon_pattern = format!("{}/hwmon/hwmon*/temp*_input", card_path.display());
            if let Ok(paths) = glob::glob(&hwmon_pattern) {
                for path in paths.flatten() {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        if let Ok(temp_milli) = contents.trim().parse::<i64>() {
                            stats.temperature_c = Some((temp_milli / 1000) as u8);
                            break;
                        }
                    }
                }
            }
        }

        // Power: try power1_average (µW → W)
        if let Ok(contents) = std::fs::read_to_string(temp_path.join("power1_average")) {
            if let Ok(power_uw) = contents.trim().parse::<u64>() {
                stats.power_w = Some((power_uw / 1_000_000) as u16);
            }
        }

        // Also try hwmon for power
        if stats.power_w.is_none() {
            let hwmon_pattern = format!("{}/hwmon/hwmon*/power1_average", card_path.display());
            if let Ok(paths) = glob::glob(&hwmon_pattern) {
                for path in paths.flatten() {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        if let Ok(power_uw) = contents.trim().parse::<u64>() {
                            stats.power_w = Some((power_uw / 1_000_000) as u16);
                            break;
                        }
                    }
                }
            }
        }

        // Fan: scan hwmon for fan1_input (RPM → %)
        let fan_pattern = format!("{}/hwmon/hwmon*/fan1_input", card_path.display());
        if let Ok(paths) = glob::glob(&fan_pattern) {
            for path in paths.flatten() {
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    if let Ok(rpm) = contents.trim().parse::<u16>() {
                        // Read fan1_max from the same hwmon directory for accurate %
                        let hwmon_dir = path.parent().unwrap();
                        let max_rpm = std::fs::read_to_string(hwmon_dir.join("fan1_max"))
                            .ok()
                            .and_then(|s| s.trim().parse::<u16>().ok())
                            .unwrap_or(3000);
                        let pct = if max_rpm > 0 && rpm > 0 {
                            ((rpm as u32 * 100) / max_rpm as u32).min(100) as u8
                        } else {
                            0
                        };
                        stats.fan_pct = Some(pct);
                        break;
                    }
                }
            }
        }

        devices.push(stats);
    }

    devices.sort_by_key(|d| d.device_id.clone());
    devices
}
