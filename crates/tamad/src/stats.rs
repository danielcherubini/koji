//! Host stats collector for the tamad daemon.
//!
//! Stateful on purpose: CPU% is a *delta* between two samples taken on the
//! same `sysinfo::System`. Creating a fresh `System` per tick would yield a
//! meaningless (always-0-ish) CPU reading — the same reason
//! `tama-core/src/proxy/server/metrics.rs` holds one `System` across its
//! loop. `tick` is blocking (GPU detection shells out to nvidia-smi /
//! reads sysfs) and must be called via `tokio::task::spawn_blocking`.

use std::path::Path;
use std::sync::Arc;

use crate::state::TamadState;
use tama_core::tamad::GpuInfo;
use tama_core::tamad::ProcessInfo;
use tama_core::tamad::SystemStats;

/// Collects a full host stats snapshot (CPU/RAM/swap/disk + per-GPU info)
/// on a fixed cadence, reusing one `sysinfo::System` across ticks.
pub struct StatsCollector {
    state: Arc<TamadState>,
    /// Refreshed once per tick; persists across ticks so CPU% is a real
    /// inter-sample delta.
    sys: sysinfo::System,
    /// Refreshed per tick.
    disks: sysinfo::Disks,
}

impl StatsCollector {
    /// Build a collector and take one baseline sample so the first tick
    /// already has a meaningful CPU delta.
    pub fn new(state: Arc<TamadState>) -> Self {
        let mut sys = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::new()
                .with_cpu(sysinfo::CpuRefreshKind::everything())
                .with_memory(sysinfo::MemoryRefreshKind::everything()),
        );
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Self {
            state,
            sys,
            disks: sysinfo::Disks::new_with_refreshed_list(),
        }
    }

    /// Refresh all subsystems and return a full snapshot.
    ///
    /// Blocking — call from `tokio::task::spawn_blocking`.
    pub fn tick(&mut self, processes: Vec<ProcessInfo>) -> SystemStats {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();

        // CPU/RAM/swap straight from the persistent System (NOT from
        // SystemMetrics — swap is not populated by collect_system_metrics_with).
        let cpu_usage_percent = self.sys.global_cpu_info().cpu_usage() as f64;
        let memory_total_bytes = self.sys.total_memory() as i64;
        let memory_used_bytes = self.sys.used_memory() as i64;
        let swap_total_bytes = self.sys.total_swap() as i64;
        let swap_used_bytes = self.sys.used_swap() as i64;

        let (disk_total_bytes, disk_free_bytes) =
            Self::disk_usage_for(&mut self.disks, &self.state.models_dir);

        // Reuse the same System for GPU detection (its internals refresh
        // CPU/memory again — harmless; we only consume `.gpus`).
        let metrics = crate::gpu::system::collect_system_metrics_with(&mut self.sys);
        let gpus = map_gpus(&metrics.gpus);

        SystemStats {
            cpu_usage_percent,
            memory_total_bytes,
            memory_used_bytes,
            swap_total_bytes,
            swap_used_bytes,
            disk_total_bytes,
            disk_free_bytes,
            gpus,
            processes,
        }
    }

    /// Total/available bytes of the filesystem containing `dir`.
    ///
    /// Longest mount-point prefix of `dir` wins; the `/` mount is always a
    /// valid fallback, so a real host always resolves to a disk.
    fn disk_usage_for(disks: &mut sysinfo::Disks, dir: &Path) -> (i64, i64) {
        disks.refresh();
        let mut best_len: usize = 0;
        let mut best: Option<(u64, u64)> = None;
        for disk in disks.iter() {
            let mount = disk.mount_point();
            if dir.starts_with(mount) {
                let len = mount.components().count();
                if len > best_len {
                    best_len = len;
                    best = Some((disk.total_space(), disk.available_space()));
                }
            }
        }
        match best {
            Some((total, free)) => (total as i64, free as i64),
            // Empty disk list (shouldn't happen on a real host).
            None => (0, 0),
        }
    }
}

/// Map `GpuDeviceStats` list to proto `GpuInfo`.
fn map_gpus(gpus: &[tama_core::gpu::GpuDeviceStats]) -> Vec<GpuInfo> {
    gpus.iter()
        .enumerate()
        .map(|(position, g)| {
            // "GPU0" → 0; if the suffix is not a bare integer, use position.
            let digits: String = g.device_id.chars().filter(|c| c.is_ascii_digit()).collect();
            let index = digits.parse::<i32>().unwrap_or(position as i32);
            let (vram_total_bytes, vram_used_bytes) = match &g.vram {
                Some(v) => (
                    v.total_mib as i64 * 1024 * 1024,
                    v.used_mib as i64 * 1024 * 1024,
                ),
                None => (0, 0),
            };
            GpuInfo {
                index,
                name: g.name.clone(),
                // GpuDeviceStats carries no driver version today; the proto
                // field is reserved for the future.
                driver_version: String::new(),
                vram_total_bytes,
                vram_used_bytes,
                utilization_percent: g.utilization_pct.map(|u| u as f64).unwrap_or(0.0),
                temperature_c: g.temperature_c.map(|t| t as f64).unwrap_or(0.0),
                power_w: g.power_w.map(|p| p as f64).unwrap_or(0.0),
                fan_percent: g.fan_pct.map(|f| f as f64).unwrap_or(0.0),
            }
        })
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> Arc<TamadState> {
        let dir = tempfile::tempdir().unwrap();
        // Keep the tempdir alive for the test's lifetime via a leak-free
        // guard: the state only needs models_dir as a path string.
        let args = crate::CliArgs {
            addr: "127.0.0.1:50051".to_string(),
            protocol: "grpc".to_string(),
            name: Some("stats-test".to_string()),
            public_url: None,
            models_dir: Some(dir.path().join("models")),
            data_dir: Some(dir.keep()),
            no_replay_desired: false,
        };
        Arc::new(TamadState::from_cli(&args).unwrap())
    }

    /// A tick yields real memory numbers, a plausible non-NaN CPU across
    /// two ticks (proves the persistent-System delta works), positive disk
    /// figures for the models-dir filesystem, and structurally valid GPU
    /// entries on whatever hardware the test host has (GPU-less hosts
    /// yield an empty list without panicking).
    #[test]
    fn test_tick_host_snapshot() {
        let collector = StatsCollector::new(test_state());
        let mut collector = collector;

        let first = collector.tick(vec![]);
        for g in &first.gpus {
            assert!(
                g.vram_used_bytes <= g.vram_total_bytes,
                "vram_used must not exceed vram_total"
            );
            assert!((0.0..=100.0).contains(&g.utilization_percent));
        }
        assert!(
            first.memory_total_bytes > 0,
            "memory_total_bytes must be non-zero on a real host"
        );
        assert!(first.memory_used_bytes >= 0);
        assert!(
            first.disk_total_bytes > 0,
            "models-dir filesystem must have a positive total size"
        );
        assert!(first.disk_free_bytes >= 0);
        assert!(
            !first.cpu_usage_percent.is_nan(),
            "first tick CPU must not be NaN"
        );

        let second = collector.tick(vec![]);
        assert!(
            !second.cpu_usage_percent.is_nan(),
            "second tick CPU must not be NaN"
        );
        assert!(
            (0.0..=100.0).contains(&second.cpu_usage_percent),
            "CPU usage must be in 0..=100, got {}",
            second.cpu_usage_percent
        );

        // Processes pass through untouched.
        let proc = ProcessInfo {
            model_name: "m".to_string(),
            provider_name: "p".to_string(),
            pid: 1,
            alive: true,
            endpoint_url: "http://x".to_string(),
            status: "ready".to_string(),
            desired: false,
            restart_count: 0,
            max_restarts: 0,
        };
        let third = collector.tick(vec![proc.clone()]);
        assert_eq!(third.processes.len(), 1);
        assert_eq!(third.processes[0].model_name, "m");
    }

    /// `map_gpus` parses device indices, multiplies VRAM MiB→bytes, and
    /// defaults None fields to 0.
    #[test]
    fn test_map_gpus() {
        use tama_core::gpu::{GpuDeviceStats, GpuVendor, VramInfo};

        let gpus = vec![
            GpuDeviceStats {
                device_id: "GPU0".to_string(),
                vendor: GpuVendor::Nvidia,
                name: "RTX 4090".to_string(),
                utilization_pct: Some(42),
                vram: Some(VramInfo {
                    used_mib: 1024,
                    total_mib: 24576,
                }),
                temperature_c: Some(71),
                power_w: Some(350),
                fan_pct: Some(40),
                pci_bus: None,
                uuid: None,
            },
            GpuDeviceStats {
                device_id: "unknown".to_string(),
                vendor: GpuVendor::Amd,
                name: "Mystery".to_string(),
                utilization_pct: None,
                vram: None,
                temperature_c: None,
                power_w: None,
                fan_pct: None,
                pci_bus: None,
                uuid: None,
            },
        ];

        let out = map_gpus(&gpus);
        assert_eq!(out.len(), 2);

        assert_eq!(out[0].index, 0);
        assert_eq!(out[0].name, "RTX 4090");
        assert_eq!(out[0].driver_version, "");
        assert_eq!(out[0].vram_total_bytes, 24576 * 1024 * 1024);
        assert_eq!(out[0].vram_used_bytes, 1024 * 1024 * 1024);
        assert_eq!(out[0].utilization_percent, 42.0);
        assert_eq!(out[0].temperature_c, 71.0);
        assert_eq!(out[0].power_w, 350.0);
        assert_eq!(out[0].fan_percent, 40.0);

        // Unparseable device_id → position in the vec; None fields → 0.
        assert_eq!(out[1].index, 1);
        assert_eq!(out[1].vram_total_bytes, 0);
        assert_eq!(out[1].vram_used_bytes, 0);
        assert_eq!(out[1].utilization_percent, 0.0);
        assert_eq!(out[1].temperature_c, 0.0);
        assert_eq!(out[1].power_w, 0.0);
        assert_eq!(out[1].fan_percent, 0.0);
    }
}
