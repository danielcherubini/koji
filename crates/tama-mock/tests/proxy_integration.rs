//! Integration tests for tama-mock as a mock backend.
//!
//! These tests verify that tama-mock correctly serves the API endpoints
//! that a real llama-server would expose, enabling it to stand in for
//! integration testing of proxy forwarding and server lifecycle management.

use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Find a free port by binding to port 0.
fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Spawn tama-mock on a random port and wait for it to become healthy.
fn spawn_mock(port: u16) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_tama-mock"))
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn tama-mock")
}

/// Wait for tama-mock to respond on the given port, up to `timeout_secs`.
async fn wait_for_mock_ready(port: u16, timeout_secs: u64) {
    let url = format!("http://127.0.0.1:{port}/health");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        if tokio::time::Instant::now() >= deadline {
            panic!("Mock server did not become ready within {timeout_secs}s");
        }
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => return,
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

/// Verify that tama-mock correctly serves all API endpoints with canned JSON.
#[tokio::test]
async fn test_mock_backend_proxying_and_crash_detection() {
    let port = find_free_port();
    let mut mock_child = spawn_mock(port);

    // Wait for mock to be ready
    wait_for_mock_ready(port, 10).await;

    let client = reqwest::Client::new();

    // Test GET /v1/models — should return the canned model list
    let resp = client
        .get(format!("http://127.0.0.1:{port}/v1/models"))
        .send()
        .await
        .expect("models request should succeed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("valid JSON from /v1/models");
    assert_eq!(body["object"], "list");
    assert_eq!(body["data"][0]["id"], "mock-model");
    assert_eq!(body["data"][0]["object"], "model");

    // Test POST /v1/completions — should return timing data
    let resp = client
        .post(format!("http://127.0.0.1:{port}/v1/completions"))
        .header("Content-Type", "application/json")
        .body(r#"{"prompt":"Hello","max_tokens":10}"#)
        .send()
        .await
        .expect("completions request should succeed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("valid JSON from /v1/completions");
    assert_eq!(body["timings"]["predicted_per_second"], 42.5);

    // Test POST /v1/chat/completions — should return timing + usage data
    let resp = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"test","messages":[{"role":"user","content":"hi"}],"max_tokens":10}"#)
        .send()
        .await
        .expect("chat completions request should succeed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp
        .json()
        .await
        .expect("valid JSON from /v1/chat/completions");
    assert_eq!(body["timings"]["predicted_per_second"], 42.5);
    assert_eq!(body["timings"]["draft_n"], 10);
    assert_eq!(body["timings"]["draft_n_accepted"], 7);
    assert_eq!(body["usage"]["completion_tokens"], 12);

    // Now kill the mock backend to simulate a crash
    mock_child.kill().expect("kill mock");
    let _ = mock_child.wait();

    // After killing, requests should fail (connection refused)
    let resp = client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await;
    assert!(
        resp.is_err(),
        "Request to killed mock should fail with connection error"
    );
}

/// Verify that tama-mock serves /health even when --hang is passed,
/// and that the process can be killed cleanly.
#[tokio::test]
async fn test_mock_backend_health_and_hang_smoke() {
    let port = find_free_port();

    // Spawn with --hang flag (no token output, but still serves HTTP)
    let mut mock_child = Command::new(env!("CARGO_BIN_EXE_tama-mock"))
        .arg("--hang")
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn tama-mock --hang");

    // Wait for the mock to be ready
    wait_for_mock_ready(port, 10).await;

    let client = reqwest::Client::new();

    // Health endpoint should still return 200 even with --hang
    let resp = client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
        .expect("health check should succeed");
    assert_eq!(resp.status(), 200);

    // /v1/models should also work with --hang
    let resp = client
        .get(format!("http://127.0.0.1:{port}/v1/models"))
        .send()
        .await
        .expect("models endpoint should succeed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("valid JSON");
    assert_eq!(body["data"][0]["id"], "mock-model");

    // /v1/completions should also work with --hang
    let resp = client
        .post(format!("http://127.0.0.1:{port}/v1/completions"))
        .header("Content-Type", "application/json")
        .body(r#"{"prompt":"test","max_tokens":5}"#)
        .send()
        .await
        .expect("completions endpoint should succeed");
    assert_eq!(resp.status(), 200);

    // Kill the process cleanly
    mock_child.kill().expect("kill mock");
    let _ = mock_child.wait();
}
