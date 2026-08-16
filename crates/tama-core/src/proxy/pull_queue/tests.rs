use std::sync::Arc;
use std::time::Instant;

use super::{PullEvent, PullQueueService};
use crate::config::Config;
use crate::proxy::ProxyState;
use crate::testing::postgres::with_schema;
use sqlx::Row;

/// Build a `PullQueueService` on a fresh isolated schema.
///
/// Returns the service plus the pool and guard (call `guard.finish().await`
/// at the end of the test).
async fn setup_service() -> (
    PullQueueService,
    sqlx::PgPool,
    crate::testing::postgres::SchemaGuard,
) {
    let guard = with_schema().await;
    let pool = guard.pool.clone();

    let svc = PullQueueService::new(std::sync::Arc::new(pool.clone()), 2);
    (svc, pool, guard)
}

#[tokio::test]
async fn test_enqueue_and_dequeue() {
    let (svc, pool, guard) = setup_service().await;

    svc.enqueue(
        "job-1",
        "unsloth/Qwen3.6-35B-A3B-GGUF",
        "Qwen3.6-35B-Q4_K_M.gguf",
        Some("Qwen3.6 35B"),
        "model",
        Some("Q4_K_M"),
        Some(4096),
    )
    .await
    .unwrap();

    let item = svc.dequeue().await.unwrap().unwrap();
    assert_eq!(item.job_id, "job-1");
    assert_eq!(item.repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
    assert_eq!(item.filename, "Qwen3.6-35B-Q4_K_M.gguf");
    assert_eq!(item.display_name, Some("Qwen3.6 35B".to_string()));
    assert_eq!(item.status, "queued");
    assert_eq!(item.kind, "model");
    let _ = pool;

    guard.finish().await;
}

#[tokio::test]
async fn test_update_status_emits_event() {
    let (svc, _pool, guard) = setup_service().await;

    svc.enqueue(
        "job-1",
        "unsloth/Qwen3.6-35B-A3B-GGUF",
        "Qwen3.6-35B-Q4_K_M.gguf",
        Some("Qwen3.6 35B"),
        "model",
        Some("Q4_K_M"),
        Some(4096),
    )
    .await
    .unwrap();

    let mut rx = svc.subscribe_events();

    svc.update_status("job-1", "running", 0, Some(2000), None, None)
        .await
        .unwrap();

    let event = rx.try_recv().unwrap();
    match event {
        PullEvent::Started {
            job_id,
            repo_id,
            filename,
            total_bytes,
        } => {
            assert_eq!(job_id, "job-1");
            assert_eq!(repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
            assert_eq!(filename, "Qwen3.6-35B-Q4_K_M.gguf");
            assert_eq!(total_bytes, Some(2000));
        }
        other => panic!("Expected Started event, got {:?}", other),
    }

    guard.finish().await;
}

#[tokio::test]
async fn test_cancel_emits_event() {
    let (svc, _pool, guard) = setup_service().await;

    svc.enqueue(
        "job-1",
        "unsloth/Qwen3.6-35B-A3B-GGUF",
        "Qwen3.6-35B-Q4_K_M.gguf",
        Some("Qwen3.6 35B"),
        "model",
        Some("Q4_K_M"),
        Some(4096),
    )
    .await
    .unwrap();

    let mut rx = svc.subscribe_events();

    svc.cancel("job-1").await.unwrap();

    let event = rx.try_recv().unwrap();
    match event {
        PullEvent::Cancelled { job_id, filename } => {
            assert_eq!(job_id, "job-1");
            assert_eq!(filename, "Qwen3.6-35B-Q4_K_M.gguf");
        }
        other => panic!("Expected Cancelled event, got {:?}", other),
    }

    guard.finish().await;
}

#[tokio::test]
async fn test_dequeue_empty_queue_returns_none() {
    let (svc, _pool, guard) = setup_service().await;

    let result = svc.dequeue().await.unwrap();
    assert!(result.is_none());

    guard.finish().await;
}

/// Integration test: verify that enqueue_pull creates a pull_queue row
/// with the correct fields including quant and context_length.
#[tokio::test]
async fn test_enqueue_pull_creates_queue_row() {
    let (svc, _pool, guard) = setup_service().await;

    // Subscribe before enqueue so we can receive the event
    let mut rx = svc.subscribe_events();

    svc.enqueue(
        "pull-test-001",
        "unsloth/Qwen3.6-35B-A3B-GGUF",
        "Qwen3.6-35B-Q4_K_M.gguf",
        Some("Qwen3.6 35B"),
        "model",
        Some("Q4_K_M"),
        Some(4096),
    )
    .await
    .unwrap();

    // Verify the row exists in the DB via the service
    let item = svc
        .get_queue_item("pull-test-001")
        .await
        .unwrap()
        .expect("row should exist");

    assert_eq!(item.job_id, "pull-test-001");
    assert_eq!(item.status, "queued");
    assert_eq!(item.quant, Some("Q4_K_M".to_string()));
    assert_eq!(item.context_length, Some(4096));

    // Verify the Queued event was emitted
    let event = rx.try_recv().unwrap();
    match event {
        PullEvent::Queued {
            job_id,
            repo_id,
            filename,
        } => {
            assert_eq!(job_id, "pull-test-001");
            assert_eq!(repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
            assert_eq!(filename, "Qwen3.6-35B-Q4_K_M.gguf");
        }
        other => panic!("Expected Queued event, got {:?}", other),
    }

    guard.finish().await;
}

/// Integration test: verify full lifecycle status transitions through the DB.
#[tokio::test]
async fn test_status_transitions_through_lifecycle() {
    let (svc, _pool, guard) = setup_service().await;

    // Subscribe before enqueue so we can receive events
    let mut rx = svc.subscribe_events();

    // Step 1: Enqueue
    svc.enqueue(
        "pull-test-002",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        Some(2048),
    )
    .await
    .unwrap();

    let item = svc
        .get_queue_item("pull-test-002")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "queued");

    // Step 2: Transition to running
    svc.update_status("pull-test-002", "running", 0, None, None, None)
        .await
        .unwrap();

    let item = svc
        .get_queue_item("pull-test-002")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "running");
    assert!(item.started_at.is_some());

    // Step 3: Transition to verifying
    svc.update_status("pull-test-002", "verifying", 1000, Some(2000), None, None)
        .await
        .unwrap();

    let item = svc
        .get_queue_item("pull-test-002")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "verifying");

    // Step 4: Transition to completed with duration
    let start = Instant::now();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    svc.update_status(
        "pull-test-002",
        "completed",
        2000,
        Some(2000),
        None,
        Some(duration_ms),
    )
    .await
    .unwrap();

    let item = svc
        .get_queue_item("pull-test-002")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "completed");
    assert!(item.completed_at.is_some());

    // Drain any intermediate events and find the Completed event
    let mut completed_event = None;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, PullEvent::Completed { .. }) {
            completed_event = Some(event);
        }
    }
    let event = completed_event.expect("Expected Completed event");
    match event {
        PullEvent::Completed {
            job_id,
            filename,
            size_bytes,
            duration_ms: event_duration,
        } => {
            assert_eq!(job_id, "pull-test-002");
            assert_eq!(filename, "model.gguf");
            assert_eq!(size_bytes, 2000);
            assert!(
                event_duration >= duration_ms,
                "event duration {} should be >= computed {}",
                event_duration,
                duration_ms
            );
        }
        other => panic!("Expected Completed event, got {:?}", other),
    }

    guard.finish().await;
}

/// Integration test: verify duration_ms is computed via Instant::elapsed()
/// and not derived from timestamp subtraction.
#[tokio::test]
async fn test_duration_ms_computed_via_instant() {
    let (svc, _pool, guard) = setup_service().await;

    // Subscribe before enqueue so we can receive events
    let mut rx = svc.subscribe_events();

    // Enqueue the item
    svc.enqueue(
        "pull-test-003",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Transition through the lifecycle with known delays
    svc.update_status("pull-test-003", "running", 0, None, None, None)
        .await
        .unwrap();

    // Sleep for a known duration, then compute duration via Instant::elapsed()
    let start = Instant::now();
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    let computed_duration = start.elapsed().as_millis() as u64;

    svc.update_status(
        "pull-test-003",
        "completed",
        5000,
        Some(5000),
        None,
        Some(computed_duration),
    )
    .await
    .unwrap();

    // Drain any intermediate events and find the Completed event
    let mut completed_event = None;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, PullEvent::Completed { .. }) {
            completed_event = Some(event);
        }
    }
    let event = completed_event.expect("Expected Completed event");
    match event {
        PullEvent::Completed { duration_ms, .. } => {
            assert!(
                duration_ms >= computed_duration,
                "duration_ms ({}) should be >= computed ({})",
                duration_ms,
                computed_duration
            );
        }
        other => panic!("Expected Completed event, got {:?}", other),
    }

    // Verify the DB row has completed_at set (timestamp-based), but
    // duration_ms was computed in Rust via Instant::elapsed()
    let item = svc
        .get_queue_item("pull-test-003")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "completed");
    assert!(item.completed_at.is_some());

    guard.finish().await;
}

// ── CAS (try_mark_running) tests ──────────────────────────────────────

/// Test that try_mark_running returns true when the item is queued.
#[tokio::test]
async fn test_try_mark_running_succeeds_for_queued() {
    let (svc, _pool, guard) = setup_service().await;

    svc.enqueue(
        "cas-job-1",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    let result = svc.try_mark_running("cas-job-1").await.unwrap();
    assert!(result, "CAS should succeed for a queued item");

    // Verify the status changed to running
    let item = svc
        .get_queue_item("cas-job-1")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "running");

    guard.finish().await;
}

/// Test that try_mark_running returns false when the item is not queued.
#[tokio::test]
async fn test_try_mark_running_fails_for_non_queued() {
    let (svc, pool, guard) = setup_service().await;

    svc.enqueue(
        "cas-job-2",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Manually set status to running (simulating another consumer claiming it)
    sqlx::query(
        "UPDATE pull_queue SET status = 'running', started_at = now() \
         WHERE job_id = 'cas-job-2'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let result = svc.try_mark_running("cas-job-2").await.unwrap();
    assert!(
        !result,
        "CAS should fail when item is already in running state"
    );

    // Verify the status is still running
    let item = svc
        .get_queue_item("cas-job-2")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "running");

    guard.finish().await;
}

/// Test that try_mark_running returns false for a non-existent job.
#[tokio::test]
async fn test_try_mark_running_nonexistent_job() {
    let (svc, _pool, guard) = setup_service().await;

    let result = svc.try_mark_running("nonexistent-job").await.unwrap();
    assert!(!result, "CAS should return false for a non-existent job");

    guard.finish().await;
}

// ── on_startup_recovery tests ─────────────────────────────────────────

/// Test that on_startup_recovery marks stale running items as queued.
/// A running item with started_at > 1 hour ago is considered stale.
#[tokio::test]
async fn test_on_startup_recovery_stale_items() {
    let (svc, pool, guard) = setup_service().await;

    // Enqueue an item
    svc.enqueue(
        "recovery-job-1",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Manually set it to running with an old started_at (> 1 hour ago)
    sqlx::query(
        "UPDATE pull_queue SET status = 'running', started_at = now() - interval '2 hours' \
         WHERE job_id = 'recovery-job-1'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Verify it's running before recovery
    let item = svc
        .get_queue_item("recovery-job-1")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "running");

    // Run startup recovery
    svc.on_startup_recovery().await.unwrap();

    // Verify it's now queued
    let item = svc
        .get_queue_item("recovery-job-1")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "queued");
    assert!(item.started_at.is_none());
    assert!(item.completed_at.is_none());

    guard.finish().await;
}

/// Test that on_startup_recovery does NOT affect non-stale running items.
#[tokio::test]
async fn test_on_startup_recovery_non_stale_items() {
    let (svc, pool, guard) = setup_service().await;

    // Enqueue an item
    svc.enqueue(
        "recovery-job-2",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Set it to running with a recent started_at (< 1 hour ago)
    sqlx::query(
        "UPDATE pull_queue SET status = 'running', started_at = now() \
         WHERE job_id = 'recovery-job-2'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Run startup recovery
    svc.on_startup_recovery().await.unwrap();

    // Verify it's still running (not stale)
    let item = svc
        .get_queue_item("recovery-job-2")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "running");

    guard.finish().await;
}

/// Test that on_startup_recovery does NOT affect completed items.
#[tokio::test]
async fn test_on_startup_recovery_completed_items() {
    let (svc, _pool, guard) = setup_service().await;

    svc.enqueue(
        "recovery-job-3",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Set it to completed
    svc.update_status("recovery-job-3", "completed", 5000, Some(5000), None, None)
        .await
        .unwrap();

    // Run startup recovery
    svc.on_startup_recovery().await.unwrap();

    // Verify it's still completed
    let item = svc
        .get_queue_item("recovery-job-3")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "completed");

    guard.finish().await;
}

/// Test that on_startup_recovery handles items with NULL started_at.
#[tokio::test]
async fn test_on_startup_recovery_null_started_at() {
    let (svc, pool, guard) = setup_service().await;

    svc.enqueue(
        "recovery-job-4",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Set it to running with NULL started_at
    sqlx::query(
        "UPDATE pull_queue SET status = 'running', started_at = NULL \
         WHERE job_id = 'recovery-job-4'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Run startup recovery
    svc.on_startup_recovery().await.unwrap();

    // NULL started_at matches the `started_at IS NULL` branch of the recovery
    // condition, so the item is re-queued.
    let item = svc
        .get_queue_item("recovery-job-4")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "queued");

    guard.finish().await;
}

// ── Concurrent access tests ───────────────────────────────────────────

/// Test that concurrent enqueue and dequeue operations work correctly.
#[tokio::test]
async fn test_concurrent_enqueue_dequeue() {
    let (svc, pool, guard) = setup_service().await;
    let svc = Arc::new(svc);
    let num_items = 10;

    // Enqueue items concurrently
    let mut handles = vec![];
    for i in 0..num_items {
        let svc_clone = Arc::clone(&svc);
        handles.push(tokio::spawn(async move {
            svc_clone
                .enqueue(
                    &format!("concurrent-job-{}", i),
                    "test/repo",
                    "model.gguf",
                    None,
                    "model",
                    Some("Q4_K_M"),
                    None,
                )
                .await
                .unwrap();
        }));
    }

    // Wait for all enqueues to complete
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all items are in the queue
    let active = svc.get_active_items().await.unwrap();
    assert_eq!(active.len(), num_items);

    // Dequeue and claim each item (dequeue reads, try_mark_running claims)
    for i in 0..num_items {
        let item = svc.dequeue().await.unwrap();
        assert!(
            item.is_some(),
            "Item concurrent-job-{} should be dequeued",
            i
        );
        let job_id = item.unwrap().job_id;
        // Verify it's one of the expected jobs
        assert!(
            job_id.starts_with("concurrent-job-"),
            "Expected concurrent-job-*, got {}",
            job_id
        );
        // Claim the item (changes status to running)
        let claimed = svc.try_mark_running(&job_id).await.unwrap();
        assert!(claimed, "Item {} should be claimable", job_id);
    }

    // Verify all items are now running (not queued)
    let queued_count: i64 =
        sqlx::query("SELECT COUNT(*) AS n FROM pull_queue WHERE status = 'queued'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("n");
    assert_eq!(queued_count, 0, "All items should be claimed (not queued)");

    guard.finish().await;
}

/// Test that concurrent status transitions don't cause data corruption.
#[tokio::test]
async fn test_concurrent_status_transitions() {
    let (svc, _pool, guard) = setup_service().await;
    let svc = Arc::new(svc);

    // Enqueue an item
    svc.enqueue(
        "concurrent-status-job",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Perform multiple status transitions concurrently
    let svc_clone = Arc::clone(&svc);
    let h1 = tokio::spawn(async move {
        svc_clone
            .update_status("concurrent-status-job", "running", 0, None, None, None)
            .await
            .unwrap();
    });

    let svc_clone = Arc::clone(&svc);
    let h2 = tokio::spawn(async move {
        // Small delay to increase contention
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        svc_clone
            .update_status(
                "concurrent-status-job",
                "verifying",
                1000,
                Some(2000),
                None,
                None,
            )
            .await
            .unwrap();
    });

    let svc_clone = Arc::clone(&svc);
    let h3 = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        svc_clone
            .update_status(
                "concurrent-status-job",
                "completed",
                2000,
                Some(2000),
                None,
                None,
            )
            .await
            .unwrap();
    });

    h1.await.unwrap();
    h2.await.unwrap();
    h3.await.unwrap();

    // Verify the final status is completed
    let item = svc
        .get_queue_item("concurrent-status-job")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "completed");
    assert_eq!(item.bytes_pulled, 2000);

    guard.finish().await;
}

/// Test that cancel fails for items already in terminal state.
#[tokio::test]
async fn test_cancel_terminal_state_fails() {
    let (svc, _pool, guard) = setup_service().await;

    svc.enqueue(
        "cancel-test-job",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Transition to completed
    svc.update_status("cancel-test-job", "completed", 5000, Some(5000), None, None)
        .await
        .unwrap();

    // Cancel should fail for terminal state
    let result = svc.cancel("cancel-test-job").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("terminal state"));

    guard.finish().await;
}

/// Test that cancel fails for a non-existent job.
#[tokio::test]
async fn test_cancel_nonexistent_job() {
    let (svc, _pool, guard) = setup_service().await;

    let result = svc.cancel("nonexistent-cancel-job").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));

    guard.finish().await;
}

// ── queue_processor_loop integration tests ────────────────────────────

/// Integration test: the queue_processor_loop's startup recovery
/// calls svc.on_startup_recovery() which marks stale running items as queued.
/// We verify this by creating a PullQueueService directly, inserting
/// a stale item, then invoking recovery.
#[tokio::test]
async fn test_queue_processor_loop_stale_recovery() {
    let (svc, pool, guard) = setup_service().await;
    let _config = Config::default();

    // Insert a stale running item (started > 1 hour ago)
    sqlx::query(
        "INSERT INTO pull_queue (job_id, repo_id, filename, status, started_at, kind) \
         VALUES ('loop-recovery-job', 'test/repo', 'model.gguf', 'running', now() - interval '2 hours', 'model')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Verify it's running before recovery
    let status: String =
        sqlx::query("SELECT status FROM pull_queue WHERE job_id = 'loop-recovery-job'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("status");
    assert_eq!(status, "running");

    // Directly invoke startup recovery (what the loop does on first iteration)
    svc.on_startup_recovery().await.unwrap();

    // Verify the item was recovered
    let row =
        sqlx::query("SELECT status, started_at FROM pull_queue WHERE job_id = 'loop-recovery-job'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let item_status: String = row.get("status");
    let started_at: Option<sqlx::types::time::OffsetDateTime> = row.get("started_at");

    assert_eq!(
        item_status, "queued",
        "Stale item should be recovered to queued"
    );
    assert!(
        started_at.is_none(),
        "started_at should be cleared after recovery"
    );

    guard.finish().await;
}

/// Integration test: the queue_processor_loop dequeues queued items
/// and marks them as running via try_mark_running + update_status.
#[tokio::test]
async fn test_queue_processor_loop_dequeues_items() {
    let (svc, pool, guard) = setup_service().await;
    let _config = Config::default();

    // Insert multiple queued items
    for i in 0..3 {
        sqlx::query(
            "INSERT INTO pull_queue (job_id, repo_id, filename, status, kind) \
             VALUES ($1, 'test/repo', 'model.gguf', 'queued', 'model')",
        )
        .bind(format!("loop-dequeue-job-{}", i))
        .execute(&pool)
        .await
        .unwrap();
    }

    // Simulate what the loop does: dequeue + try_mark_running + update_status
    for _ in 0..3 {
        let item = svc.dequeue().await.unwrap();
        if let Some(item) = item {
            let claimed = svc.try_mark_running(&item.job_id).await.unwrap();
            if claimed {
                let _ = svc
                    .update_status(&item.job_id, "running", 0, None, None, None)
                    .await;
            }
        }
    }

    // Verify all items are now running
    let count: i64 = sqlx::query("SELECT COUNT(*) AS n FROM pull_queue WHERE status = 'running'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");

    assert_eq!(
        count, 3,
        "All 3 items should have been dequeued and marked as running"
    );

    guard.finish().await;
}

/// Integration test: the queue_processor_loop detects dead tasks
/// (running items not registered in pull_jobs with started_at > 10s)
/// and re-queues them so they can be retried.
#[tokio::test]
async fn test_queue_processor_loop_dead_task_detection() {
    let guard = with_schema().await;
    let pool = guard.pool.clone();

    let config = Config::default();

    let state = Arc::new(ProxyState::new(
        config,
        None,
        Some(std::sync::Arc::new(pool.clone())),
    ));

    // The service (constructed by ProxyState::new from the pool) is the same
    // one the loop uses; reach it via the state.
    let svc = state
        .pull_queue()
        .as_ref()
        .expect("pull_queue configured")
        .clone();

    // Insert a running item whose pull task died (started 15 seconds ago)
    sqlx::query(
        "INSERT INTO pull_queue (job_id, repo_id, filename, status, started_at, kind) \
         VALUES ('loop-dead-job', 'test/repo', 'model.gguf', 'running', now() - interval '15 seconds', 'model')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Verify the job is NOT in pull_jobs (simulating a crashed task)
    let jobs = state.pull.pull_jobs.read().await;
    assert!(
        !jobs.contains_key("loop-dead-job"),
        "Job should not be in pull_jobs"
    );
    drop(jobs);

    // Simulate what the loop does for dead task recovery:
    // detect dead task (running > 10s, not in pull_jobs) and re-queue
    super::recovery::requeue_dead_task(svc.pool.as_ref(), "loop-dead-job")
        .await
        .unwrap();

    // Verify the task was re-queued
    let status: String =
        sqlx::query("SELECT status FROM pull_queue WHERE job_id = 'loop-dead-job'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("status");

    assert_eq!(status, "queued", "Dead task should be re-queued for retry");

    guard.finish().await;
}

/// Test that PullQueueService emits events for all status transitions.
#[tokio::test]
async fn test_all_status_transitions_emit_events() {
    let (svc, _pool, guard) = setup_service().await;

    // Subscribe BEFORE enqueue so we capture the Queued event
    let mut rx = svc.subscribe_events();

    svc.enqueue(
        "events-job",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Verify Queued event was emitted
    let event = rx.try_recv().unwrap();
    match event {
        PullEvent::Queued { job_id, .. } => assert_eq!(job_id, "events-job"),
        other => panic!("Expected Queued event, got {:?}", other),
    }

    // Transition to running
    svc.update_status("events-job", "running", 0, None, None, None)
        .await
        .unwrap();
    let event = rx.try_recv().unwrap();
    match event {
        PullEvent::Started { job_id, .. } => assert_eq!(job_id, "events-job"),
        other => panic!("Expected Started event, got {:?}", other),
    }

    // Transition to verifying
    svc.update_status("events-job", "verifying", 1000, Some(2000), None, None)
        .await
        .unwrap();
    let event = rx.try_recv().unwrap();
    match event {
        PullEvent::Verifying { job_id, .. } => assert_eq!(job_id, "events-job"),
        other => panic!("Expected Verifying event, got {:?}", other),
    }

    // Transition to completed
    svc.update_status("events-job", "completed", 2000, Some(2000), None, None)
        .await
        .unwrap();
    let event = rx.try_recv().unwrap();
    match event {
        PullEvent::Completed { job_id, .. } => assert_eq!(job_id, "events-job"),
        other => panic!("Expected Completed event, got {:?}", other),
    }

    guard.finish().await;
}

/// Test that progress updates emit Progress events without changing status.
#[tokio::test]
async fn test_progress_updates_emit_events() {
    let (svc, _pool, guard) = setup_service().await;

    svc.enqueue(
        "progress-job",
        "test/repo",
        "model.gguf",
        None,
        "model",
        Some("Q4_K_M"),
        None,
    )
    .await
    .unwrap();

    // Transition to running first
    svc.update_status("progress-job", "running", 0, Some(5000), None, None)
        .await
        .unwrap();

    let mut rx = svc.subscribe_events();

    // Update progress
    svc.update_progress("progress-job", 2500, Some(5000))
        .await
        .unwrap();

    let event = rx.try_recv().unwrap();
    match event {
        PullEvent::Progress {
            job_id,
            bytes_pulled,
            total_bytes,
        } => {
            assert_eq!(job_id, "progress-job");
            assert_eq!(bytes_pulled, 2500);
            assert_eq!(total_bytes, Some(5000));
        }
        other => panic!("Expected Progress event, got {:?}", other),
    }

    // Verify status is still running
    let item = svc
        .get_queue_item("progress-job")
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(item.status, "running");

    guard.finish().await;
}

/// Test that get_active_items returns only queued, running, and verifying items.
#[tokio::test]
async fn test_get_active_items_excludes_terminal() {
    let (svc, _pool, guard) = setup_service().await;

    // Enqueue multiple items
    svc.enqueue(
        "active-1",
        "test/repo",
        "model1.gguf",
        None,
        "model",
        None,
        None,
    )
    .await
    .unwrap();
    svc.enqueue(
        "active-2",
        "test/repo",
        "model2.gguf",
        None,
        "model",
        None,
        None,
    )
    .await
    .unwrap();
    svc.enqueue(
        "active-3",
        "test/repo",
        "model3.gguf",
        None,
        "model",
        None,
        None,
    )
    .await
    .unwrap();

    // Transition active-1 to running, active-2 to completed
    svc.update_status("active-1", "running", 0, None, None, None)
        .await
        .unwrap();
    svc.update_status("active-2", "completed", 5000, Some(5000), None, None)
        .await
        .unwrap();

    // active-3 is still queued

    let active = svc.get_active_items().await.unwrap();
    let job_ids: Vec<&str> = active.iter().map(|i| i.job_id.as_str()).collect();

    assert!(
        job_ids.contains(&"active-1"),
        "running item should be active"
    );
    assert!(
        !job_ids.contains(&"active-2"),
        "completed item should NOT be active"
    );
    assert!(
        job_ids.contains(&"active-3"),
        "queued item should be active"
    );

    guard.finish().await;
}

/// Test that get_history_items returns completed, failed, and cancelled items.
#[tokio::test]
async fn test_get_history_items_excludes_active() {
    let (svc, _pool, guard) = setup_service().await;

    // Enqueue and complete an item
    svc.enqueue(
        "history-1",
        "test/repo",
        "model1.gguf",
        None,
        "model",
        None,
        None,
    )
    .await
    .unwrap();
    svc.update_status("history-1", "completed", 5000, Some(5000), None, None)
        .await
        .unwrap();

    // Enqueue and fail an item
    svc.enqueue(
        "history-2",
        "test/repo",
        "model2.gguf",
        None,
        "model",
        None,
        None,
    )
    .await
    .unwrap();
    svc.update_status(
        "history-2",
        "failed",
        0,
        None,
        Some("Download failed"),
        None,
    )
    .await
    .unwrap();

    // Enqueue and cancel an item
    svc.enqueue(
        "history-3",
        "test/repo",
        "model3.gguf",
        None,
        "model",
        None,
        None,
    )
    .await
    .unwrap();
    svc.cancel("history-3").await.unwrap();

    // Enqueue a queued item (should NOT be in history)
    svc.enqueue(
        "history-4",
        "test/repo",
        "model4.gguf",
        None,
        "model",
        None,
        None,
    )
    .await
    .unwrap();

    let history = svc.get_history_items(100, 0).await.unwrap();
    let job_ids: Vec<&str> = history.iter().map(|i| i.job_id.as_str()).collect();

    assert!(
        job_ids.contains(&"history-1"),
        "completed item should be in history"
    );
    assert!(
        job_ids.contains(&"history-2"),
        "failed item should be in history"
    );
    assert!(
        job_ids.contains(&"history-3"),
        "cancelled item should be in history"
    );
    assert!(
        !job_ids.contains(&"history-4"),
        "queued item should NOT be in history"
    );

    guard.finish().await;
}

// ── PullEvent tagged serialization tests ──────────────────────────────

/// Test that all PullEvent variants serialize with the correct `event` tag.
#[tokio::test]
async fn test_pull_event_tagged_serialization_all_variants() {
    let cases: Vec<(PullEvent, &str)> = vec![
        (
            PullEvent::Started {
                job_id: "j".into(),
                repo_id: "a/b".into(),
                filename: "f".into(),
                total_bytes: Some(1),
            },
            "Started",
        ),
        (
            PullEvent::Progress {
                job_id: "j".into(),
                bytes_pulled: 1,
                total_bytes: None,
            },
            "Progress",
        ),
        (
            PullEvent::Verifying {
                job_id: "j".into(),
                filename: "f".into(),
            },
            "Verifying",
        ),
        (
            PullEvent::Completed {
                job_id: "j".into(),
                filename: "f".into(),
                size_bytes: 2,
                duration_ms: 3,
            },
            "Completed",
        ),
        (
            PullEvent::Failed {
                job_id: "j".into(),
                filename: "f".into(),
                error: "e".into(),
            },
            "Failed",
        ),
        (
            PullEvent::Cancelled {
                job_id: "j".into(),
                filename: "f".into(),
            },
            "Cancelled",
        ),
        (
            PullEvent::Queued {
                job_id: "j".into(),
                repo_id: "a/b".into(),
                filename: "f".into(),
            },
            "Queued",
        ),
    ];
    for (event, expected_name) in cases {
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["event"], expected_name);
        assert!(event.to_sse_event().is_ok());
    }
}
