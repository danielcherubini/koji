use super::*;
use crate::api::error::error_response;
use crate::api::helpers::shared_repository;
use crate::web_types::WebState;
use tama_core::proxy::tama_handlers::OkResponse;
use tama_core::proxy::ProxyState;

// ── Handler: Get benchmark result ─────────────────────────────────────

pub async fn get_benchmark_result(
    Extension(web_state): Extension<WebState>,
    State(_state): State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let jobs = match web_state.jobs.as_ref() {
        Some(j) => j.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Job manager not available",
                None,
            )
        }
    };

    let job = match jobs.get(&job_id).await {
        Some(j) => j.clone(),
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                "Job not found",
                Some("NotFoundError"),
            )
        }
    };

    let state = job.state.read().await;
    let error = state.error.clone();
    let status = format!("{:?}", state.status);
    drop(state);

    // Read log lines for context
    let log_lines: Vec<String> = {
        let head = job.log_head.read().await;
        let tail = job.log_tail.read().await;
        let mut lines: Vec<String> = head.iter().cloned().collect();
        lines.extend(tail.iter().cloned());
        lines
    };

    // Get benchmark results if available
    let benchmark_results = {
        let results = job.benchmark_results.read().await;
        let cloned = results.clone();
        tracing::info!(
            "get_benchmark_result: benchmark_results={:?}",
            cloned.is_some()
        );
        cloned
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "job_id": job_id,
            "status": status,
            "error": error,
            "log_lines": log_lines,
            "benchmark_results": benchmark_results,
        })),
    )
        .into_response()
}

// ── Handler: SSE events for benchmark progress ────────────────────────

pub async fn benchmark_events(
    Extension(web_state): Extension<WebState>,
    State(_state): State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
    let jobs = match web_state.jobs.as_ref() {
        Some(j) => j.clone(),
        None => {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    let job = match jobs.get(&job_id).await {
        Some(j) => j.clone(),
        None => {
            return Err(StatusCode::NOT_FOUND);
        }
    };

    let stream = crate::api::sse::job_event_stream(job);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ── Handler: List benchmark history ───────────────────────────────────

pub async fn list_benchmark_history(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl IntoResponse {
    let repo_handle = match shared_repository(&web_state) {
        Ok(h) => h,
        Err(resp) => return resp,
    };

    let entries = match tokio::task::spawn_blocking(move || {
        let repo = repo_handle.lock().unwrap();
        repo.list_benchmarks()
    })
    .await
    {
        Ok(Ok(entries)) => entries,
        Ok(Err(e)) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    let history: Vec<BenchmarkHistoryEntry> = entries
        .into_iter()
        .map(|e| {
            let pp_sizes: Vec<u32> = serde_json::from_str(&e.pp_sizes).unwrap_or_default();
            let tg_sizes: Vec<u32> = serde_json::from_str(&e.tg_sizes).unwrap_or_default();

            // `results_json` may be:
            // - full BenchReport with "summaries" key (llama-bench)
            // - SpecBenchResult with "entries" key (spec decode)
            // - plain summaries array (legacy rows)
            let raw: serde_json::Value = serde_json::from_str(&e.results).unwrap_or_else(|err| {
                tracing::warn!("Failed to parse results for benchmark id={}: {}", e.id, err);
                serde_json::Value::Null
            });
            let summaries = summaries_from_results_json(&raw, &tg_sizes);
            let results_count = summaries.as_array().map(|a| a.len()).unwrap_or(0);
            BenchmarkHistoryEntry {
                id: e.id,
                created_at: e.created_at,
                model_id: e.model_id,
                display_name: e.display_name,
                quant: e.quant,
                backend: e.backend,
                engine: Some(e.engine),
                benchmark_type: e.benchmark_type,
                suite_id: e.suite_id,
                pp_sizes,
                tg_sizes,
                runs: e.runs,
                results_count,
                status: e.status,
                results: summaries,
            }
        })
        .collect();

    Json(history).into_response()
}

// ── Handler: Delete benchmark history entry ───────────────────────────

pub async fn delete_benchmark(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let repo_handle = match shared_repository(&web_state) {
        Ok(h) => h,
        Err(resp) => return resp,
    };

    match tokio::task::spawn_blocking(move || {
        let repo = repo_handle.lock().unwrap();
        repo.delete_benchmark(id)
    })
    .await
    {
        Ok(Ok(())) => Json(OkResponse::OK).into_response(),
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// Convert raw benchmark results JSON into summaries array.
///
/// Handles four shapes:
/// 1. BenchReport with `"summaries"` key → use as-is
/// 2. SpecBenchResult with `"entries"` + `"baseline_tg_ts"` → map to summary format
/// 3. MtpBenchResult with `"entries"` + `"aggregate"` → convert each entry to summary format
/// 4. Plain array → legacy rows, use as-is
/// 5. Anything else → empty array
fn summaries_from_results_json(raw: &serde_json::Value, tg_sizes: &[u32]) -> serde_json::Value {
    // 1. BenchReport with "summaries" key (llama-bench)
    if let Some(v) = raw.get("summaries") {
        if v.is_array() {
            return v.clone();
        }
    }

    // 2. SpecBenchResult: convert entries to llama-bench summary format
    // Maps: tg_ts_mean → tg_mean, tg_ts_stddev → tg_stddev,
    //       spec_type + draft_max → extra fields for display
    if raw.get("baseline_tg_ts").is_some() {
        if let Some(entries) = raw.get("entries") {
            if entries.is_array() {
                let mut summaries = serde_json::Value::Array(vec![]);
                for entry in entries.as_array().unwrap() {
                    let tg_mean = entry["tg_ts_mean"].as_f64().unwrap_or(0.0);
                    let stddev = entry["tg_ts_stddev"].as_f64().unwrap_or(0.0);
                    let status = entry["status"].as_str().unwrap_or("failed");
                    let delta_pct = entry["delta_pct"].as_f64().unwrap_or(0.0);
                    let spec_type = entry["spec_type"].as_str().unwrap_or("");
                    let draft_max = entry["draft_max"].as_u64().unwrap_or(0);
                    let ngram_n = entry["ngram_n"].as_u64();
                    let ngram_m = entry["ngram_m"].as_u64();

                    let mut summary = serde_json::Map::new();
                    // Frontend expects these fields for rendering.
                    summary.insert("prompt_tokens".to_string(), serde_json::json!(0u64));
                    summary.insert(
                        "gen_tokens".to_string(),
                        serde_json::json!(tg_sizes.first().copied().unwrap_or(0)),
                    );
                    summary.insert("tg_mean".to_string(), serde_json::json!(tg_mean));
                    summary.insert("tg_stddev".to_string(), serde_json::json!(stddev));
                    // Keep spec-specific fields for display.
                    summary.insert("spec_type".to_string(), serde_json::json!(spec_type));
                    summary.insert("draft_max".to_string(), serde_json::json!(draft_max));
                    if let Some(n) = ngram_n {
                        summary.insert("ngram_n".to_string(), serde_json::json!(n));
                    }
                    if let Some(m) = ngram_m {
                        summary.insert("ngram_m".to_string(), serde_json::json!(m));
                    }
                    if delta_pct != 0.0 {
                        summary.insert("delta_pct".to_string(), serde_json::json!(delta_pct));
                        summary.insert(
                            "delta_pct_display".to_string(),
                            serde_json::json!(format!("{:+.1}%", delta_pct)),
                        );
                    }
                    summary.insert("status".to_string(), serde_json::json!(status));
                    summaries
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::Value::Object(summary));
                }
                return summaries;
            }
        }
    }

    // 3. MtpBenchResult: convert entries to llama-bench summary format
    // Maps: predicted_per_second → tg_mean, no stddev (→0.0),
    //       error → status ("failed"/"success"), carry draft_max + accept_rate
    if raw.get("aggregate").is_some() {
        if let Some(entries) = raw.get("entries") {
            if entries.is_array() {
                let mut summaries = serde_json::Value::Array(vec![]);
                for entry in entries.as_array().unwrap() {
                    let tg_mean = entry["predicted_per_second"].as_f64().unwrap_or(0.0);
                    let status = match entry["error"].as_str() {
                        Some(_) => "failed",
                        None => "success",
                    };
                    let draft_max = entry["draft_max"].as_u64().unwrap_or(0);
                    let accept_rate = entry["accept_rate"].as_f64();

                    let mut summary = serde_json::Map::new();
                    // Frontend expects these fields for rendering.
                    summary.insert("prompt_tokens".to_string(), serde_json::json!(0u64));
                    summary.insert(
                        "gen_tokens".to_string(),
                        serde_json::json!(tg_sizes.first().copied().unwrap_or(0)),
                    );
                    summary.insert("tg_mean".to_string(), serde_json::json!(tg_mean));
                    summary.insert("tg_stddev".to_string(), serde_json::json!(0.0f64));
                    // MTP-specific display fields.
                    summary.insert("draft_max".to_string(), serde_json::json!(draft_max));
                    if let Some(rate) = accept_rate {
                        summary.insert("accept_rate".to_string(), serde_json::json!(rate));
                    }
                    if let Some(name) = entry["name"].as_str() {
                        summary.insert("name".to_string(), serde_json::json!(name));
                    }
                    summary.insert("status".to_string(), serde_json::json!(status));
                    summaries
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::Value::Object(summary));
                }
                return summaries;
            }
        }
    }

    // 4. Plain array → legacy rows, use as-is
    if raw.is_array() {
        return raw.clone();
    }

    // 5. Anything else → empty array
    serde_json::Value::Array(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::tests::assert_error_shape;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tama_core::config::Config;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    /// Test that a SpecBenchResult JSON is properly converted to summaries.
    ///
    /// SpecBenchResult serializes as `{baseline_tg_ts, baseline_tg_stddev, entries: [...]}`
    /// — it has NO `summaries` key. The conversion should recognize this shape
    /// (via `entries` array + `baseline_tg_ts` presence) and map the fields correctly.
    #[test]
    fn test_summaries_from_spec_bench_result() {
        let spec_json = serde_json::json!({
            "baseline_tg_ts": 50.0,
            "baseline_tg_stddev": 1.0,
            "entries": [
                {
                    "tg_ts_mean": 80.0,
                    "tg_ts_stddev": 2.0,
                    "spec_type": "ngram-simple",
                    "draft_max": 16,
                    "delta_pct": 60.0,
                    "status": "success"
                }
            ]
        });

        let tg_sizes = vec![2048];
        let summaries = summaries_from_results_json(&spec_json, &tg_sizes);

        assert!(
            summaries.is_array(),
            "summaries should be an array for SpecBenchResult"
        );
        let arr = summaries.as_array().unwrap();
        assert_eq!(arr.len(), 1, "should have exactly 1 summary entry");

        let entry = &arr[0];
        assert_eq!(
            entry["tg_mean"].as_f64(),
            Some(80.0),
            "tg_mean should be converted from tg_ts_mean"
        );
        assert_eq!(
            entry["tg_stddev"].as_f64(),
            Some(2.0),
            "tg_stddev should be converted from tg_ts_stddev"
        );
        assert_eq!(
            entry["spec_type"].as_str(),
            Some("ngram-simple"),
            "spec_type should be preserved"
        );
        assert_eq!(
            entry["draft_max"].as_u64(),
            Some(16),
            "draft_max should be preserved"
        );
        assert_eq!(
            entry["delta_pct"].as_f64(),
            Some(60.0),
            "delta_pct should be preserved"
        );
        assert_eq!(
            entry["status"].as_str(),
            Some("success"),
            "status should be preserved"
        );
        assert_eq!(
            entry["gen_tokens"].as_u64(),
            Some(2048),
            "gen_tokens should come from tg_sizes"
        );
    }

    /// Test that a BenchReport JSON with `"summaries"` key passes through unchanged.
    #[test]
    fn test_summaries_from_bench_report() {
        let bench_report = serde_json::json!({
            "model": "llama-7b",
            "summaries": [
                {
                    "prompt_tokens": 0,
                    "gen_tokens": 512,
                    "tg_mean": 45.3,
                    "tg_stddev": 0.5,
                    "status": "success"
                }
            ]
        });

        let tg_sizes = vec![512];
        let summaries = summaries_from_results_json(&bench_report, &tg_sizes);

        assert!(summaries.is_array(), "summaries should be an array");
        let arr = summaries.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["tg_mean"].as_f64(), Some(45.3));
    }

    /// Test that legacy plain-array rows pass through unchanged.
    #[test]
    fn test_summaries_from_legacy_array() {
        let legacy = serde_json::json!([
            {
                "prompt_tokens": 0,
                "gen_tokens": 256,
                "tg_mean": 30.0,
                "tg_stddev": 1.0
            }
        ]);

        let tg_sizes = vec![256];
        let summaries = summaries_from_results_json(&legacy, &tg_sizes);

        assert!(
            summaries.is_array(),
            "summaries should be an array for legacy rows"
        );
        let arr = summaries.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        // Legacy rows pass through unchanged — tg_mean from the original
        assert_eq!(arr[0]["tg_mean"].as_f64(), Some(30.0));
    }

    /// Test that unknown/invalid shapes produce an empty array.
    #[test]
    fn test_summaries_from_unknown_shape() {
        let unknown = serde_json::json!({"foo": "bar", "baz": 42});

        let tg_sizes = vec![1024];
        let summaries = summaries_from_results_json(&unknown, &tg_sizes);

        assert!(summaries.is_array(), "summaries should be an array");
        assert_eq!(
            summaries.as_array().unwrap().len(),
            0,
            "unknown shape should produce empty array"
        );
    }

    /// Test that SpecBenchResult without entries (e.g. failed run) produces empty array.
    #[test]
    fn test_summaries_from_spec_no_entries() {
        let spec_json = serde_json::json!({
            "baseline_tg_ts": 50.0,
            "baseline_tg_stddev": 1.0,
            "entries": []
        });

        let tg_sizes = vec![2048];
        let summaries = summaries_from_results_json(&spec_json, &tg_sizes);

        assert!(summaries.is_array(), "summaries should be an array");
        assert_eq!(
            summaries.as_array().unwrap().len(),
            0,
            "empty entries should produce empty array"
        );
    }

    /// Test that an MtpBenchResult JSON is properly converted to summaries.
    ///
    /// MtpBenchResult serializes as `{entries: [...], aggregate: {...}}`
    /// — it has NO `summaries` key, no `baseline_tg_ts`. The conversion should
    /// recognize this shape (via `entries` array + `aggregate` presence) and
    /// map each entry to the summary format, preserving MTP-specific fields.
    #[test]
    fn test_summaries_from_mtp_bench_result() {
        let mtp_json = serde_json::json!({
            "entries": [
                {
                    "draft_max": 0,
                    "name": "code_python",
                    "wall_s": 1.5,
                    "predicted_n": 100,
                    "draft_n": 0,
                    "draft_n_accepted": 0,
                    "accept_rate": null,
                    "predicted_per_second": 66.67,
                    "error": null
                },
                {
                    "draft_max": 4,
                    "name": "code_python",
                    "wall_s": 0.8,
                    "predicted_n": 100,
                    "draft_n": 50,
                    "draft_n_accepted": 30,
                    "accept_rate": 0.6,
                    "predicted_per_second": 125.0,
                    "error": null
                },
                {
                    "draft_max": 4,
                    "name": "explain_concept",
                    "wall_s": 2.0,
                    "predicted_n": 0,
                    "draft_n": 0,
                    "draft_n_accepted": 0,
                    "accept_rate": null,
                    "predicted_per_second": 0.0,
                    "error": "server crashed"
                }
            ],
            "aggregate": {
                "n_requests": 3,
                "total_predicted": 200,
                "total_draft": 50,
                "total_draft_accepted": 30,
                "aggregate_accept_rate": 0.6,
                "wall_s_total": 4.3
            }
        });

        let tg_sizes = vec![128];
        let summaries = summaries_from_results_json(&mtp_json, &tg_sizes);

        assert!(
            summaries.is_array(),
            "summaries should be an array for MtpBenchResult"
        );
        let arr = summaries.as_array().unwrap();
        assert_eq!(
            arr.len(),
            3,
            "should have exactly 3 summary entries (one per prompt)"
        );

        // Entry 0: baseline (draft_max=0)
        let entry0 = &arr[0];
        assert_eq!(
            entry0["tg_mean"].as_f64(),
            Some(66.67),
            "tg_mean from predicted_per_second"
        );
        assert_eq!(
            entry0["tg_stddev"].as_f64(),
            Some(0.0),
            "tg_stddev defaults to 0.0"
        );
        assert_eq!(entry0["draft_max"].as_u64(), Some(0), "draft_max preserved");
        assert_eq!(
            entry0["name"].as_str(),
            Some("code_python"),
            "name preserved"
        );
        assert!(
            entry0["accept_rate"].is_null(),
            "accept_rate should be null for baseline (was None)"
        );
        assert_eq!(
            entry0["status"].as_str(),
            Some("success"),
            "status should be success when error is null"
        );
        assert_eq!(
            entry0["gen_tokens"].as_u64(),
            Some(128),
            "gen_tokens from tg_sizes.first()"
        );

        // Entry 1: draft_max=4, successful
        let entry1 = &arr[1];
        assert_eq!(
            entry1["tg_mean"].as_f64(),
            Some(125.0),
            "tg_mean from predicted_per_second"
        );
        assert_eq!(entry1["draft_max"].as_u64(), Some(4), "draft_max preserved");
        assert_eq!(
            entry1["accept_rate"].as_f64(),
            Some(0.6),
            "accept_rate preserved as number"
        );
        assert_eq!(
            entry1["status"].as_str(),
            Some("success"),
            "status should be success when error is null"
        );

        // Entry 2: failed prompt
        let entry2 = &arr[2];
        assert_eq!(
            entry2["tg_mean"].as_f64(),
            Some(0.0),
            "tg_mean from predicted_per_second=0"
        );
        assert_eq!(
            entry2["status"].as_str(),
            Some("failed"),
            "status should be failed when error is non-null"
        );
    }

    /// Test SpecBenchResult with failed/skipped status values.
    #[test]
    fn test_summaries_from_spec_failed_status() {
        let spec_json = serde_json::json!({
            "baseline_tg_ts": 50.0,
            "baseline_tg_stddev": 1.0,
            "entries": [
                {
                    "tg_ts_mean": 0.0,
                    "tg_ts_stddev": 0.0,
                    "spec_type": "ngram-simple",
                    "draft_max": 16,
                    "delta_pct": -20.0,
                    "status": "skipped_oom"
                }
            ]
        });

        let tg_sizes = vec![2048];
        let summaries = summaries_from_results_json(&spec_json, &tg_sizes);

        let arr = summaries.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0]["status"].as_str(),
            Some("skipped_oom"),
            "status should be preserved as skipped_oom"
        );
    }

    /// GET /tama/v1/benchmarks/jobs/:id — a non-existent job should return
    /// 404 with the canonical error shape.
    #[tokio::test]
    async fn test_get_benchmark_result_not_found_error_shape() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(config, None, None));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            repository: None,
            db_pool: None,
        });

        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/benchmarks/jobs/nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "get_benchmark_result should return 404 for non-existent job"
        );

        let detail = assert_error_shape(resp).await;
        assert_eq!(
            detail.r#type,
            Some("NotFoundError".to_string()),
            "not-found job should return NotFoundError type"
        );
    }
}
