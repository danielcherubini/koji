use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Width of each aggregation bucket in milliseconds (30 seconds).
const BUCKET_MS: i64 = 30_000;

/// Maximum number of frozen (complete) buckets to retain in the ring.
/// Together with the trailing in-progress bucket this yields ~31 bars for
/// a 15-minute window.
const MAX_FROZEN_BUCKETS: usize = 30;

/// Floor a Unix millisecond timestamp to the start of its 30s wall-clock
/// bucket. Stable across ticks — the boundary depends only on the timestamp,
/// not on when the collector started.
fn bucket_start(ts_ms: i64) -> i64 {
    (ts_ms / BUCKET_MS) * BUCKET_MS
}

/// Accumulator for the in-progress 30s bucket. Sums each 2s sample's fields
/// and produces a [`MetricBucket`] with averaged values when frozen or polled.
///
/// When a new sample's timestamp falls in a different 30s window than the
/// accumulator's, the caller freezes the old bucket (via [`to_bucket`]) and
/// starts a fresh accumulator for the new window. This guarantees completed
/// buckets are immutable — only the trailing in-progress bucket changes as
/// new samples arrive.
struct BucketAccumulator {
    bucket_start_ms: i64,
    cpu_sum: f64,
    ram_used_sum: f64,
    ram_total_last: u64,
    network_dl_sum: f64,
    network_ul_sum: f64,
    has_network: bool,
    /// Per-GPU running utilization sum. Index aligns with the order GPUs
    /// appear in `MetricSample.gpus`. Resized as new GPUs appear; a missing
    /// index is treated as 0.0 utilization.
    gpu_util_sums: Vec<f64>,
    /// Running sum of generation tok/s from samples that reported values.
    tps_sum: f64,
    /// Count of samples that reported a non-None tps value.
    tps_count: usize,
    /// Running sum of prompt tok/s from samples that reported values.
    prompt_tps_sum: f64,
    /// Count of samples that reported a non-None prompt_tps value.
    prompt_tps_count: usize,
    count: usize,
}

impl BucketAccumulator {
    fn new(bucket_start_ms: i64) -> Self {
        Self {
            bucket_start_ms,
            cpu_sum: 0.0,
            ram_used_sum: 0.0,
            ram_total_last: 0,
            network_dl_sum: 0.0,
            network_ul_sum: 0.0,
            has_network: false,
            gpu_util_sums: Vec::new(),
            tps_sum: 0.0,
            tps_count: 0,
            prompt_tps_sum: 0.0,
            prompt_tps_count: 0,
            count: 0,
        }
    }

    /// Add a 2s sample's graphable fields to this bucket's running sums.
    fn add(&mut self, sample: &crate::gpu::MetricSample) {
        self.cpu_sum += sample.cpu_usage_pct as f64;
        self.ram_used_sum += sample.ram_used_mib as f64;
        self.ram_total_last = sample.ram_total_mib;
        if let Some(ref net) = sample.network {
            self.network_dl_sum += net.download_mibps;
            self.network_ul_sum += net.upload_mibps;
            self.has_network = true;
        }
        // Track per-GPU utilization sums. Resize to fit the highest GPU index
        // seen so far; missing indices stay 0.0 (treated as 0% util).
        for (i, gpu) in sample.gpus.iter().enumerate() {
            if i >= self.gpu_util_sums.len() {
                self.gpu_util_sums.resize(i + 1, 0.0);
            }
            self.gpu_util_sums[i] += gpu.utilization_pct.unwrap_or(0) as f64;
        }
        // Accumulate inference stats (only from samples that reported values)
        if let Some(tps) = sample.tps {
            self.tps_sum += tps as f64;
            self.tps_count += 1;
        }
        if let Some(prompt_tps) = sample.prompt_tps {
            self.prompt_tps_sum += prompt_tps as f64;
            self.prompt_tps_count += 1;
        }
        self.count += 1;
    }

    /// Produce a [`MetricBucket`] from the accumulated sums. `complete`
    /// should be `true` when the 30s window has elapsed (frozen) or `false`
    /// for the trailing in-progress bucket.
    fn to_bucket(&self, complete: bool) -> crate::gpu::MetricBucket {
        let n = self.count.max(1) as f64;
        crate::gpu::MetricBucket {
            ts_unix_ms: self.bucket_start_ms,
            cpu_usage_pct: (self.cpu_sum / n) as f32,
            ram_used_mib: (self.ram_used_sum / n) as u64,
            ram_total_mib: self.ram_total_last,
            network: if self.has_network {
                Some(crate::network::NetworkStats {
                    download_mibps: self.network_dl_sum / n,
                    upload_mibps: self.network_ul_sum / n,
                })
            } else {
                None
            },
            gpu_utils: self.gpu_util_sums.iter().map(|s| (s / n) as f32).collect(),
            tps: if self.tps_count > 0 {
                (self.tps_sum / self.tps_count as f64) as f32
            } else {
                0.0
            },
            prompt_tps: if self.prompt_tps_count > 0 {
                (self.prompt_tps_sum / self.prompt_tps_count as f64) as f32
            } else {
                0.0
            },
            complete,
        }
    }
}

/// Feed a sample into the accumulator, freezing the previous bucket when the
/// sample crosses a 30s boundary. Shared between DB-seed replay and the live
/// collection loop so both paths produce identical bucket structure.
fn feed_sample(
    frozen: &mut VecDeque<crate::gpu::MetricBucket>,
    accum: &mut Option<BucketAccumulator>,
    sample: &crate::gpu::MetricSample,
) {
    let bs = bucket_start(sample.ts_unix_ms);
    if let Some(a) = accum.as_mut() {
        if bs > a.bucket_start_ms {
            // Sample crossed into a new 30s window — freeze the old bucket.
            frozen.push_back(a.to_bucket(true));
            while frozen.len() > MAX_FROZEN_BUCKETS {
                frozen.pop_front();
            }
            *a = BucketAccumulator::new(bs);
        }
    } else {
        *accum = Some(BucketAccumulator::new(bs));
    }
    accum.as_mut().unwrap().add(sample);
}

/// Start the system metrics collection background task.
///
/// Creates an in-memory history buffer seeded from Postgres, then spawns
/// a task that periodically collects system metrics, persists them,
/// updates the buffer, and broadcasts to subscribers.
///
/// Returns a `JoinHandle` that can be stored to prevent task cancellation.
pub fn start_metrics_collector(
    state: Arc<crate::proxy::ProxyState>,
) -> tokio::task::JoinHandle<()> {
    let mut frozen_buckets: VecDeque<crate::gpu::MetricBucket> =
        VecDeque::with_capacity(MAX_FROZEN_BUCKETS + 1);
    let mut accum: Option<BucketAccumulator> = None;

    // Spawn background task to refresh system metrics every 2s.
    // Each tick: collect metrics, build unified sample (system + inference),
    // persist to Postgres, update in-memory buffer, broadcast full buffer.
    let metrics_state = Arc::clone(&state);
    tokio::spawn(async move {
        // Seed in-memory bucket accumulator from Postgres. Replay the most
        // recent raw rows through the same feed_sample() path used by the
        // live loop so the seeded buckets have identical structure to live ones.
        if let Some(pool) = metrics_state.db_pool() {
            if let Ok(rows) = crate::db::queries::get_recent_system_metrics(&pool, 450).await {
                for row in rows {
                    let sample = row_into_sample(&row);
                    feed_sample(&mut frozen_buckets, &mut accum, &sample);
                }
            }
        }

        let mut sys = sysinfo::System::new();

        // Network detection — done once before the loop
        let primary_interface = crate::network::get_primary_interface();
        if let Some(ref iface) = primary_interface {
            tracing::info!("Using primary network interface: {}", iface);
        }

        // Before the loop: Create Networks instance and establish baseline
        let mut net = sysinfo::Networks::new_with_refreshed_list();
        let mut prev_rx: u64 = 0;
        let mut prev_tx: u64 = 0;

        // First refresh to establish baseline
        net.refresh();

        // Capture baseline cumulative values so the first tick doesn't include
        // all historical bytes since system boot
        if let Some(ref iface) = primary_interface {
            if let Some(iface_data) = net.get(iface) {
                prev_rx = iface_data.total_received();
                prev_tx = iface_data.total_transmitted();
            }
        }

        loop {
            // 1. Collect system metrics (spawn_blocking, unchanged pattern)
            let (snapshot, returned_sys) = tokio::task::spawn_blocking(move || {
                let snapshot = crate::gpu::collect_system_metrics_with(&mut sys);
                (snapshot, sys)
            })
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("system metrics collection panicked: {}", e);
                (crate::gpu::SystemMetrics::default(), sysinfo::System::new())
            });
            sys = returned_sys;

            // 1b. Collect network stats
            let (network_stats, cum_rx, cum_tx) = if let Some(ref iface) = primary_interface {
                let (stats, rx, tx) =
                    crate::network::collect_network_stats(iface, &mut net, prev_rx, prev_tx);
                prev_rx = rx;
                prev_tx = tx;
                (stats, rx, tx)
            } else {
                (None, 0, 0)
            };

            // 1c. Attach network stats to the system snapshot
            let mut snapshot = snapshot;
            snapshot.network = network_stats.clone();

            // Update the cached snapshot read by /tama/v1/system/health.
            metrics_state
                .metrics
                .set_system_metrics(snapshot.clone())
                .await;

            // 2. Read latest inference stats from watch channel (per-server HashMap)
            // Aggregate across all servers: use the most recently updated server's stats
            // for numeric fields; OR the spec_decoding_active flag across all servers.
            let inference_map = metrics_state.metrics.inference_stats_snapshot();
            let latest_server = inference_map.values().max_by_key(|s| s.last_updated_ms);

            // Gate tps/prompt_tps on freshness — if no inference update within the
            // bucket window (30s), treat values as stale so buckets show 0 tok/s.
            let stale_threshold_ms = BUCKET_MS;
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            let (tps, prompt_tps) = if let Some(server) = latest_server {
                if now_ms - server.last_updated_ms <= stale_threshold_ms {
                    (server.tps, server.prompt_tps)
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };
            let cache_hit_pct = latest_server.and_then(|s| s.cache_hit_pct);
            let spec_accept_pct = latest_server.and_then(|s| s.spec_accept_pct);
            let spec_decoding_active = inference_map.values().any(|s| s.spec_decoding_active);
            let inference_last_updated_ms = latest_server.map(|s| s.last_updated_ms);

            // 3. Collect model statuses
            let model_statuses = metrics_state.collect_model_state_snapshots().await;
            let models_loaded = model_statuses
                .iter()
                .filter(|m| matches!(m.state, crate::gpu::ModelState::Ready))
                .count() as u64;

            // 4. Build unified MetricSample WITH inference fields
            let sample = crate::gpu::MetricSample {
                ts_unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                cpu_usage_pct: snapshot.cpu_usage_pct,
                ram_used_mib: snapshot.ram_used_mib,
                ram_total_mib: snapshot.ram_total_mib,
                gpu_utilization_pct: snapshot.gpu_utilization_pct,
                vram: snapshot.vram.clone(),
                gpus: snapshot.gpus.clone(),
                models_loaded,
                models: model_statuses,
                tps,
                prompt_tps,
                cache_hit_pct,
                spec_accept_pct,
                spec_decoding_active,
                inference_last_updated_ms,
                network: network_stats.clone(),
            };

            // 5. Persist to Postgres (include inference fields in SystemMetricsRow)
            let row = crate::db::queries::SystemMetricsRow {
                ts_unix_ms: sample.ts_unix_ms,
                cpu_usage_pct: sample.cpu_usage_pct,
                ram_used_mib: sample.ram_used_mib as i64,
                ram_total_mib: sample.ram_total_mib as i64,
                gpu_utilization_pct: sample.gpu_utilization_pct.map(|v| v as i64),
                vram_used_mib: sample.vram.as_ref().map(|v| v.used_mib as i64),
                vram_total_mib: sample.vram.as_ref().map(|v| v.total_mib as i64),
                models_loaded: sample.models_loaded as i64,
                tps: sample.tps.map(|v| v as f64),
                prompt_tps: sample.prompt_tps.map(|v| v as f64),
                cache_hit_pct: sample.cache_hit_pct.map(|v| v as f64),
                spec_accept_pct: sample.spec_accept_pct.map(|v| v as f64),
                net_rx_bytes: Some(cum_rx as i64),
                net_tx_bytes: Some(cum_tx as i64),
            };
            // Persist (non-fatal — a DB hiccup must not kill the collector)
            let retention_secs = metrics_state
                .config
                .read()
                .await
                .proxy
                .metrics_retention_secs;
            let cutoff_ms = sample.ts_unix_ms - (retention_secs as i128 * 1000) as i64;
            if let Some(pool) = metrics_state.db_pool() {
                if let Err(e) =
                    crate::db::queries::insert_system_metric(&pool, &row, cutoff_ms).await
                {
                    tracing::warn!("failed to persist system metric: {}", e);
                }
            }

            // 6. Feed this sample into the bucket accumulator. This freezes
            // the previous bucket when the sample crosses a 30s boundary,
            // guaranteeing completed buckets are immutable thereafter.
            feed_sample(&mut frozen_buckets, &mut accum, &sample);

            // Derive the trailing in-progress bucket from the accumulator.
            let in_progress = accum.as_ref().map(|a| a.to_bucket(false));

            // 7. Broadcast: frozen buckets + in-progress last + current
            //    (instantaneous CPU/RAM/Network + GPU/model/inference state).
            let mut all_buckets = frozen_buckets.make_contiguous().to_vec();
            if let Some(b) = in_progress {
                all_buckets.push(b);
            }
            let current = sample.clone().into_current();
            let snapshot = crate::gpu::MetricsSnapshot {
                buckets: all_buckets,
                current,
            };
            metrics_state.metrics.publish_metrics(snapshot);

            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    })
}

/// Convert a `SystemMetricsRow` from Postgres into a `MetricSample`.
/// Used to seed the in-memory history buffer on startup.
fn row_into_sample(row: &crate::db::queries::SystemMetricsRow) -> crate::gpu::MetricSample {
    crate::gpu::MetricSample {
        ts_unix_ms: row.ts_unix_ms,
        cpu_usage_pct: row.cpu_usage_pct,
        ram_used_mib: row.ram_used_mib.max(0) as u64,
        ram_total_mib: row.ram_total_mib.max(0) as u64,
        gpu_utilization_pct: row.gpu_utilization_pct.and_then(|v| {
            if (0..=100).contains(&v) {
                Some(v as u8)
            } else {
                None
            }
        }),
        vram: row.vram_used_mib.and_then(|used| {
            row.vram_total_mib.map(|total| crate::gpu::VramInfo {
                used_mib: used.max(0) as u64,
                total_mib: total.max(0) as u64,
            })
        }),
        models_loaded: row.models_loaded.max(0) as u64,
        models: vec![], // Not stored in DB — seeded samples have no model status
        gpus: vec![],   // historical rows don't store per-GPU; left empty
        tps: row.tps.map(|v| v as f32),
        prompt_tps: row.prompt_tps.map(|v| v as f32),
        cache_hit_pct: row.cache_hit_pct.map(|v| v as f32),
        spec_accept_pct: row.spec_accept_pct.map(|v| v as f32),
        spec_decoding_active: false,     // Transient — not in DB
        inference_last_updated_ms: None, // Transient — not in DB
        network: None,                   // Throughput not reconstructable from single row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `MetricSample` with only the fields needed for
    /// `BucketAccumulator` tests. All non-inference fields use defaults.
    fn sample(tps: Option<f32>, prompt_tps: Option<f32>) -> crate::gpu::MetricSample {
        crate::gpu::MetricSample {
            ts_unix_ms: 0,
            cpu_usage_pct: 0.0,
            ram_used_mib: 0,
            ram_total_mib: 0,
            gpu_utilization_pct: None,
            vram: None,
            gpus: vec![],
            models_loaded: 0,
            models: vec![],
            tps,
            prompt_tps,
            cache_hit_pct: None,
            spec_accept_pct: None,
            spec_decoding_active: false,
            inference_last_updated_ms: None,
            network: None,
        }
    }

    /// Mixed Some/None samples average only over Some values.
    ///
    /// Three samples are added: two with Some(tps) and one with None.
    /// The bucket tps must be the mean of the two Some values, not diluted
    /// by the None sample. The same logic applies to prompt_tps.
    #[test]
    fn test_mixed_some_none_averages_only_some() {
        let mut acc = BucketAccumulator::new(0);

        acc.add(&sample(Some(10.0), Some(20.0)));
        acc.add(&sample(None, None));
        acc.add(&sample(Some(30.0), Some(40.0)));

        let bucket = acc.to_bucket(true);

        // tps: mean of 10.0 and 30.0 = 20.0 (None sample excluded)
        assert_eq!(bucket.tps, 20.0);
        // prompt_tps: mean of 20.0 and 40.0 = 30.0 (None sample excluded)
        assert_eq!(bucket.prompt_tps, 30.0);
    }

    /// All None produces 0.0 for both tps and prompt_tps.
    #[test]
    fn test_all_none_produces_zero() {
        let mut acc = BucketAccumulator::new(0);

        acc.add(&sample(None, None));
        acc.add(&sample(None, None));
        acc.add(&sample(None, None));

        let bucket = acc.to_bucket(true);

        assert_eq!(bucket.tps, 0.0);
        assert_eq!(bucket.prompt_tps, 0.0);
    }

    /// A single sample passes its tps/prompt_tps values through unchanged.
    #[test]
    fn test_single_sample_passes_through() {
        let mut acc = BucketAccumulator::new(0);

        acc.add(&sample(Some(55.5), Some(123.0)));

        let bucket = acc.to_bucket(true);

        assert_eq!(bucket.tps, 55.5);
        assert_eq!(bucket.prompt_tps, 123.0);
    }
}
