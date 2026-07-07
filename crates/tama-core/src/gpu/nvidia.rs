use super::types::GpuDeviceStats;
use super::vram::VramInfo;

/// Parse a single line of nvidia-smi CSV output into `GpuDeviceStats`.
///
/// Expected format (9 comma-separated fields, no units):
/// `index,name,uuid,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,fan.speed`
///
/// Returns `None` if the line is malformed or fields cannot be parsed.
pub(crate) fn parse_nvidia_smi_csv_line(line: &str) -> Option<GpuDeviceStats> {
    let parts: Vec<&str> = line.split(",").collect();
    if parts.len() != 9 {
        return None;
    }

    let index: u32 = parts[0].trim().parse().ok()?;
    let name = parts[1].trim().to_string();
    let uuid = if parts[2].trim().is_empty() {
        None
    } else {
        Some(parts[2].trim().to_string())
    };
    let utilization: u8 = parts[3].trim().parse().ok()?;
    let mem_used: u64 = parts[4].trim().parse().ok()?;
    let mem_total: u64 = parts[5].trim().parse().ok()?;
    let temperature: u8 = parts[6].trim().parse().ok()?;
    let power: u16 = parts[7].trim().parse().ok()?;
    let fan: u8 = parts[8].trim().parse().ok()?;

    Some(GpuDeviceStats {
        device_id: format!("nvidia{index}"),
        vendor: "nvidia".to_string(),
        name,
        utilization_pct: Some(utilization),
        vram: Some(VramInfo {
            used_mib: mem_used,
            total_mib: mem_total,
        }),
        temperature_c: Some(temperature),
        power_w: Some(power),
        fan_pct: Some(fan),
        pci_bus: None,
        uuid,
    })
}

/// Query all NVIDIA GPU devices via nvidia-smi.
/// Returns one `GpuDeviceStats` per detected GPU.
pub(crate) fn query_nvidia_devices() -> Vec<GpuDeviceStats> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,name,uuid,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,fan.speed",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok();

    let Some(output) = output else {
        return vec![];
    };

    if !output.status.success() {
        return vec![];
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(|line| parse_nvidia_smi_csv_line(line.trim()))
        .collect()
}
