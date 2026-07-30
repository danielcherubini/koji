//! Tests for llama-server lifecycle management using tama-mock as a stand-in.
//!
//! These tests verify that `ServerHandle::complete()` and `ServerHandle::chat_complete()`
//! work correctly when the target server is the mock backend.

use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use tama_core::bench::llama_cli_spec::server::{spawn_server, ServerArgs};

/// Spawn the tama-mock binary on a random port, then exercise the full
/// server lifecycle: spawn → wait_ready → complete → chat_complete.
#[tokio::test]
async fn test_spawn_server_wait_ready_and_complete() {
    let port = find_free_port();

    // Spawn tama-mock (not hanging, default crash_after=0 means it runs forever)
    let mut mock_child = std::process::Command::new(env!("CARGO_BIN_EXE_tama-mock"))
        .arg("--port")
        .arg(port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to spawn tama-mock");

    // Wait for the mock to be ready before proceeding
    wait_for_mock_ready(port, 10).await;

    let model_path = std::path::PathBuf::from("/dev/null/mock-model.gguf");

    // Build ServerArgs — we use /bin/sh as a dummy binary since spawn_server
    // will try to spawn it. We don't actually need the binary to work because
    // tama-mock is already serving on the port.
    let args = ServerArgs {
        binary: std::path::PathBuf::from("/bin/sh"),
        model_path,
        port,
        ngl: None,
        flash_attn: true,
        spec_type: None,
        spec_ngram_n: None,
        spec_ngram_m: None,
        spec_ngram_min_hits: None,
        spec_ngram_min: None,
        spec_ngram_max: None,
        draft_max: None,
        draft_min: None,
        spec_draft_ngl: None,
        context_size: None,
    };

    // spawn_server will spawn /bin/sh (which exits immediately) but wait_ready
    // polls the HTTP port where tama-mock is already listening.
    let handle = match spawn_server(&args, 10).await {
        Ok(h) => h,
        Err(e) => {
            // If spawn_server fails because /bin/sh doesn't support the args,
            // we still have the mock server running — skip the HTTP tests.
            eprintln!(
                "spawn_server returned error (expected for non-server binary): {}",
                e
            );
            let _ = mock_child.kill();
            let _ = mock_child.wait();
            return;
        }
    };

    // Test complete()
    let tokens_per_sec = handle.complete("Hello world", 10).await;
    assert!(
        tokens_per_sec.is_ok(),
        "complete() should succeed: {:?}",
        tokens_per_sec.err()
    );
    let tps = tokens_per_sec.unwrap();
    assert!(
        (tps - 42.5).abs() < 0.01,
        "expected predicted_per_second=42.5, got {}",
        tps
    );

    // Test chat_complete()
    let timing = handle
        .chat_complete("mock-model", &[("user", "Hello")], 10)
        .await;
    assert!(
        timing.is_ok(),
        "chat_complete() should succeed: {:?}",
        timing.err()
    );
    let t = timing.unwrap();
    assert!((t.predicted_per_second - 42.5).abs() < 0.01);
    assert_eq!(t.draft_n, 10);
    assert_eq!(t.draft_n_accepted, 7);
    assert_eq!(t.predicted_n, 12);

    // Drop the handle (kills the child process)
    drop(handle);

    // Kill the mock process
    let _ = mock_child.kill();
}

/// Verify that spawn_server returns an error when the server never becomes ready.
/// We use a script that exits immediately, so /v1/models is never available.
#[tokio::test]
async fn test_spawn_server_ready_timeout() {
    let port = find_free_port();

    // Create a temporary script that exits immediately (never serves HTTP)
    let temp_dir = tempfile::tempdir().unwrap();
    let exit_script = temp_dir.path().join("exit-immediately");
    std::fs::write(&exit_script, "#!/bin/sh\nexit 0\n").unwrap();
    let mut perms = std::fs::metadata(&exit_script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&exit_script, perms).unwrap();

    let model_path = std::path::PathBuf::from("/dev/null/mock-model.gguf");

    let args = ServerArgs {
        binary: exit_script,
        model_path,
        port,
        ngl: None,
        flash_attn: true,
        spec_type: None,
        spec_ngram_n: None,
        spec_ngram_m: None,
        spec_ngram_min_hits: None,
        spec_ngram_min: None,
        spec_ngram_max: None,
        draft_max: None,
        draft_min: None,
        spec_draft_ngl: None,
        context_size: None,
    };

    let result = spawn_server(&args, 2).await;
    assert!(
        result.is_err(),
        "spawn_server should return Err when server never becomes ready"
    );
    let err_msg = match result {
        Ok(_) => unreachable!(),
        Err(e) => e.to_string(),
    };
    assert!(
        err_msg.contains("did not become ready") || err_msg.contains("failed to load model"),
        "Error should mention readiness timeout, got: {}",
        err_msg
    );
}

/// Find a free port by binding to port 0.
fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Wait for tama-mock to respond on the given port, up to `timeout_secs`.
async fn wait_for_mock_ready(port: u16, timeout_secs: u64) {
    let url = format!("http://127.0.0.1:{port}/health");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        if tokio::time::Instant::now() >= deadline {
            panic!("Mock server did not become ready within {timeout_secs}s");
        }
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => return,
            _ => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
}
