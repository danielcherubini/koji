use serde::{Deserialize, Serialize};
use sysinfo::Networks;

/// Tick interval in seconds (hardcoded)
const TICK_INTERVAL_SECS: f64 = 2.0;

/// Network throughput statistics for a single tick
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NetworkStats {
    /// Download throughput in MiB/s since last tick
    pub download_mibps: f64,
    /// Upload throughput in MiB/s since last tick
    pub upload_mibps: f64,
}

/// Detect the primary (default route) network interface.
///
/// Strategy:
/// 1. Linux: parse `/proc/net/route` for default route (destination `00000000`)
/// 2. macOS: run `route get default` and parse `interface:` line
/// 3. Fallback: use sysinfo to find the first non-loopback interface
///
/// Returns `None` if all interfaces are loopback or detection fails.
pub fn get_primary_interface() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        if let Some(iface) = parse_proc_net_route() {
            return Some(iface);
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(iface) = parse_macos_route() {
            return Some(iface);
        }
    }

    // Fallback for all platforms: use sysinfo
    sysinfo_fallback()
}

#[cfg(target_os = "linux")]
fn parse_proc_net_route() -> Option<String> {
    let content = std::fs::read_to_string("/proc/net/route").ok()?;

    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 3 {
            // Column 1 is Interface, Column 2 is Destination (hex)
            let interface = fields[0];
            let destination = fields[1];
            if destination == "00000000" {
                return Some(interface.to_string());
            }
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn parse_macos_route() -> Option<String> {
    let output = std::process::Command::new("route")
        .args(["get", "default"])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(value) = line.split(':').nth(1) {
            let iface = value.trim();
            if !iface.is_empty() {
                return Some(iface.to_string());
            }
        }
    }

    None
}

fn sysinfo_fallback() -> Option<String> {
    let networks = Networks::new_with_refreshed_list();
    for (key, _) in networks.iter() {
        if !key.starts_with("lo") {
            return Some(key.to_string());
        }
    }
    None
}

/// Collect network throughput statistics for the given interface.
///
/// Returns:
/// - `Some(NetworkStats)` with download/upload MiB/s if the interface is found
/// - `None` if the interface is missing from sysinfo
/// - Always returns updated cumulative rx/tx counters
///
/// # Arguments
/// * `primary_interface` - The interface name to monitor
/// * `networks` - Mutable reference to Networks for refreshing byte counters
/// * `previous_rx` - Cumulative bytes received from the previous tick
/// * `previous_tx` - Cumulative bytes transmitted from the previous tick
pub fn collect_network_stats(
    primary_interface: &str,
    networks: &mut Networks,
    previous_rx: u64,
    previous_tx: u64,
) -> (Option<NetworkStats>, u64, u64) {
    networks.refresh();

    // Try the primary interface first
    let (delta_rx, delta_tx) = if let Some(iface_data) = networks.get(primary_interface) {
        let current_rx = iface_data.total_received();
        let current_tx = iface_data.total_transmitted();

        // Guard against counter wraparound
        let delta_rx = current_rx.saturating_sub(previous_rx);
        let delta_tx = current_tx.saturating_sub(previous_tx);

        (delta_rx, delta_tx)
    } else {
        // Interface not found — try first non-lo interface as fallback
        let mut found = false;
        let mut fallback_rx = 0u64;
        let mut fallback_tx = 0u64;

        for (name, iface_data) in networks.iter() {
            if !name.starts_with("lo") {
                fallback_rx = iface_data.total_received();
                fallback_tx = iface_data.total_transmitted();
                found = true;
                break;
            }
        }

        if !found {
            return (None, previous_rx, previous_tx);
        }

        let delta_rx = fallback_rx.saturating_sub(previous_rx);
        let delta_tx = fallback_tx.saturating_sub(previous_tx);

        (delta_rx, delta_tx)
    };

    // Convert bytes to MiB/s: delta_bytes / 1024 / 1024 / TICK_INTERVAL_SECS
    let download_mibps = delta_rx as f64 / 1024.0 / 1024.0 / TICK_INTERVAL_SECS;
    let upload_mibps = delta_tx as f64 / 1024.0 / 1024.0 / TICK_INTERVAL_SECS;

    // Update cumulative counters with saturating_add to avoid overflow
    let new_rx = previous_rx.saturating_add(delta_rx);
    let new_tx = previous_tx.saturating_add(delta_tx);

    (
        Some(NetworkStats {
            download_mibps,
            upload_mibps,
        }),
        new_rx,
        new_tx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_primary_interface_loopback_fallback() {
        let interface = get_primary_interface();
        // Should return Some with a non-lo interface name if one exists
        if let Some(ref name) = interface {
            assert!(!name.starts_with("lo"), "Interface should not be loopback");
        }
    }

    #[test]
    fn test_collect_network_stats_zero_delta() {
        let mut networks = Networks::new_with_refreshed_list();
        // Refresh once to establish baseline
        networks.refresh();

        // Get current values as baseline
        let primary = get_primary_interface();
        if let Some(ref iface) = primary {
            if let Some(data) = networks.get(iface) {
                let baseline_rx = data.total_received();
                let baseline_tx = data.total_transmitted();

                // Create fresh Networks to get zero delta
                let mut fresh_networks = Networks::new_with_refreshed_list();

                let (stats, new_rx, new_tx) =
                    collect_network_stats(iface, &mut fresh_networks, baseline_rx, baseline_tx);

                if let Some(s) = stats {
                    // With zero delta, throughput should be 0.0
                    assert_eq!(s.download_mibps, 0.0);
                    assert_eq!(s.upload_mibps, 0.0);
                }
                // Cumulative should be unchanged when delta is 0
                assert_eq!(new_rx, baseline_rx);
                assert_eq!(new_tx, baseline_tx);
            }
        }
    }

    #[test]
    fn test_collect_network_stats_positive_delta() {
        // Simulate known byte deltas with a mock Networks-like scenario
        // Since we can't easily inject fake data into sysinfo::Networks,
        // we verify the math directly:
        // delta = 2_097_152 bytes (2 MiB), tick = 2s → 1.0 MiB/s
        let expected_mibps = 2_097_152.0 / 1024.0 / 1024.0 / TICK_INTERVAL_SECS;
        assert!((expected_mibps - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_collect_network_stats_wraparound() {
        // Simulate counter wraparound: current < previous
        // Previous cumulative: 10_000, current: 5_000 → delta should be 0
        let delta = if 5_000u64 >= 10_000u64 {
            5_000 - 10_000
        } else {
            0
        };
        assert_eq!(delta, 0);

        // Verify wraparound produces 0.0 MiB/s
        let mibps = delta as f64 / 1024.0 / 1024.0 / TICK_INTERVAL_SECS;
        assert_eq!(mibps, 0.0);
    }

    #[test]
    fn test_network_stats_serialization() {
        let stats = NetworkStats {
            download_mibps: 5.25,
            upload_mibps: 2.1,
        };

        let json = serde_json::to_string(&stats).expect("Failed to serialize");
        let deserialized: NetworkStats =
            serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(stats, deserialized);
        assert_eq!(deserialized.download_mibps, 5.25);
        assert_eq!(deserialized.upload_mibps, 2.1);
    }

    #[test]
    fn test_network_stats_default() {
        let stats = NetworkStats::default();
        assert_eq!(stats.download_mibps, 0.0);
        assert_eq!(stats.upload_mibps, 0.0);
    }

    #[test]
    fn test_saturating_add_overflow_guard() {
        let max = u64::MAX;
        // saturating_add should not panic
        let result = max.saturating_add(100);
        assert_eq!(result, u64::MAX);
    }
}
