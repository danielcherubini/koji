//! Benchmark history database query functions.
//!
//! All functions take a `&PgPool` and are async (plan-190 Task 8).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

/// Row from the benchmarks table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRow {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub engine: String,
    pub pp_sizes: String,        // JSON array string
    pub tg_sizes: String,        // JSON array string
    pub threads: Option<String>, // JSON array string or null
    pub ngl_range: Option<String>,
    pub runs: u32,
    pub warmup: u32,
    pub results: String, // JSON array string
    pub load_time_ms: Option<f64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub duration_seconds: f64,
    pub status: String,
    /// Identifies what kind of benchmark was run (e.g., "baseline", "pp_sweep").
    /// NULL for legacy rows.
    pub benchmark_type: Option<String>,
    /// Suite identifier for grouping related benchmark runs. NULL for legacy rows.
    pub suite_id: Option<String>,
}

/// Parameters for inserting a benchmark result row.
#[derive(Debug, Clone)]
pub struct BenchmarkInsertParams<'a> {
    pub model_id: &'a str,
    pub display_name: Option<&'a str>,
    pub quant: Option<&'a str>,
    pub backend: &'a str,
    pub engine: &'a str,
    pub pp_sizes_json: &'a str,
    pub tg_sizes_json: &'a str,
    pub threads_json: Option<&'a str>,
    pub ngl_range: Option<&'a str>,
    pub runs: u32,
    pub warmup: u32,
    pub results_json: &'a str,
    pub load_time_ms: Option<f64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub duration_seconds: f64,
    pub status: &'a str,
    /// Identifies what kind of benchmark was run (e.g., "baseline", "pp_sweep").
    pub benchmark_type: Option<&'a str>,
    /// Suite identifier for grouping related benchmark runs.
    pub suite_id: Option<&'a str>,
}

/// Decode a `benchmarks` row into a [`BenchmarkRow`].
fn decode_benchmark(row: &sqlx::postgres::PgRow) -> anyhow::Result<BenchmarkRow> {
    let runs: i64 = row.get("runs");
    let warmup: i64 = row.get("warmup");
    Ok(BenchmarkRow {
        id: row.get("id"),
        created_at: row.get("created_at"),
        model_id: row.get("model_id"),
        display_name: row.get("display_name"),
        quant: row.get("quant"),
        backend: row.get("backend"),
        engine: row.get("engine"),
        pp_sizes: row.get("pp_sizes"),
        tg_sizes: row.get("tg_sizes"),
        threads: row.get("threads"),
        ngl_range: row.get("ngl_range"),
        runs: u32::try_from(runs).unwrap_or(0),
        warmup: u32::try_from(warmup).unwrap_or(0),
        results: row.get("results"),
        load_time_ms: row.get("load_time_ms"),
        vram_used_mib: row.get("vram_used_mib"),
        vram_total_mib: row.get("vram_total_mib"),
        duration_seconds: row.get("duration_seconds"),
        status: row.get("status"),
        benchmark_type: row.get("benchmark_type"),
        suite_id: row.get("suite_id"),
    })
}

/// Insert a benchmark result row. Returns the new row id.
pub async fn insert_benchmark(pool: &PgPool, params: &BenchmarkInsertParams<'_>) -> Result<i64> {
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let row = sqlx::query(
        "INSERT INTO benchmarks (
            created_at, model_id, display_name, quant, backend, engine,
            pp_sizes, tg_sizes, threads, ngl_range, runs, warmup,
            results, load_time_ms, vram_used_mib, vram_total_mib,
            duration_seconds, status, benchmark_type, suite_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
        RETURNING id",
    )
    .bind(created_at)
    .bind(params.model_id)
    .bind(params.display_name)
    .bind(params.quant)
    .bind(params.backend)
    .bind(params.engine)
    .bind(params.pp_sizes_json)
    .bind(params.tg_sizes_json)
    .bind(params.threads_json)
    .bind(params.ngl_range)
    .bind(i64::from(params.runs))
    .bind(i64::from(params.warmup))
    .bind(params.results_json)
    .bind(params.load_time_ms)
    .bind(params.vram_used_mib)
    .bind(params.vram_total_mib)
    .bind(params.duration_seconds)
    .bind(params.status)
    .bind(params.benchmark_type)
    .bind(params.suite_id)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

/// Fetch all benchmark entries ordered by created_at DESC.
///
/// Ties on `created_at` resolve by id ASC — the same effective order as the
/// former SQLite table scan — so append-only rows stay deterministic.
pub async fn list_benchmarks(pool: &PgPool) -> Result<Vec<BenchmarkRow>> {
    let rows = sqlx::query(
        "SELECT id, created_at, model_id, display_name, quant, backend, engine,
                pp_sizes, tg_sizes, threads, ngl_range, runs, warmup,
                results, load_time_ms, vram_used_mib, vram_total_mib,
                duration_seconds, status, benchmark_type, suite_id
         FROM benchmarks
         ORDER BY created_at DESC, id ASC",
    )
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_benchmark).collect()
}

/// Delete a benchmark entry by id.
pub async fn delete_benchmark(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM benchmarks WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::postgres::with_schema;

    /// Helper to create test benchmark parameters.
    fn make_benchmark<'a>(
        model_id: &'a str,
        backend: &'a str,
        pp_sizes: &'a str,
        tg_sizes: &'a str,
        results: &'a str,
    ) -> BenchmarkInsertParams<'a> {
        BenchmarkInsertParams {
            model_id,
            display_name: Some("Test Model"),
            quant: Some("Q4_K_M"),
            backend,
            engine: "llama_bench",
            pp_sizes_json: pp_sizes,
            tg_sizes_json: tg_sizes,
            threads_json: Some("[4,8]"),
            ngl_range: None,
            runs: 3,
            warmup: 1,
            results_json: results,
            load_time_ms: Some(1500.0),
            vram_used_mib: Some(4096),
            vram_total_mib: Some(8192),
            duration_seconds: 30.5,
            status: "success",
            benchmark_type: Some("baseline"),
            suite_id: None,
        }
    }

    /// Minimal-params builder for the null-round-trip tests.
    fn null_benchmark<'a>(model_id: &'a str) -> BenchmarkInsertParams<'a> {
        BenchmarkInsertParams {
            model_id,
            display_name: None,
            quant: None,
            backend: "llama_cpp",
            engine: "llama_bench",
            pp_sizes_json: "[512]",
            tg_sizes_json: "[128]",
            threads_json: None,
            ngl_range: None,
            runs: 3,
            warmup: 1,
            results_json: "[]",
            load_time_ms: None,
            vram_used_mib: None,
            vram_total_mib: None,
            duration_seconds: 0.0,
            status: "success",
            benchmark_type: None,
            suite_id: None,
        }
    }

    #[tokio::test]
    async fn test_insert_benchmark_returns_id() {
        let guard = with_schema().await;
        let params = make_benchmark(
            "qwen7b",
            "llama_cpp",
            "[512,1024]",
            "[128,256]",
            "[{\"pp\":100}]",
        );

        let id = insert_benchmark(&guard.pool, &params).await.unwrap();

        assert_eq!(id, 1);
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_list_benchmarks_empty() {
        let guard = with_schema().await;
        let benchmarks = list_benchmarks(&guard.pool).await.unwrap();
        assert!(benchmarks.is_empty());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_list_benchmarks_returns_inserted() {
        let guard = with_schema().await;
        let params = make_benchmark("qwen7b", "llama_cpp", "[512,1024]", "[128,256]", "[{}]");

        insert_benchmark(&guard.pool, &params).await.unwrap();

        let benchmarks = list_benchmarks(&guard.pool).await.unwrap();
        assert_eq!(benchmarks.len(), 1);
        assert_eq!(benchmarks[0].model_id, "qwen7b");
        assert_eq!(benchmarks[0].backend, "llama_cpp");
        assert_eq!(benchmarks[0].display_name, Some("Test Model".to_string()));
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_delete_benchmark() {
        let guard = with_schema().await;
        let params = make_benchmark("qwen7b", "llama_cpp", "[512]", "[128]", "[{}]");

        let id = insert_benchmark(&guard.pool, &params).await.unwrap();

        delete_benchmark(&guard.pool, id).await.unwrap();

        let benchmarks = list_benchmarks(&guard.pool).await.unwrap();
        assert!(benchmarks.is_empty());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_list_benchmarks_ordered_desc() {
        let guard = with_schema().await;
        // Insert multiple benchmarks with explicit timestamps to control order.
        for (created_at, model) in [(1000i64, "model_a"), (3000, "model_c"), (2000, "model_b")] {
            sqlx::query(
                "INSERT INTO benchmarks (created_at, model_id, backend, pp_sizes, tg_sizes, results, duration_seconds, status)
                 VALUES ($1, $2, 'llama_cpp', '[512]', '[128]', '[{}]', 10.0, 'success')",
            )
            .bind(created_at)
            .bind(model)
            .execute(&guard.pool)
            .await
            .unwrap();
        }

        let benchmarks = list_benchmarks(&guard.pool).await.unwrap();
        assert_eq!(benchmarks.len(), 3);
        assert_eq!(benchmarks[0].model_id, "model_c"); // created_at=3000
        assert_eq!(benchmarks[1].model_id, "model_b"); // created_at=2000
        assert_eq!(benchmarks[2].model_id, "model_a"); // created_at=1000
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_insert_benchmark_with_nulls() {
        let guard = with_schema().await;

        let params = null_benchmark("qwen7b");
        let id = insert_benchmark(&guard.pool, &params).await.unwrap();
        assert_eq!(id, 1);

        let benchmarks = list_benchmarks(&guard.pool).await.unwrap();
        assert_eq!(benchmarks.len(), 1);
        assert!(benchmarks[0].display_name.is_none());
        assert!(benchmarks[0].quant.is_none());
        assert!(benchmarks[0].benchmark_type.is_none());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_insert_and_list_benchmarks_round_trip() {
        let guard = with_schema().await;
        let params = BenchmarkInsertParams {
            model_id: "test-model",
            display_name: Some("Test Model"),
            quant: Some("Q4_K_M"),
            backend: "llama_cpp",
            engine: "llama_bench",
            pp_sizes_json: "[512,1024]",
            tg_sizes_json: "[128,256]",
            threads_json: Some("[8,16]"),
            ngl_range: Some("0-99+1"),
            runs: 3,
            warmup: 1,
            results_json: r#"[{"test_name":"tg128","pp_mean":120.5,"tg_mean":45.2}]"#,
            load_time_ms: Some(1500.0),
            vram_used_mib: Some(6144),
            vram_total_mib: Some(8192),
            duration_seconds: 45.5,
            status: "success",
            benchmark_type: Some("baseline"),
            suite_id: None,
        };
        let id = insert_benchmark(&guard.pool, &params).await.unwrap();

        assert_eq!(id, 1);

        let entries = list_benchmarks(&guard.pool).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model_id, "test-model");
        assert_eq!(entries[0].display_name, Some("Test Model".to_string()));
        assert_eq!(entries[0].quant, Some("Q4_K_M".to_string()));
        assert_eq!(entries[0].ngl_range, Some("0-99+1".to_string()));
        assert_eq!(entries[0].threads, Some("[8,16]".to_string()));
        assert_eq!(entries[0].runs, 3);
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_insert_benchmark_returns_incrementing_ids() {
        let guard = with_schema().await;
        let params_a = null_benchmark("a");
        let params_b = null_benchmark("b");
        let id1 = insert_benchmark(&guard.pool, &params_a).await.unwrap();
        let id2 = insert_benchmark(&guard.pool, &params_b).await.unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_suite_id_round_trip_some() {
        let guard = with_schema().await;
        let params = BenchmarkInsertParams {
            model_id: "suite-model",
            display_name: Some("Suite Model"),
            ..null_benchmark("suite-model")
        };
        let params = BenchmarkInsertParams {
            benchmark_type: Some("baseline"),
            suite_id: Some("suite-abc"),
            ..params
        };

        let id = insert_benchmark(&guard.pool, &params).await.unwrap();
        assert_eq!(id, 1);

        let entries = list_benchmarks(&guard.pool).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].suite_id, Some("suite-abc".to_string()));
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_suite_id_round_trip_none() {
        let guard = with_schema().await;
        let params = null_benchmark("no-suite-model");

        let id = insert_benchmark(&guard.pool, &params).await.unwrap();
        assert_eq!(id, 1);

        let entries = list_benchmarks(&guard.pool).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].suite_id.is_none());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_suite_id_mixed_in_list() {
        let guard = with_schema().await;

        // Insert with suite_id
        let params_with_suite = BenchmarkInsertParams {
            suite_id: Some("suite-1"),
            ..null_benchmark("model-a")
        };

        // Insert without suite_id
        let params_no_suite = null_benchmark("model-b");

        insert_benchmark(&guard.pool, &params_with_suite)
            .await
            .unwrap();
        insert_benchmark(&guard.pool, &params_no_suite)
            .await
            .unwrap();

        let entries = list_benchmarks(&guard.pool).await.unwrap();
        assert_eq!(entries.len(), 2);

        // Both have the same created_at (SystemTime::now()), so the id ASC
        // tie-break applies: model-a was inserted first → lower id → first.
        assert_eq!(entries[0].model_id, "model-a");
        assert_eq!(entries[0].suite_id, Some("suite-1".to_string()));

        assert_eq!(entries[1].model_id, "model-b");
        assert!(entries[1].suite_id.is_none());
        guard.finish().await;
    }
}
