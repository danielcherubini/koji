//! Host stats collector for the tamad daemon.
//!
//! Stateful on purpose: CPU% is a *delta* between two samples taken on the
//! same `sysinfo::System`. Creating a fresh `System` per tick would yield a
//! meaningless (always-0-ish) CPU reading — the same reason
//! `tama-core/src/proxy/server/metrics.rs` holds one `System` across its
//! loop. `tick` is blocking (GPU detection shells out to nvidia-smi /
//! reads sysfs) and must be called via `tokio::task::spawn_blocking`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::state::TamadState;
use crate::vllm_metrics;
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
    /// Spec-decode scrape state per model_name (all ready+alive backends,
    /// any engine — the scraped body determines vLLM-ness).
    spec: HashMap<String, SpecState>,
    /// Overridable in tests (`Duration::ZERO` = scrape every tick).
    scrape_interval: Duration,
    /// Blocking HTTP client for `/metrics` scrapes (per-scrape timeout),
    /// or `None` when the client could not be built. Built lazily on the
    /// first tick — constructing a blocking reqwest client inside an
    /// async context panics, and `new` runs there at service boot — while
    /// a tick always runs via `spawn_blocking`.
    ///
    /// A build failure (e.g. a TLS-provider init on a misconfigured host)
    /// is NOT fatal: it stores `None` and spec scraping is silently
    /// disabled (one-time `debug!`), so a scrape problem's blast radius
    /// never extends past this feature to the rest of the host stats.
    http: OnceLock<Option<reqwest::blocking::Client>>,
}

/// Per-endpoint spec-decode scrape state. `prev` is the last cumulative
/// counter set (diffed on the next scrape); the `last_*` fields are the
/// most recent observation until it goes stale or is evicted.
#[derive(Debug, Default)]
struct SpecState {
    /// Last cumulative counter set (None until the first successful parse).
    prev: Option<vllm_metrics::SpecCounters>,
    /// The last scraped body contained the vLLM spec-decode counters.
    is_vllm: bool,
    /// Last scrape attempt, for the per-endpoint scrape throttle.
    last_scrape: Option<Instant>,
    /// Acceptance rate of the last window with spec traffic.
    last_rate_pct: Option<f64>,
    /// Whether the last observation window had spec traffic.
    last_active: bool,
    /// Unix millis of the last observation (poison-pill for freshness).
    last_obs_ms: i64,
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
            spec: HashMap::new(),
            scrape_interval: vllm_metrics::SCRAPE_INTERVAL,
            http: OnceLock::new(),
        }
    }

    /// Lazily build the blocking scrape client (first-tick, from a
    /// non-async thread) and return an owned handle. Cloning a
    /// `Client` is cheap (clones share the underlying connection pool),
    /// and an owned copy keeps this call from holding a borrow of `self`
    /// across the mutable `self.spec` work in the scrape loop.
    ///
    /// A build failure (e.g. TLS/OpenSSL init on a misconfigured host)
    /// is not fatal: it returns `None` with a one-time `debug!` rather
    /// than panicking — a panic out of this `spawn_blocking` tick would
    /// drop the whole host stats stream and re-panic on every proxy
    /// reconnect until tamad is restarted. The happy path is unchanged.
    fn http(&self) -> Option<reqwest::blocking::Client> {
        self.http
            .get_or_init(|| {
                match reqwest::blocking::Client::builder()
                    .timeout(vllm_metrics::PER_SCRAPE_TIMEOUT)
                    .build()
                {
                    Ok(client) => Some(client),
                    Err(e) => {
                        tracing::debug!("spec scrape disabled: {e}");
                        None
                    }
                }
            })
            .clone()
    }

    /// Override the per-endpoint scrape throttle (tests use ~0ms).
    #[cfg(test)]
    pub(crate) fn with_scrape_interval(mut self, interval: Duration) -> Self {
        self.scrape_interval = interval;
        self
    }

    /// Refresh all subsystems and return a full snapshot.
    ///
    /// Blocking — call from `tokio::task::spawn_blocking`.
    pub fn tick(&mut self, mut processes: Vec<ProcessInfo>) -> SystemStats {
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

        // Spec-decode observation: scrape ready+alive engine /metrics
        // endpoints and stamp the diffed acceptance rate onto their
        // ProcessInfo entries. Never blocks more than the scrape budget.
        self.scrape_spec(&mut processes);

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

    /// Scrape `/metrics` for every ready+alive process and diff the spec
    /// counters; stamp `spec_accept_pct` / `spec_decoding_active` on the
    /// matching entries. Blocking (HTTP) — legitimate only because `tick`
    /// already runs via `spawn_blocking`. The tick must never linger: the
    /// proxy's 5s `LIVE_FRAME_MAX_AGE` freshness gate blanks every model on
    /// the host if a tick overshoots, so scrapes are throttled per endpoint
    /// and the cumulative scrape work is capped at `TICK_SCRAPE_BUDGET`.
    /// The budget is preflighted *before* a scrape is started: a send
    /// can run up to the full `PER_SCRAPE_TIMEOUT` before timing out and
    /// cannot be interrupted, so a post-hoc check would admit one extra
    /// scrape and let a hanging engine push the tick past the budget.
    /// Skipped models simply retry next tick.
    fn scrape_spec(&mut self, processes: &mut [ProcessInfo]) {
        // Scraping is a logged no-op when the client could not be built
        // (the one-time `debug!` fired inside `http()`); the rest of the
        // tick proceeds with host metrics.
        let Some(client) = self.http() else {
            return;
        };
        let now = Instant::now();
        let now_ms = unix_now_ms();
        let mut scrape_elapsed = Duration::ZERO;

        for p in processes.iter_mut() {
            if p.status != "ready" || !p.alive {
                continue;
            }
            let model_name = p.model_name.clone();
            let throttled = self.spec.get(&model_name).is_some_and(|s| {
                s.last_scrape.is_some_and(|t| {
                    self.scrape_interval > Duration::ZERO
                        && now.duration_since(t) < self.scrape_interval
                })
            });
            // Preflight the budget: admit a new scrape only when the
            // remaining `TICK_SCRAPE_BUDGET` can cover a full
            // `PER_SCRAPE_TIMEOUT` — the send can take up to that long
            // before timing out and cannot be cancelled once started, so
            // refusing it now keeps total scrape work within the budget
            // even against a hanging engine.
            if throttled
                || scrape_elapsed + vllm_metrics::PER_SCRAPE_TIMEOUT
                    >= vllm_metrics::TICK_SCRAPE_BUDGET
            {
                continue;
            }
            let Some(url) = vllm_metrics::metrics_url_for(&p.endpoint_url) else {
                continue;
            };

            let t0 = Instant::now();
            let outcome = client.get(&url).send().and_then(|r| {
                let ok = r.status().is_success();
                r.text().map(move |t| (ok, t))
            });
            scrape_elapsed += t0.elapsed();
            let s = self.spec.entry(model_name).or_default();
            s.last_scrape = Some(Instant::now());

            // ANY failure (send error, non-2xx, text error): debug-log
            // (never warn — down engines would spam the log), keep the
            // last observation, move on.
            let text = match outcome {
                Ok((true, t)) => t,
                Ok((false, _)) => {
                    tracing::debug!("{} spec scrape: non-2xx response", p.model_name);
                    continue;
                }
                Err(e) => {
                    tracing::debug!("{} spec scrape failed: {e}", p.model_name);
                    continue;
                }
            };

            match vllm_metrics::parse_spec_metrics(&text) {
                // Non-vLLM engine (or a vLLM build without the spec
                // counters): leave the entry at its `to_process_info`
                // defaults and memo that it is not a vLLM engine.
                None => s.is_vllm = false,
                Some(cur) => {
                    s.is_vllm = true;
                    if let Some((pct, active)) = vllm_metrics::observe(s.prev, cur) {
                        s.last_rate_pct = Some(pct);
                        s.last_active = active;
                        s.last_obs_ms = now_ms;
                    }
                    s.prev = Some(cur);
                }
            }
        }

        // Evict state for models no longer in this tick's process list — a
        // restarted engine gets a fresh `prev` (its counters were reset).
        let current: std::collections::HashSet<&str> =
            processes.iter().map(|p| p.model_name.as_str()).collect();
        self.spec.retain(|name, _| current.contains(name.as_str()));

        // Emit observations to the tracked entries. An entry summarizes as
        // inactive (defaults) as soon as it stops being ready, or when the
        // observation goes stale (older than STALE_MS).
        for p in processes.iter_mut() {
            if p.status != "ready" || !p.alive {
                continue;
            }
            let Some(s) = self.spec.get(&p.model_name) else {
                continue;
            };
            let fresh = now_ms - s.last_obs_ms <= vllm_metrics::STALE_MS;
            p.spec_accept_pct = if s.is_vllm && fresh {
                s.last_rate_pct
            } else {
                None
            };
            p.spec_decoding_active = s.is_vllm && fresh && s.last_active;
        }
    }
}

/// Unix time in millis (falls back to 0 before the epoch — pre-epoch
/// always reads as stale, which is the safe side).
fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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

        // Processes pass through untouched (invalid port → scrape fails
        // silently, spec fields stay at their defaults; no DNS in tests).
        let proc = ProcessInfo {
            model_name: "m".to_string(),
            provider_name: "p".to_string(),
            pid: 1,
            alive: true,
            endpoint_url: "http://127.0.0.1:0".to_string(),
            status: "ready".to_string(),
            desired: false,
            restart_count: 0,
            max_restarts: 0,
            spec_accept_pct: None,
            spec_decoding_active: false,
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

    /// A ready+alive process feeding a mock engine /metrics endpoint.
    fn spec_process(endpoint_url: String) -> ProcessInfo {
        ProcessInfo {
            model_name: "m".to_string(),
            provider_name: "vllm".to_string(),
            pid: 1,
            alive: true,
            endpoint_url,
            status: "ready".to_string(),
            desired: false,
            restart_count: 0,
            max_restarts: 0,
            spec_accept_pct: None,
            spec_decoding_active: false,
        }
    }

    /// VLLM body: three spec counters, one label set each.
    fn vllm_body(drafts: f64, draft_tokens: f64, accepted: f64) -> String {
        format!("\n# HELP vllm:spec_decode_num_drafts_total Total spec iterations\n# TYPE vllm:spec_decode_num_drafts_total counter\nvllm:spec_decode_num_drafts_total{{model_name=\"m\",engine=\"0\"}} {drafts}\nvllm:spec_decode_num_draft_tokens_total{{model_name=\"m\",engine=\"0\"}} {draft_tokens}\nvllm:spec_decode_num_accepted_tokens_total{{model_name=\"m\",engine=\"0\"}} {accepted}\n")
    }

    /// Two ticks against mock vLLM engines: the first scrape only seeds
    /// the cumulative counters (no delta yet → defaults), and the second,
    /// after the counters advance, reports the windowed acceptance rate
    /// and marks the engine active. Detection is body-driven: only the
    /// metrics *body* decides vLLM-ness. Ticks run on a worker thread —
    /// the blocking reqwest client must not run in an async context.
    #[test]
    fn test_tick_spec_scrape_vllm_positive() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // Tick-1 endpoint seeds the cumulative counters at zero; the tick-2
        // endpoint serves the real-log window (165/371 ≈ 44.5%).
        let seed = rt.block_on(MockServer::start());
        let next = rt.block_on(MockServer::start());
        rt.block_on(
            Mock::given(method("GET"))
                .and(path("/metrics"))
                .respond_with(ResponseTemplate::new(200).set_body_string(vllm_body(0.0, 0.0, 0.0)))
                .mount(&seed),
        );
        rt.block_on(
            Mock::given(method("GET"))
                .and(path("/metrics"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_string(vllm_body(115.0, 371.0, 165.0)),
                )
                .mount(&next),
        );

        let (first, second) = std::thread::spawn(move || {
            let mut collector =
                StatsCollector::new(test_state()).with_scrape_interval(Duration::from_millis(1));
            let first = collector.tick(vec![spec_process(seed.uri())]);
            let second = collector.tick(vec![spec_process(next.uri())]);
            (first, second)
        })
        .join()
        .unwrap();

        // Tick 1: first scrape seeds prev — no window yet.
        assert_eq!(first.processes.len(), 1);
        assert!(first.processes[0].spec_accept_pct.is_none());
        assert!(!first.processes[0].spec_decoding_active);

        let Some(pct) = second.processes[0].spec_accept_pct else {
            panic!("expected a spec acceptance rate on tick 2");
        };
        assert!((44.4..=44.55).contains(&pct), "expected ~44.47, got {pct}");
        assert!(second.processes[0].spec_decoding_active);
    }

    /// Non-vLLM body (llama.cpp-style metrics) → defaults on both ticks;
    /// a non-READY process is untouched regardless of engine.
    #[test]
    fn test_tick_spec_scrape_non_vllm_negative() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let server = rt.block_on(MockServer::start());
        let body = "llamacpp_duration_s{status_stage=\"0\",lifespan_stage=\"0\",vram_stage=\"3\"} 2.5\nllamacpp_inference_duration_s{model=\"m\"} 1.0\n";
        rt.block_on(
            Mock::given(method("GET"))
                .and(path("/metrics"))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .mount(&server),
        );

        let (first, second) = std::thread::spawn(move || {
            let mut collector =
                StatsCollector::new(test_state()).with_scrape_interval(Duration::from_millis(1));
            let first = collector.tick(vec![spec_process(server.uri())]);

            // A stopped process is never scraped and keeps its defaults.
            let mut stopped = spec_process(server.uri());
            stopped.status = "stopped".to_string();
            stopped.alive = false;
            let second = collector.tick(vec![spec_process(server.uri()), stopped]);
            (first, second)
        })
        .join()
        .unwrap();

        assert!(first.processes[0].spec_accept_pct.is_none());
        assert!(!first.processes[0].spec_decoding_active);
        assert!(second.processes[0].spec_accept_pct.is_none());
        assert!(!second.processes[0].spec_decoding_active);
        assert!(second.processes[1].spec_accept_pct.is_none());
        assert!(!second.processes[1].spec_decoding_active);
    }

    /// Dead engine: nothing listening on the endpoint → the scrape fails,
    /// the tick completes, and the process keeps its defaults.
    #[test]
    fn test_tick_spec_scrape_dead_endpoint() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let endpoint = format!("http://127.0.0.1:{port}/v1");

        let mut collector =
            StatsCollector::new(test_state()).with_scrape_interval(Duration::from_millis(1));

        let out = collector.tick(vec![spec_process(endpoint)]);
        assert_eq!(out.processes.len(), 1);
        assert!(out.processes[0].spec_accept_pct.is_none());
        assert!(!out.processes[0].spec_decoding_active);
    }

    /// Budget-preflight regression: when a slow first scrape leaves less
    /// than `PER_SCRAPE_TIMEOUT` of headroom, the next scrape in the same
    /// tick is refused (never sent) instead of only being checked after
    /// it has (possibly needlessly) finished. Deterministic: verified via
    /// the mock's request log — one 1.1s-slow engine spends 1.1s, leaving
    /// <2s of the 3s tick budget, so the second model's scrape is refused
    /// and exactly ONE request reaches `/metrics`.
    #[test]
    fn test_tick_spec_scrape_budget_preflight_refuses_overshoot() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // Engine answers /metrics after ~1.1s — under the 2s
        // per-scrape timeout, but enough that a second scrape would push
        // the tick past the 3s budget.
        let slow = rt.block_on(MockServer::start());
        rt.block_on(
            Mock::given(method("GET"))
                .and(path("/metrics"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(vllm_body(0.0, 0.0, 0.0))
                        .set_delay(Duration::from_millis(1100)),
                )
                .mount(&slow),
        );
        let uri = slow.uri();

        // Two ready+alive models against the same slow endpoint; the
        // per-endpoint scrape throttle is disabled so only the per-tick
        // budget decides.
        let a = spec_process(uri.clone());
        let mut b = spec_process(uri.clone());
        b.model_name = "m2".to_string();

        std::thread::spawn(move || {
            let mut collector =
                StatsCollector::new(test_state()).with_scrape_interval(Duration::ZERO);
            collector.tick(vec![a, b]);
        })
        .join()
        .unwrap();

        // Exactly one scrape was sent: model A spends ~1.1s, leaving
        // 3s - 1.1s < 2s (PER_SCRAPE_TIMEOUT), so model B's scrape is
        // refused by the preflight and never reaches the mock.
        let reqs = rt
            .block_on(slow.received_requests())
            .expect("request recording is on");
        let metrics_hits = reqs.iter().filter(|r| r.url.path() == "/metrics").count();
        assert_eq!(
            metrics_hits, 1,
            "preflight must refuse a scrape that could push the tick past its budget"
        );
    }
}
