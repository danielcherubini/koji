# Faster HF Downloads Plan

**Goal:** Replace hf-hub's slow downloader with our own enhanced parallel downloader, and fix HF token passthrough so gated repos work from CLI.

**Architecture:** Enhance the existing `download/parallel.rs` + `download/single.rs` with jitter-based retry backoff (adapted from hf_transfer) and auth header support. Replace `hf-hub`'s `download_with_progress` in `pull/download.rs` with our `download_chunked_with_progress`. Add `get_hf_token()` helper that respects `HF_TOKEN` env, `$HF_HOME/token`, and `~/.cache/huggingface/token`.

**Tech Stack:** Rust, tokio, reqwest, rand (new dep), hf-hub (kept for metadata only)

---

### Task 1: Add rand dependency and jitter helpers to downloader

**Context:**
The retry backoff in `parallel.rs` and `single.rs` uses `Duration::from_secs(2u64.pow(attempt - 1))` which is pure exponential with no jitter. This causes thundering herd on retries. The hf_transfer library uses `exponential_backoff(300, attempt, 10_000)` with random jitter (0-500ms) to spread retries. This task adds `rand` as a workspace dependency and the jitter helpers to both download modules.

**Files:**
- Modify: `Cargo.toml` (workspace) — add `rand = "0.9"` to `[workspace.dependencies]`
- Modify: `crates/tama-core/Cargo.toml` — add `rand.workspace = true`
- Modify: `crates/tama-core/src/models/download/parallel.rs` — add `jitter()` and `exponential_backoff()` helpers, replace backoff calls
- Modify: `crates/tama-core/src/models/download/single.rs` — add `jitter()` and `exponential_backoff()` helpers, replace backoff calls

**What to implement:**

1. In `Cargo.toml` (workspace root), add to `[workspace.dependencies]`:
   ```toml
   rand = "0.9"
   ```

2. In `crates/tama-core/Cargo.toml`, add:
   ```toml
   rand.workspace = true
   ```

3. In `crates/tama-core/src/models/download/parallel.rs`, add near the top (after existing imports):
   ```rust
   use rand::Rng;
   ```

   Add these helper functions (before `download_parallel`):
   ```rust
   /// Random jitter in milliseconds (0..=500), adapted from hf_transfer.
   fn jitter() -> u64 {
       rand::rng().random_range(0..=500)
   }

   /// Exponential backoff with jitter, adapted from hf_transfer.
   /// Base: 300ms, max: 10000ms.
   fn exponential_backoff(attempt: u32) -> Duration {
       let base = 300 + (attempt as u64).pow(2) + jitter();
       Duration::from_millis(base.min(10_000))
   }
   ```

   Replace ALL occurrences of `Duration::from_secs(2u64.pow(attempt - 1))` with `exponential_backoff(attempt)`.

   There are 4 such occurrences in `parallel.rs`:
   - Line ~173: after "Chunk {} failed (attempt {}/{}), retrying..."
   - Line ~192: after "Chunk {} got status {} (expected 206), retrying..."
   - Line ~237: after stream failure retry
   - Line ~251: after "Chunk {} short read ({}/{} bytes), retrying..."

4. In `crates/tama-core/src/models/download/single.rs`, add near the top:
   ```rust
   use rand::Rng;
   ```

   Add the same `jitter()` and `exponential_backoff()` helpers.

   Replace ALL occurrences of `Duration::from_secs(2u64.pow(attempt - 1))` with `exponential_backoff(attempt)`.

   There are 4 such occurrences in `single.rs`:
   - Line ~40: after "Download failed (attempt {}/{}), retrying..."
   - Line ~64: after "Server returned {}, retrying ({}/{})..."
   - Line ~77: after second "Server returned {}, retrying ({}/{})..."
   - Line ~135: after stream failure retry

**Steps:**
- [ ] Add `rand = "0.9"` to workspace `Cargo.toml` `[workspace.dependencies]`
- [ ] Add `rand.workspace = true` to `crates/tama-core/Cargo.toml`
- [ ] Add `jitter()` and `exponential_backoff()` to `parallel.rs`
- [ ] Replace backoff calls in `parallel.rs` (4 occurrences)
- [ ] Add `jitter()` and `exponential_backoff()` to `single.rs`
- [ ] Replace backoff calls in `single.rs` (4 occurrences)
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-core -- models::download`
  - Did all existing tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "feat: add jitter-based retry backoff to parallel downloader"

**Acceptance criteria:**
- [ ] `rand` is a workspace dependency used by `tama-core`
- [ ] `parallel.rs` uses `exponential_backoff(attempt)` instead of `2u64.pow(attempt - 1)`
- [ ] `single.rs` uses `exponential_backoff(attempt)` instead of `2u64.pow(attempt - 1)`
- [ ] All existing tests pass
- [ ] No clippy warnings

---

### Task 2: Add auth header support to existing downloader

**Context:**
The existing `download_chunked_with_progress` function has no way to pass authentication headers. This means gated repos on HuggingFace fail because the HEAD request, GET request, and Range requests all lack the `Authorization: Bearer <token>` header. This task adds an optional `headers` parameter through the entire download chain.

**Files:**
- Modify: `crates/tama-core/src/models/download/mod.rs` — add `headers` param to `download_chunked_with_progress` and `download_chunked`
- Modify: `crates/tama-core/src/models/download/parallel.rs` — add `headers` param to `download_parallel` and `download_chunk_with_retry`
- Modify: `crates/tama-core/src/models/download/single.rs` — add `headers` param to `download_single`

**What to implement:**

1. In `crates/tama-core/src/models/download/mod.rs`:

   Change `download_chunked` signature from:
   ```rust
   pub async fn download_chunked(
       client: &Client,
       url: &str,
       dest: &Path,
       connections: usize,
   ) -> Result<u64>
   ```
   To:
   ```rust
   pub async fn download_chunked(
       client: &Client,
       url: &str,
       dest: &Path,
       connections: usize,
       headers: Option<&HeaderMap>,
   ) -> Result<u64>
   ```
   Update the body to pass `headers` to `download_chunked_with_progress`.

   Change `download_chunked_with_progress` signature from:
   ```rust
   pub async fn download_chunked_with_progress(
       client: &Client,
       url: &str,
       dest: &Path,
       connections: usize,
       progress_callback: Option<ProgressCallback>,
   ) -> Result<u64>
   ```
   To:
   ```rust
   pub async fn download_chunked_with_progress(
       client: &Client,
       url: &str,
       dest: &Path,
       connections: usize,
       progress_callback: Option<ProgressCallback>,
       headers: Option<&HeaderMap>,
   ) -> Result<u64>
   ```

   In the body:
   - Add `.headers(headers.cloned().unwrap_or_default())` to the HEAD request
   - Pass `headers` to both `single::download_single` and `parallel::download_parallel`

2. In `crates/tama-core/src/models/download/parallel.rs`:

   Add import: `use reqwest::header::HeaderMap;`

   Change `download_parallel` signature to add `headers: Option<&HeaderMap>` as the last parameter.

   In the body, pass `headers` to each spawned `download_chunk_with_retry` call.

   Change `download_chunk_with_retry` signature to add `headers: &HeaderMap` as a parameter.

   In the body, add `.headers(headers.clone())` to the Range request:
   ```rust
   let request = client.get(url)
       .header("Range", &range)
       .headers(headers.clone());
   ```

3. In `crates/tama-core/src/models/download/single.rs`:

   Add import: `use reqwest::header::HeaderMap;`

   Change `download_single` signature to add `headers: Option<&HeaderMap>` as the last parameter.

   In the body, add `.headers(headers.cloned().unwrap_or_default())` to both the initial GET request and any resume GET request.

**Steps:**
- [ ] Add `headers` param to `download_chunked` in `mod.rs`
- [ ] Add `headers` param to `download_chunked_with_progress` in `mod.rs`
- [ ] Apply headers to HEAD request in `mod.rs`
- [ ] Pass headers to `single::download_single` and `parallel::download_parallel` in `mod.rs`
- [ ] Add `headers` param to `download_parallel` in `parallel.rs`
- [ ] Add `headers` param to `download_chunk_with_retry` in `parallel.rs`
- [ ] Apply headers to Range request in `download_chunk_with_retry`
- [ ] Add `headers` param to `download_single` in `single.rs`
- [ ] Apply headers to GET requests in `download_single`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors (check all call sites).
- [ ] Run `cargo test --package tama-core -- models::download`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add auth header support to parallel downloader"

**Acceptance criteria:**
- [ ] `download_chunked_with_progress` accepts `Option<&HeaderMap>` parameter
- [ ] Headers are applied to HEAD, GET, and Range requests
- [ ] All existing tests pass
- [ ] No clippy warnings

---

### Task 3: Add get_hf_token() and fix hf_api() token passthrough

**Context:**
The HF token is stored in `Config.general.hf_token` but is only used in the serve handler (where it's set as `HF_TOKEN` env var). CLI commands like `tama model pull` and `tama model update` never use the token, so gated repos fail. This task adds a `get_hf_token()` helper that resolves the token from env var, `$HF_HOME/token`, or `~/.cache/huggingface/token`, and wires it into `hf_api()` so hf-hub metadata calls also get authenticated.

**Files:**
- Modify: `crates/tama-core/src/models/pull/download.rs` — add `get_hf_token()` helper
- Modify: `crates/tama-core/src/models/pull/mod.rs` — add `.with_token(get_hf_token())` to `hf_api()` builder; move `get_hf_token` here or make it accessible
- Test: `crates/tama-core/src/models/pull/download.rs` — unit tests for `get_hf_token()` in `#[cfg(test)]` module

**What to implement:**

1. In `crates/tama-core/src/models/pull/mod.rs`, add the `get_hf_token` function (before `hf_api`):

   ```rust
   /// Resolve HF token from environment or token file.
   /// Priority: HF_TOKEN env → $HF_HOME/token → ~/.cache/huggingface/token
   fn get_hf_token() -> Option<String> {
       // 1. HF_TOKEN env var
       if let Ok(token) = std::env::var("HF_TOKEN") {
           let trimmed = token.trim().to_string();
           if !trimmed.is_empty() {
               return Some(trimmed);
           }
       }

       // 2. $HF_HOME/token
       if let Ok(hf_home) = std::env::var("HF_HOME") {
           let token_path = PathBuf::from(&hf_home).join("token");
           if let Ok(content) = std::fs::read_to_string(&token_path) {
               let trimmed = content.trim().to_string();
               if !trimmed.is_empty() {
                   return Some(trimmed);
               }
           }
       }

       // 3. ~/.cache/huggingface/token
       if let Some(home) = dirs::home_dir() {
           let token_path = home.join(".cache/huggingface/token");
           if let Ok(content) = std::fs::read_to_string(&token_path) {
               let trimmed = content.trim().to_string();
               if !trimmed.is_empty() {
                   return Some(trimmed);
               }
           }
       }

       None
   }
   ```

   Note: Check if `dirs` crate is already a dependency. If not, use `std::env::var("HOME")` instead:
   ```rust
   if let Ok(home) = std::env::var("HOME") {
       let token_path = PathBuf::from(&home).join(".cache/huggingface/token");
       // ...
   }
   ```

2. In the same file (`mod.rs`), update `hf_api()`:
   ```rust
   pub(crate) async fn hf_api() -> Result<&'static Api> {
       HF_API
           .get_or_try_init(|| async {
               let token = get_hf_token();
               ApiBuilder::new()
                   .with_token(token)
                   .with_max_files(8)
                   .build()
           })
           .await
   }
   ```

3. Add unit tests for `get_hf_token()` in `mod.rs` (or in a test module). Cover:
   - `HF_TOKEN` env var takes priority
   - `$HF_HOME/token` is used when env var not set
   - `~/.cache/huggingface/token` is fallback
   - Empty/whitespace-only values are treated as None
   - Returns None when no token source available

   Use `tempfile::tempdir()` for token file tests and `std::env::set_var`/`remove_var` for env var tests. Use a `std::sync::Mutex<()>` guard for env var tests to avoid needing `#[serial]` (which requires the `serial_test` crate).

4. In `crates/tama-core/src/models/pull/download.rs`:
   - **DO NOT remove** `ProgressAdapter` and `cleanup_hf_cache` yet — the proxy handler (Task 6) still needs them. They'll be removed in Task 6.
   - Remove `DownloadResult` struct (we'll redefine it simpler in Task 4)
   - Remove `download_gguf_with_progress` and `download_gguf` functions entirely — they'll be rewritten in Task 4

   Keep only the `#[cfg(test)]` module (remove tests for `download_gguf` and `download_gguf_with_progress`).

**Steps:**
- [ ] Check if `dirs` crate is a dependency (grep Cargo.toml). If not, use `std::env::var("HOME")` approach.
- [ ] Implement `get_hf_token()` in `pull/mod.rs` — make it `pub(crate)` so the proxy handler (Task 6) can access it
- [ ] Update `hf_api()` to call `.with_token(get_hf_token())`
- [ ] Write unit tests for `get_hf_token()` (env priority, file resolution, empty handling)
- [ ] Remove `download_gguf_with_progress` and `download_gguf` from `pull/download.rs`
- [ ] Remove `DownloadResult` from `pull/download.rs`
- [ ] **DO NOT remove** `ProgressAdapter` and `cleanup_hf_cache` — proxy handler still needs them
- [ ] Update `pull/mod.rs` re-exports: remove `download_gguf`, `download_gguf_with_progress`, `DownloadResult`
- [ ] Run `cargo build --workspace`
  - This will FAIL because consumers (pull.rs, update.rs, proxy handler) still import removed items. That's expected — they'll be fixed in Tasks 5-6.
  - Fix only compilation errors in `tama-core` (not `tama-cli` or proxy).
- [ ] Run `cargo test --package tama-core -- models::pull`
  - The `get_hf_token` tests should pass.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add HF token resolution and fix hf_api auth"

**Acceptance criteria:**
- [ ] `get_hf_token()` resolves: `HF_TOKEN` env → `$HF_HOME/token` → `~/.cache/huggingface/token` → None
- [ ] `hf_api()` passes token to `ApiBuilder::with_token()`
- [ ] `ProgressAdapter` and `cleanup_hf_cache` are **preserved** (removed in Task 6)
- [ ] Unit tests pass for `get_hf_token()`
- [ ] `tama-core` compiles (tama-cli errors expected — fixed next)

---

### Task 4: Replace hf-hub download with our parallel downloader

**Context:**
Now that the existing downloader supports auth headers (Task 2) and token resolution works (Task 3), we can replace hf-hub's slow downloader with our own. The new `download_gguf_with_progress` builds the HF resolve URL directly and calls `download_chunked_with_progress` with auth headers. This eliminates the hf-hub cache intermediary (no more symlink/canonicalize/cross-filesystem copy dance).

**Files:**
- Modify: `crates/tama-core/src/models/pull/download.rs` — rewrite with new `download_gguf_with_progress`
- Modify: `crates/tama-core/src/models/pull/mod.rs` — update re-exports

**What to implement:**

1. In `crates/tama-core/src/models/pull/download.rs`, write the new `download_gguf_with_progress`:

   ```rust
   use std::path::{Path, PathBuf};

   use anyhow::{Context, Result};
   use reqwest::Client;

   use crate::models::download::ProgressCallback;

   /// Result of downloading a GGUF file.
   #[derive(Debug)]
   pub struct DownloadResult {
       /// Local path to the file
       pub path: PathBuf,
       /// File size in bytes
       pub size_bytes: u64,
   }

   /// Download a GGUF file from a HuggingFace repo using our parallel downloader.
   /// Uses HTTP Range requests with auth headers for gated repos.
   pub async fn download_gguf_with_progress(
       repo_id: &str,
       filename: &str,
       dest_dir: &Path,
       progress_callback: Option<ProgressCallback>,
   ) -> Result<DownloadResult> {
       let endpoint = std::env::var("HF_ENDPOINT")
           .unwrap_or_else(|_| "https://huggingface.co".to_string());
       let url = format!(
           "{}/{}/resolve/main/{}",
           endpoint, repo_id, filename
       );

       let dest_path = dest_dir.join(filename);
       if let Some(parent) = dest_path.parent() {
           std::fs::create_dir_all(parent)
               .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
       }

       // Build auth headers
       let mut headers = reqwest::header::HeaderMap::new();
       if let Some(token) = super::get_hf_token() {
           headers.insert(
               reqwest::header::AUTHORIZATION,
               format!("Bearer {}", token).parse()
                   .context("Failed to parse Authorization header")?,
           );
       }

       // Build client with HTTP/2 keep-alive
       let client = Client::builder()
           .http2_keep_alive_timeout(std::time::Duration::from_secs(15))
           .build()
           .context("Failed to build HTTP client")?;

       let size_bytes = crate::models::download::download_chunked_with_progress(
           &client,
           &url,
           &dest_path,
           8, // max connections
           progress_callback,
           Some(&headers),
       )
       .await
       .with_context(|| format!("Failed to download '{}' from '{}'", filename, repo_id))?;

       Ok(DownloadResult {
           path: dest_path,
           size_bytes,
       })
   }
   ```

2. In `crates/tama-core/src/models/pull/mod.rs`, update re-exports:
   ```rust
   pub use download::{download_gguf_with_progress, DownloadResult};
   ```

3. Add a `#[cfg(test)]` module with at least one test:
   - Test that `download_gguf_with_progress` builds the correct URL format
   - Test that empty token is not added as header (mock or unit test of header building)

**Steps:**
- [ ] Implement `download_gguf_with_progress` in `pull/download.rs`
- [ ] Implement simplified `DownloadResult` struct
- [ ] Update re-exports in `pull/mod.rs`
- [ ] Add unit tests for URL construction and header building
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo test --package tama-core -- models::pull::download`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: replace hf-hub download with parallel downloader"

**Acceptance criteria:**
- [ ] `download_gguf_with_progress` builds HF resolve URL and calls `download_chunked_with_progress`
- [ ] Auth headers are added when token is available
- [ ] `tama-core` compiles cleanly
- [ ] Tests pass

---

### Task 5: Update CLI consumers (pull.rs, update.rs)

**Context:**
The CLI commands `tama model pull` and `tama model update` currently call `download_gguf` (which we removed in Task 3). They need to switch to `download_gguf_with_progress` which now handles everything (auth, progress bar, parallel downloads). The `Client::new()` in both files becomes dead code since the new function constructs its own client.

**Files:**
- Modify: `crates/tama-cli/src/commands/model/pull.rs` — switch to `download_gguf_with_progress`
- Modify: `crates/tama-cli/src/commands/model/update.rs` — switch to `download_gguf_with_progress`

**What to implement:**

1. In `crates/tama-cli/src/commands/model/pull.rs`:

   Remove: `use reqwest::Client;` (if only used for download)

   Find the download loop (around line 107):
   ```rust
   let client = Client::new();
   let result = pull::download_gguf(&client, repo_id, &gguf.filename, &model_dir).await?;
   ```

   Replace with:
   ```rust
   let result = pull::download_gguf_with_progress(
       repo_id,
       &gguf.filename,
       &model_dir,
       None, // progress callback — download_chunked shows its own progress bar
   ).await?;
   ```

   Remove the `let client = Client::new();` line.

2. In `crates/tama-cli/src/commands/model/update.rs`:

   Remove: `use reqwest::Client;`

   Find the download call (around line 184):
   ```rust
   let client = Client::new();
   let dl = tama_core::models::pull::download_gguf(&client, repo_id, &file_info.filename, &model.dir).await?;
   ```

   Replace with:
   ```rust
   let dl = tama_core::models::pull::download_gguf_with_progress(
       repo_id,
       &file_info.filename,
       &model.dir,
       None,
   ).await?;
   ```

   Remove the `let client = Client::new();` line.

**Steps:**
- [ ] Update `pull.rs` to use `download_gguf_with_progress`
- [ ] Remove `Client::new()` from `pull.rs`
- [ ] Remove unused `reqwest::Client` import from `pull.rs`
- [ ] Update `update.rs` to use `download_gguf_with_progress`
- [ ] Remove `Client::new()` from `update.rs`
- [ ] Remove unused `reqwest::Client` import from `update.rs`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: update CLI pull/update to use new downloader"

**Acceptance criteria:**
- [ ] `pull.rs` uses `download_gguf_with_progress` (no `Client::new()`)
- [ ] `update.rs` uses `download_gguf_with_progress` (no `Client::new()`)
- [ ] Workspace builds cleanly
- [ ] No clippy warnings

---

### Task 6: Update proxy handler and final cleanup

**Context:**
The proxy download handler (`proxy/tama_handlers/pull/download.rs`) still uses hf-hub's `download_with_progress` with `ProgressAdapter`. This task switches it to use our `download_chunked_with_progress` directly, simplifies `run_verification` (no more cache intermediary), and removes `ProgressAdapter` and `cleanup_hf_cache` from the codebase.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs` — replace hf-hub download + simplify verification
- Modify: `crates/tama-core/src/models/pull/download.rs` — remove `ProgressAdapter` and `cleanup_hf_cache`
- Modify: `crates/tama-core/src/models/pull/mod.rs` — update re-exports

**What to implement:**

1. **Replace the download call** (around lines 295-320 of `proxy/tama_handlers/pull/download.rs`):

   Current code:
   ```rust
   let repo = api.model(repo_id_clone.clone());
   let progress_adapter = crate::models::pull::ProgressAdapter::new(Some(progress_callback));
   let cached_path = match repo
       .download_with_progress(&filename_clone, progress_adapter)
       .await
   { /* ... error handling ... */ };
   ```

   Replace with:
   ```rust
   // Build resolve URL directly
   let endpoint = std::env::var("HF_ENDPOINT")
       .unwrap_or_else(|_| "https://huggingface.co".to_string());
   let url = format!("{}/{}/resolve/main/{}", endpoint, repo_id_clone, filename_clone);

   // Build auth headers
   let mut headers = reqwest::header::HeaderMap::new();
   if let Some(token) = crate::models::pull::get_hf_token() {
       headers.insert(
           reqwest::header::AUTHORIZATION,
           format!("Bearer {}", token).parse()
               .context("Failed to parse Authorization header")?,
       );
   }

   let client = reqwest::Client::builder()
       .http2_keep_alive_timeout(std::time::Duration::from_secs(15))
       .build()
       .context("Failed to build HTTP client")?;

   // Download directly to dest_path (no cache intermediary)
   let total_size = match crate::models::download::download_chunked_with_progress(
       &client, &url, &dest_path, 8,
       Some(progress_callback), Some(&headers),
   ).await {
       Ok(size) => size,
       Err(e) => {
           // ... same error handling as before, set job status to Failed ...
           return;
       }
   };
   ```

   After this change:
   - `cached_path` no longer exists — the file is at `dest_path`
   - `total_size` replaces `bytes` (file size from download)
   - Remove the `tokio::fs::metadata(&cached_path)` call (lines ~330) — use `total_size` instead

2. **Simplify `run_verification`** (around lines 564-760):

   The function currently takes `cached_path: PathBuf` and `dest_path: PathBuf` and:
   - Hashes `cached_path` (the hf-hub cache file)
   - On pass: canonicalize blob → rename/copy to dest → cleanup symlink
   - On fail: delete blob and symlink

   New behavior (file is already at `dest_path`):
   - Hash `dest_path` directly
   - On pass: skip move/copy (file is already in place)
   - On fail: delete `dest_path`

   Changes to `run_verification`:
   ```rust
   // Remove cached_path parameter (keep dest_path)
   async fn run_verification(
       pull_jobs: Arc<...>,
       _db_dir: Option<...>,
       download_queue: Option<...>,
       job_id: String,
       repo_id: String,
       filename: String,
       _quant_hint: Option<String>,
       dest_path: std::path::PathBuf,
       bytes: u64,
   ) -> VerificationOutcome {
   ```

   In the hashing section (around line 635):
   ```rust
   // OLD: let hash_src = cached_path.clone();
   // NEW: hash dest_path directly
   let hash_src = dest_path.clone();
   ```

   In the "verification passed" section (around lines 684-730):
   - Remove the entire `canonicalize(&cached_path)` block
   - Remove the `rename(&blob, &dest_path)` block
   - Remove the `copy(&blob, &dest_path)` fallback
   - Remove the symlink cleanup (`remove_file(&cached_path)`)
   - Keep the job status update (set to Completed)

   In the "verification failed" section (around lines 753-770):
   ```rust
   // OLD: delete blob and symlink from cache
   // NEW: delete dest_path
   tokio::fs::remove_file(&dest_path).await.ok();
   ```

3. **Update the call site** of `run_verification` (around line 378):
   ```rust
   // OLD:
   let outcome = run_verification(..., cached_path.clone(), dest_path.clone(), bytes).await;
   // NEW: remove cached_path argument
   let outcome = run_verification(..., dest_path.clone(), total_size).await;
   ```

4. **Remove `ProgressAdapter` and `cleanup_hf_cache`** from `pull/download.rs`:
   - Delete the `ProgressAdapter` struct and its `hf_hub::api::tokio::Progress` impl
   - Delete the `cleanup_hf_cache` function
   - Delete their tests from the `#[cfg(test)]` module

5. **Update re-exports** in `pull/mod.rs`:
   ```rust
   // Remove cleanup_hf_cache and ProgressAdapter from pub use
   pub use download::{download_gguf_with_progress, DownloadResult};
   ```

6. **Check hf-hub dependency**: It IS still needed for `list_gguf_files`, `fetch_blob_metadata`, `fetch_hf_metadata` (metadata API calls). DO NOT remove `hf-hub` from dependencies.

7. **Clean up unused imports** in `proxy/tama_handlers/pull/download.rs`:
   - Remove `ProgressAdapter` import
   - Remove hf-hub `ApiRepo` usage for downloads (keep for `repo.url()` if needed, or construct URL directly)

**Steps:**
- [ ] Replace hf-hub download in proxy handler with `download_chunked_with_progress`
- [ ] Remove `cached_path` variable — use `dest_path` and `total_size` throughout
- [ ] Update `run_verification` signature: remove `cached_path` parameter
- [ ] Update `run_verification`: hash `dest_path` directly instead of `cached_path`
- [ ] Update `run_verification`: remove move/copy/symlink cleanup on pass
- [ ] Update `run_verification`: delete `dest_path` on fail (instead of cache files)
- [ ] Update `run_verification` call site: remove `cached_path` argument
- [ ] Remove `ProgressAdapter` and `cleanup_hf_cache` from `pull/download.rs`
- [ ] Update `pull/mod.rs` re-exports: remove `cleanup_hf_cache`, `ProgressAdapter`
- [ ] Verify `hf-hub` is still needed (it is — for metadata)
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: update proxy handler to use parallel downloader"

**Acceptance criteria:**
- [ ] Proxy handler uses `download_chunked_with_progress` (no hf-hub download)
- [ ] `run_verification` hashes `dest_path` directly (no `cached_path`)
- [ ] `run_verification` skips move/copy on pass (file already at dest)
- [ ] `ProgressAdapter` and `cleanup_hf_cache` are fully removed from the codebase
- [ ] Workspace builds and all tests pass
- [ ] No clippy warnings

---

### Task 7: Verification and end-to-end testing

**Context:**
All code changes are complete. This task verifies the full workflow: token resolution, auth headers, parallel downloads, and CLI commands. Includes integration-level checks and cleanup.

**Files:**
- Test: `crates/tama-core/src/models/download/mod.rs` — add integration test for full download flow
- Test: `crates/tama-core/src/models/pull/mod.rs` — verify `get_hf_token` tests

**What to implement:**

1. Add an `#[ignore]` integration test that downloads a small public file:
   ```rust
   #[tokio::test]
   #[ignore]
   async fn test_download_gguf_with_progress_real() {
       let temp_dir = tempfile::tempdir().unwrap();
       let result = download_gguf_with_progress(
           "julien-c/dummy-unknown",
           "config.json",
           temp_dir.path(),
           None,
       ).await;
       assert!(result.is_ok());
       assert!(result.unwrap().path.exists());
   }
   ```

2. Run the full test suite:
   ```bash
   cargo test --workspace
   cargo clippy --workspace -- -D warnings
   cargo fmt --all --check
   ```

3. Verify no remaining references to removed items:
   ```bash
   grep -rn "ProgressAdapter\|cleanup_hf_cache" crates/
   ```
   Should return nothing (or only in tests/comments).

4. Run `cargo build --release --workspace` to verify release build.

**Steps:**
- [ ] Add `#[ignore]` integration test for real download
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all --check`
- [ ] Verify no remaining references to `ProgressAdapter` or `cleanup_hf_cache`
- [ ] Run `cargo build --release --workspace`
- [ ] Commit with message: "test: add integration test and verify full workflow"

**Acceptance criteria:**
- [ ] All workspace tests pass
- [ ] No clippy warnings
- [ ] Release build succeeds
- [ ] No orphaned references to removed items
