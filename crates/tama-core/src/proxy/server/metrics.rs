use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Start the system metrics collection background task.
///
/// Creates an in-memory history buffer seeded from SQLite, then spawns
/// a task that periodically collects system metrics, persists them,
/// updates the buffer, and broadcasts to subscribers.
///
/// Returns a `JoinHandle` that can be stored to prevent task cancellation.
pub fn start_metrics_collector(
    state: Arc<crate::proxy::ProxyState>,
) -> tokio::task::JoinHandle<()> {
    // Seed in-memory history buffer from SQLite.
    let mut history_buf: VecDeque<crate::gpu::MetricSample> = VecDeque::with_capacity(450);
    if let Some(seed_conn) = state.open_db() {
        if let Ok(rows) = crate::db::queries::get_recent_system_metrics(&seed_conn, 450) {
            for row in rows {
                history_buf.push_back(row_into_sample(&row));
            }
        }
    }

    // Spawn background task to refresh system metrics every 2s.
    // Each tick: collect metrics, build unified sample (system + inference),
    // persist to SQLite, update in-memory buffer, broadcast full buffer.
    let metrics_state = Arc::clone(&state);
    tokio::spawn(async move {
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
            *metrics_state.system_metrics.write().await = snapshot.clone();

            // 2. Read latest inference stats from watch channel
            let inference = *metrics_state.inference_stats.borrow();

            // 3. Collect model statuses
            let model_statuses = metrics_state.collect_model_statuses().await;
            let models_loaded = model_statuses.iter().filter(|m| m.state == "ready").count() as u64;

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
                tps: inference.as_ref().and_then(|i| i.tps),
                prompt_tps: inference.as_ref().and_then(|i| i.prompt_tps),
                cache_hit_pct: inference.as_ref().and_then(|i| i.cache_hit_pct),
                spec_accept_pct: inference.as_ref().and_then(|i| i.spec_accept_pct),
                spec_decoding_active: inference.map(|i| i.spec_decoding_active).unwrap_or(false),
                inference_last_updated_ms: inference.as_ref().map(|i| i.last_updated_ms),
                network: network_stats.clone(),
            };

            // 5. Persist to SQLite (include inference fields in SystemMetricsRow)
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
            // Persist (spawn_blocking, unchanged pattern)
            let retention_secs = metrics_state
                .config
                .read()
                .await
                .proxy
                .metrics_retention_secs;
            let cutoff_ms = sample.ts_unix_ms - (retention_secs as i128 * 1000) as i64;
            let db_state = Arc::clone(&metrics_state);
            let _ = tokio::task::spawn_blocking(move || {
                if let Some(conn) = db_state.open_db() {
                    if let Err(e) = crate::db::queries::insert_system_metric(&conn, &row, cutoff_ms)
                    {
                        tracing::warn!("failed to persist system metric: {}", e);
                    }
                }
            })
            .await;

            // 6. Update in-memory buffer
            history_buf.push_back(sample);
            while history_buf.len() > 450 {
                history_buf.pop_front();
            }

            // 7. Broadcast as Arc slice (no deep clone)
            let arc: Arc<[crate::gpu::MetricSample]> = history_buf.make_contiguous().into();
            let _ = metrics_state.metrics_tx.send(arc);

            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    })
}

/// Convert a `SystemMetricsRow` from SQLite into a `MetricSample`.
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
