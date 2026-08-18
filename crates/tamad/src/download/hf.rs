//! `hf` CLI execution for whole-repo (safetensors/transformers) pulls.
//!
//! Host-side execution (ADR-0010): the `hf` CLI only ever runs on the
//! tamad's host. Moved from `tama_core::models::pull::hf_cli` (pure
//! helpers `scan_dir_bytes` / `stderr_tail_str` keep shared access and
//! stay in tama-core).

use std::path::Path;
use std::sync::Arc;

use tokio::io::AsyncReadExt;

/// Check that the `hf` CLI is available on the host.
///
/// Returns an install hint when the binary is missing or errors.
pub async fn check_hf_binary() -> Result<(), String> {
    const INSTALL_HINT: &str = "hf CLI not found. Install with: pip install -U huggingface_hub";
    match tokio::process::Command::new("hf")
        .arg("--version")
        .output()
        .await
    {
        Ok(output) if output.status.success() => Ok(()),
        _ => Err(INSTALL_HINT.to_string()),
    }
}

/// Spawn the `hf` CLI to download a whole repo into `dest`.
///
/// `binary` is injected (dependency injection) so unit tests can pass a stub
/// executable instead of requiring a real `hf` install. When `hf_token` is
/// `Some`, it is passed to the child via the `HF_TOKEN` environment variable.
/// The token is only ever set as a child env var — it must never be logged.
pub async fn spawn_hf_download(
    binary: &str,
    repo_id: &str,
    dest: &Path,
    hf_token: Option<&str>,
) -> Result<tokio::process::Child, String> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args([
        "download",
        repo_id,
        "--local-dir",
        dest.to_string_lossy().as_ref(),
    ]);
    if let Some(token) = hf_token {
        cmd.env("HF_TOKEN", token);
    }
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    cmd.spawn()
        .map_err(|e| format!("failed to spawn hf CLI: {e}"))
}

/// Spawn a task that reads the child's stderr in bounded 4 KiB chunks and
/// keeps only the last 4096 raw bytes in `sink`. The task self-terminates
/// when stderr hits EOF.
///
/// The sink holds RAW BYTES: the cap drains from the front of a `Vec<u8>` at
/// arbitrary byte offsets (always safe — `Vec::drain` cannot panic on a
/// mid-character offset the way `String::drain` can), and decoding happens
/// exactly once at read time (`stderr_tail_str`), so a multi-byte char
/// straddling a chunk boundary stays whole instead of becoming one U+FFFD.
/// A char straddling the CAP boundary loses at most its leading
/// byte and decodes to a single U+FFFD.
///
/// Chunks (not lines) are the unit of accumulation: the tail is capped after
/// every chunk, so a single huge line — or `\r`-only progress output without
/// newlines — can never grow an unbounded per-line buffer.
pub fn start_stderr_reader(
    stderr: tokio::process::ChildStderr,
    sink: Arc<tokio::sync::Mutex<Vec<u8>>>,
) -> tokio::task::JoinHandle<()> {
    const TAIL_CAP: usize = 4096;
    tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = [0u8; TAIL_CAP];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut tail = sink.lock().await;
                    tail.extend_from_slice(&buf[..n]);
                    let overflow = tail.len().saturating_sub(TAIL_CAP);
                    if overflow > 0 {
                        tail.drain(0..overflow);
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tama_core::models::pull::hf_cli::stderr_tail_str;

    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// Write an executable shell stub to `dir` and return its path.
    fn write_stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, PermissionsExt::from_mode(0o755)).unwrap();
        path
    }

    /// Test that spawn_hf_download returns Err when the binary does not exist.
    #[tokio::test]
    async fn test_spawn_hf_download_missing_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let result = spawn_hf_download(
            "definitely-not-a-real-binary-xyz",
            "foo/bar",
            tmp.path(),
            None,
        )
        .await;
        assert!(
            result.is_err(),
            "spawning a missing binary should fail, got: {result:?}"
        );
    }

    /// Test that the stub runs with the expected args, HF_TOKEN is injected
    /// only when provided, and the stderr tail captures stderr (not stdout).
    #[tokio::test]
    async fn test_spawn_hf_download_runs_stub_and_captures_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        // CLI argv is `hf download <repo> --local-dir <dest>` → dest is $4.
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\necho \"line1\"\necho \"err\" 1>&2\necho \"$HF_TOKEN\" > \"$4/.hf_token_check\"\nexit 0\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        let stub_str = stub.to_str().unwrap().to_string();

        // Run 1: with an explicit token.
        let mut child = spawn_hf_download(&stub_str, "foo/bar", &dest, Some("tok123"))
            .await
            .expect("stub should spawn");
        let sink: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert!(status.success());

        let tail = stderr_tail_str(&sink)
            .await
            .expect("stderr should be captured");
        assert!(
            tail.contains("err"),
            "stderr tail should contain 'err': {tail}"
        );
        assert!(
            !tail.contains("line1"),
            "stdout must not leak into the stderr tail: {tail}"
        );
        let marker = std::fs::read_to_string(dest.join(".hf_token_check")).unwrap();
        assert_eq!(
            marker.trim(),
            "tok123",
            "HF_TOKEN env must be injected when provided"
        );

        // Run 2: without a token — HF_TOKEN must not be set by us, only
        // inherited from the test process environment (if present there).
        let mut child = spawn_hf_download(&stub_str, "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink2: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink2.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert!(status.success());

        let marker = std::fs::read_to_string(dest.join(".hf_token_check")).unwrap();
        let inherited = std::env::var("HF_TOKEN").unwrap_or_default();
        assert_eq!(
            marker.trim(),
            inherited,
            "HF_TOKEN must only be inherited, never injected"
        );
    }

    /// Test that a non-zero exit code is observable and stderr is captured.
    #[tokio::test]
    async fn test_spawn_hf_download_nonzero_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\necho \"boom\" 1>&2\nexit 3\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();

        let mut child = spawn_hf_download(stub.to_str().unwrap(), "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert_eq!(status.code(), Some(3));
        let tail = stderr_tail_str(&sink)
            .await
            .expect("stderr should be captured");
        assert!(
            tail.contains("boom"),
            "stderr tail should contain 'boom': {tail}"
        );
    }

    /// Test that a single huge stderr line (> 8 KiB) is capped to the 4096-byte
    /// tail: the sink holds exactly the LAST 4096 bytes of the stream (the
    /// tail of the line, not its head), with no unbounded per-line buffering.
    #[tokio::test]
    async fn test_stderr_reader_caps_huge_line() {
        let tmp = tempfile::tempdir().unwrap();
        // One 9000-byte line of 'x' followed by a single newline — far larger
        // than the 4096-byte cap, written as a single line to the child's
        // stderr.
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\nhead -c 9000 /dev/zero | tr '\\0' 'x' 1>&2\necho '' 1>&2\nexit 0\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();

        let mut child = spawn_hf_download(stub.to_str().unwrap(), "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert!(status.success());

        let tail = sink.lock().await;
        // 9000 'x' + 1 newline = 9001 bytes → capped to the last 4096.
        assert_eq!(
            tail.len(),
            4096,
            "sink must be capped at exactly 4096, got {}",
            tail.len()
        );
        assert!(
            tail.ends_with(b"\n"),
            "cap must keep the TAIL of the line, not the head"
        );
        assert!(
            tail[..tail.len() - 1].iter().all(|&b| b == b'x'),
            "capped tail must be all 'x' (the tail of the line)"
        );
    }

    /// Regression: a multi-byte UTF-8 character straddling the 4096-byte cap
    /// must survive intact. 4094 'a' + "é" (2 bytes) + 'b' = 4097 bytes, so
    /// the cap drops exactly one byte — the first 'a' — and the é (bytes 4095–4096
    /// of the stream) lands right on the cap boundary. Decoding happens once
    /// at read time, so the é stays a single character (the old per-chunk
    /// `from_utf8_lossy` split it into one U+FFFD per 4 KiB chunk).
    #[tokio::test]
    async fn test_stderr_reader_multibyte_char_on_cap_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\n{ head -c 4094 /dev/zero | tr '\\0' 'a'; printf '\\303\\251b'; } 1>&2\nexit 0\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();

        let mut child = spawn_hf_download(stub.to_str().unwrap(), "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert!(status.success());

        let tail = stderr_tail_str(&sink)
            .await
            .expect("stderr should be captured");
        assert_eq!(
            tail,
            format!("{}éb", "a".repeat(4093)),
            "a multi-byte char on the cap boundary must stay whole, not split into U+FFFD"
        );
    }

    /// Regression: capping the sink must never panic. The old code capped a
    /// `String` with `drain(0..overflow)` where `overflow` was a BYTE offset —
    /// when that offset landed mid-character, `String::drain` PANICKED and
    /// killed the detached reader task. Here "é" opens the stream, so the
    /// first drain offset (1) lands inside the 2-byte char: the reader must
    /// survive, and the dropped leading byte decodes to at most one U+FFFD.
    #[tokio::test]
    async fn test_stderr_reader_cap_drain_mid_char_no_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\n{ printf '\\303\\251'; head -c 4095 /dev/zero | tr '\\0' 'a'; } 1>&2\nexit 0\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();

        let mut child = spawn_hf_download(stub.to_str().unwrap(), "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader
            .await
            .expect("stderr reader must not panic on a mid-char cap");
        assert!(status.success());

        let tail = stderr_tail_str(&sink)
            .await
            .expect("stderr should be captured");
        // 4097 bytes → 1 dropped: the leading byte of the é. The orphaned
        // trailing byte decodes to a single U+FFFD, then 4095 'a'.
        assert_eq!(
            tail,
            format!("\u{FFFD}{}", "a".repeat(4095)),
            "mid-char cap must drop bytes, not panic"
        );
    }
}
