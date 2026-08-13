//! Integration tests for docker image management.
//!
//! Uses a fake `docker` CLI script to simulate Docker behavior without requiring
//! a real Docker daemon.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Set up the fake docker CLI for a test.
///
/// Copies `fixtures/fake-docker.sh` into a per-test temp directory as `docker`,
/// makes it executable, and returns (temp_dir, docker_dir) so the caller can
/// prepend `docker_dir` to PATH and set `FAKE_DOCKER_STATE_DIR`.
fn setup_fake_docker() -> (tempfile::TempDir, PathBuf) {
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let docker_dir = tmpdir.path().join("bin");
    fs::create_dir_all(&docker_dir).expect("create bin dir");

    // Copy the fake docker script into the test's tempdir.
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-docker.sh");
    let dest_path = docker_dir.join("docker");
    fs::copy(&fixture_path, &dest_path).expect("copy fake-docker.sh");
    fs::set_permissions(&dest_path, PermissionsExt::from_mode(0o755))
        .expect("chmod +x fake-docker.sh");

    (tmpdir, docker_dir)
}

fn set_fake_docker_path() -> (tempfile::TempDir, PathBuf, String) {
    let (tmpdir, docker_dir) = setup_fake_docker();
    let state_dir = tmpdir.path().join("state");
    fs::create_dir_all(&state_dir).expect("create state dir");

    // Save original PATH.
    let original_path = std::env::var("PATH").unwrap_or_default();

    // Prepend docker_dir to PATH.
    let new_path = format!("{}:{}", docker_dir.display(), original_path);
    std::env::set_var("PATH", &new_path);
    std::env::set_var("FAKE_DOCKER_STATE_DIR", state_dir.to_str().unwrap());

    (tmpdir, docker_dir, original_path)
}

fn restore_docker_path(original_path: &str) {
    std::env::set_var("PATH", original_path);
    std::env::remove_var("FAKE_DOCKER_STATE_DIR");
}

// ─── docker_available tests ──────────────────────────────────────

#[tokio::test]
async fn test_docker_available_with_fake_docker() {
    let (_tmpdir, _docker_dir, original_path) = set_fake_docker_path();

    // Import the function under test.
    use tama_core::installations::docker::image::docker_available;

    let result = docker_available().await;
    assert!(
        result.is_ok(),
        "docker_available should succeed with fake docker: {:?}",
        result
    );

    restore_docker_path(&original_path);
}

// ─── is_image_present tests ──────────────────────────────────────

#[tokio::test]
async fn test_is_image_present_absent() {
    let (_tmpdir, _docker_dir, original_path) = set_fake_docker_path();

    use tama_core::installations::docker::image::is_image_present;

    // Image not yet pulled — should return false.
    let result = is_image_present("myrepo/myimage:latest").await;
    assert!(result.is_ok());
    assert!(!result.unwrap(), "should return false for absent image");

    restore_docker_path(&original_path);
}

#[tokio::test]
async fn test_is_image_present_present() {
    let (_tmpdir, _docker_dir, original_path) = set_fake_docker_path();

    use tama_core::installations::docker::image::is_image_present;

    // Pre-create the image state file so fake-docker thinks it exists.
    // The fake docker replaces `/` with `_` but keeps `:` → "myrepo_myimage:latest"
    let state_dir = std::env::var("FAKE_DOCKER_STATE_DIR").unwrap();
    let images_dir = PathBuf::from(&state_dir).join("images");
    fs::create_dir_all(&images_dir).expect("create images dir");
    let image_file = images_dir.join("myrepo_myimage:latest");
    fs::write(&image_file, "").expect("write image state file");

    // Now the image should be present.
    let result = is_image_present("myrepo/myimage:latest").await;
    assert!(result.is_ok());
    assert!(result.unwrap(), "should return true for present image");

    restore_docker_path(&original_path);
}

// ─── pull_image tests ────────────────────────────────────────────

#[tokio::test]
async fn test_pull_image_success() {
    let (_tmpdir, _docker_dir, original_path) = set_fake_docker_path();

    use tama_core::installations::docker::image::pull_image;

    let progress_lines: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let progress_clone = Arc::clone(&progress_lines);

    let cancel = CancellationToken::new();

    let result = pull_image(
        "myrepo/myimage:latest",
        move |line| {
            if !line.is_empty() {
                progress_clone.lock().unwrap().push(line);
            }
        },
        30,
        &cancel,
    )
    .await;

    assert!(result.is_ok(), "pull_image should succeed: {:?}", result);

    // Verify progress lines were received.
    let lines = progress_lines.lock().unwrap();
    assert!(!lines.is_empty(), "should receive progress lines");

    // Verify the image state file was created.
    // The fake docker replaces `/` with `_` but keeps `:` → "myrepo_myimage:latest"
    let state_dir = std::env::var("FAKE_DOCKER_STATE_DIR").unwrap();
    let images_dir = PathBuf::from(&state_dir).join("images");
    let image_file = images_dir.join("myrepo_myimage:latest");
    assert!(
        image_file.exists(),
        "pull should create image state file: {:?}",
        image_file
    );

    restore_docker_path(&original_path);
}

#[tokio::test]
async fn test_pull_image_cancelled() {
    let (_tmpdir, _docker_dir, _original_path) = set_fake_docker_path();

    use tama_core::installations::docker::image::pull_image;

    // Note: fake-docker.sh pull is fast (~0.6s), so we need to cancel quickly.
    // We'll create a custom slow fake docker for this test instead.
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let docker_dir = tmpdir.path().join("bin");
    fs::create_dir_all(&docker_dir).expect("create bin dir");

    let state_dir = tmpdir.path().join("state");
    fs::create_dir_all(&state_dir).expect("create state dir");

    // Write a slow fake docker that sleeps for 5 seconds.
    let slow_docker = r#"#!/usr/bin/env bash
set -euo pipefail
STATE_DIR="${FAKE_DOCKER_STATE_DIR:-}"
IMAGES_DIR="${STATE_DIR}/images"
mkdir -p "$IMAGES_DIR"
case "${1:-}" in
    pull)
        echo '{"status":"Pulling..."}'
        sleep 5
        touch "${IMAGES_DIR}/${2//\//_}"
        ;;
    *)
        exit 1
        ;;
esac
"#;
    let dest = docker_dir.join("docker");
    fs::write(&dest, slow_docker).expect("write slow docker");
    fs::set_permissions(&dest, PermissionsExt::from_mode(0o755)).expect("chmod +x");

    let original_path_var = std::env::var("PATH").unwrap_or_default();
    std::env::set_var(
        "PATH",
        format!("{}:{}", docker_dir.display(), original_path_var),
    );
    std::env::set_var("FAKE_DOCKER_STATE_DIR", state_dir.to_str().unwrap());

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Spawn pull in background, then cancel after a short delay.
    let pull_handle = tokio::spawn(async move {
        pull_image("slowrepo/slowimage:latest", |_line| {}, 30, &cancel).await
    });

    // Cancel after 200ms.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    cancel_clone.cancel();

    let result = pull_handle.await.expect("pull task should complete");
    assert!(result.is_err(), "pull should be cancelled: {:?}", result);
    assert!(
        result.unwrap_err().to_string().contains("cancelled"),
        "error should mention cancellation"
    );

    restore_docker_path(&original_path_var);
}

#[tokio::test]
async fn test_pull_image_timeout() {
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let docker_dir = tmpdir.path().join("bin");
    fs::create_dir_all(&docker_dir).expect("create bin dir");

    let state_dir = tmpdir.path().join("state");
    fs::create_dir_all(&state_dir).expect("create state dir");

    // Write a slow fake docker that sleeps for 10 seconds.
    let slow_docker = r#"#!/usr/bin/env bash
set -euo pipefail
STATE_DIR="${FAKE_DOCKER_STATE_DIR:-}"
IMAGES_DIR="${STATE_DIR}/images"
mkdir -p "$IMAGES_DIR"
case "${1:-}" in
    pull)
        echo '{"status":"Pulling..."}'
        sleep 10
        touch "${IMAGES_DIR}/${2//\//_}"
        ;;
    *)
        exit 1
        ;;
esac
"#;
    let dest = docker_dir.join("docker");
    fs::write(&dest, slow_docker).expect("write slow docker");
    fs::set_permissions(&dest, PermissionsExt::from_mode(0o755)).expect("chmod +x");

    let original_path_var = std::env::var("PATH").unwrap_or_default();
    std::env::set_var(
        "PATH",
        format!("{}:{}", docker_dir.display(), original_path_var),
    );
    std::env::set_var("FAKE_DOCKER_STATE_DIR", state_dir.to_str().unwrap());

    use tama_core::installations::docker::image::pull_image;

    let cancel = CancellationToken::new();

    // Use a 2-second timeout — the fake docker sleeps for 10 seconds.
    let result = pull_image("slowrepo/slowimage:latest", |_line| {}, 2, &cancel).await;

    assert!(result.is_err(), "pull should timeout: {:?}", result);
    assert!(
        result.unwrap_err().to_string().contains("timed out"),
        "error should mention timeout"
    );

    restore_docker_path(&original_path_var);
}
