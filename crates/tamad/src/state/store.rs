//! Per-model persistent store — the plan-193 source of truth for lifecycle.
//!
//! One JSON file per model on the tamad's host disk:
//! `<data_dir>/state/<config_key>.json` (dir 0700, files 0600). The body is
//! all 12 wire fields of [`LoadModelRequest`] (verbatim prost field names, so
//! rehydrating the stored fields into a fresh `LoadModelRequest` is
//! lossless) plus a small control block: `desired` (on-disk semantics =
//! "keep"), `user_flagged`, `max_restarts` and the unix-millis
//! `updated_at_ms`.
//!
//! Writes are atomic: serialize to a temp file IN THE SAME DIRECTORY and
//! `rename` over `<key>.json` — a crash mid-write never leaves a partial
//! `<key>.json`, at worst an orphan `<key>.json.<pid>.tmp` remains, which
//! `Store::new` sweeps and which no `list()`/`get()` can surface.
//! Corrupted JSON is logged and skipped: the daemon never interrupts boot
//! over a bad manifest.
//!
//! Only the tamad touches these files (ADR-0010): the proxy never reads or
//! writes host disk, and the tamad never touches Postgres.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use tama_core::tamad::LoadModelRequest;

/// Restart budget a freshly-loaded model gets before operator attention.
/// (Consumed by the T2 respawn sweep; T1 only persists it.)
#[allow(dead_code)]
pub const DEFAULT_MAX_RESTARTS: u32 = 10;

/// Everything tamad persists about a loaded model: the launch spec (all 12
/// `LoadModelRequest` wire fields, verbatim prost names) plus the lifecycle
/// control block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProcess {
    // ── 12 LoadModelRequest wire fields (prost names, verbatim) ──────────
    pub provider_name: String,
    pub model_path: String,
    pub gpu_variant: String,
    pub params: HashMap<String, String>,
    pub model_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub health_url: String,
    pub health_timeout_ms: i64,
    pub gpu_device: String,
    pub docker_config_json: String,
    // ── control block ───────────────────────────────────────────────────
    /// Persistence intent: `true` = "keep" the model (survive tamad
    /// restarts, respawn on boot).
    pub desired: bool,
    /// Operator flagged this model (no auto-restart).
    pub user_flagged: bool,
    /// Remaining restart budget (defaults to [`DEFAULT_MAX_RESTARTS`]).
    pub max_restarts: u32,
    /// Last write, unix milliseconds.
    pub updated_at_ms: i64,
}

/// Losslessly rehydrate the 12-prost `LoadModelRequest` from a manifest.
/// (The StoredProcess fields are the prost wire fields verbatim, so
/// this is a field-by-field copy.)
impl From<&StoredProcess> for LoadModelRequest {
    fn from(sp: &StoredProcess) -> Self {
        Self {
            provider_name: sp.provider_name.clone(),
            model_path: sp.model_path.clone(),
            gpu_variant: sp.gpu_variant.clone(),
            params: sp.params.clone(),
            model_name: sp.model_name.clone(),
            command: sp.command.clone(),
            args: sp.args.clone(),
            env: sp.env.clone(),
            health_url: sp.health_url.clone(),
            health_timeout_ms: sp.health_timeout_ms,
            gpu_device: sp.gpu_device.clone(),
            docker_config_json: sp.docker_config_json.clone(),
        }
    }
}

/// Filesystem-backed per-model store rooted at `<data_dir>/state/`.
///
/// [`Store::new`] reloads every `<key>.json` manifest at construction
/// (corrupted files are logged + skipped), which is what lets later
/// reboot sweeps see pre-restart state. Only the daemon process writes —
/// an `Arc<Store>` is the unit of sharing (plan-193 T2 holds it on
/// `TamadState`). The in-memory map is a fast view; the on-disk files
/// are the source of truth and are updated under the same call as the
/// map (file first, then the map on success — the map can never claim a
/// write the disk didn't take).
///
/// Note: T1 persists and only updates, so the insert/get/list/delete
/// face has no production caller yet — it is read by the plan-193 T2
/// respawn sweep. `#[allow(dead_code)]` is scoped to the affected items
/// (see the note above the `Store` implementation), not the whole module.
#[allow(dead_code)]
pub struct Store {
    dir: PathBuf,
    entries: RwLock<HashMap<String, StoredProcess>>,
    pid: u32,
}

// T1 persists; T2 (respawn sweep) reads — see the `allow` note above the struct.
#[allow(dead_code)]
impl Store {
    /// Open (or create) the store at `<data_dir>/state/` and load existing
    /// manifests.
    ///
    /// Creates the state dir with mode 0700 if it does not exist; an
    /// already-existing dir is accepted as-is (lenient — it inherits
    /// permissions from the data dir it lives under). Orphaned `*.tmp`
    /// files left behind by a previously crashed atomic write are removed.
    pub fn new(data_dir: &Path) -> Result<Self> {
        let dir = data_dir.join("state");
        let created = !dir.is_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create state dir '{}'", dir.display()))?;
        #[cfg(unix)]
        if created {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("failed to set mode 0700 on '{}'", dir.display()))?;
        }

        let mut entries = HashMap::new();
        let dir_entries = std::fs::read_dir(&dir)
            .with_context(|| format!("failed to read state dir '{}'", dir.display()))?;
        for entry in dir_entries {
            let entry = entry.context("state dir entry")?;
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if name.ends_with(".tmp") {
                // Orphaned from a crashed atomic write — no other process
                // writes here, so it is safe to remove at construction.
                let _ = std::fs::remove_file(entry.path());
                continue;
            }
            let Some(key) = name.strip_suffix(".json") else {
                // Not one of our manifests — ignore.
                continue;
            };
            if !is_valid_key(key) {
                warn!(key, "skipping state manifest with invalid key");
                continue;
            }
            let path = entry.path();
            match std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<StoredProcess>(&bytes).ok())
            {
                Some(sp) => {
                    entries.insert(key.to_string(), sp);
                }
                None => {
                    warn!(
                        path = %path.display(),
                        "failed to parse state manifest; skipping (corrupted?)"
                    );
                }
            }
        }

        Ok(Self {
            dir,
            entries: RwLock::new(entries),
            pid: std::process::id(),
        })
    }

    /// Atomically persist `req` under `key`.
    ///
    /// Writes to a temp file in the SAME DIRECTORY (mode 0600) and renames
    /// it over `<key>.json` — the rename is atomic, so a crash mid-write
    /// leaves the previous good manifest plus an orphan tmp, never a
    /// partial `<key>.json`. Re-insert into an existing key replaces the
    /// request but preserves the key's `user_flagged` and
    /// `max_restarts` — an operator's mark or an adjusted restart
    /// budget must not be clobbered by a re-insert.
    pub fn insert(&self, key: &str, req: &LoadModelRequest, desired: bool) -> Result<()> {
        check_key(key)?;

        let (user_flagged, max_restarts) = {
            let map = self.entries.read().unwrap_or_else(|p| p.into_inner());
            match map.get(key) {
                Some(prev) => (prev.user_flagged, prev.max_restarts),
                None => (false, DEFAULT_MAX_RESTARTS),
            }
        };

        let stored = StoredProcess {
            provider_name: req.provider_name.clone(),
            model_path: req.model_path.clone(),
            gpu_variant: req.gpu_variant.clone(),
            params: req.params.clone(),
            model_name: req.model_name.clone(),
            command: req.command.clone(),
            args: req.args.clone(),
            env: req.env.clone(),
            health_url: req.health_url.clone(),
            health_timeout_ms: req.health_timeout_ms,
            gpu_device: req.gpu_device.clone(),
            docker_config_json: req.docker_config_json.clone(),
            desired,
            user_flagged,
            max_restarts,
            updated_at_ms: now_unix_ms(),
        };
        self.write_stored(key, &stored)
    }

    /// Flip the persisted `user_flagged` bit for `key` (atomic
    /// re-write of the manifest, every other field untouched). This
    /// exists separately from [`insert`](Self::insert), which
    /// by design PRESERVES the existing bit — here it's the one
    /// write path that flips it (the trip point of the T2 restart
    /// budget).
    ///
    /// Errors if no manifest exists for the key (flagging
    /// nothing is not a silent no-op).
    pub fn set_user_flagged(&self, key: &str, flagged: bool) -> Result<()> {
        check_key(key)?;
        let mut next = {
            let map = self.entries.read().unwrap_or_else(|p| p.into_inner());
            match map.get(key) {
                Some(sp) => sp.clone(),
                None => {
                    bail!("no stored process for key '{key}'")
                }
            }
        };
        next.user_flagged = flagged;
        next.updated_at_ms = now_unix_ms();
        self.write_stored(key, &next)
    }

    /// The sole path for atomic writes of a manifest: serialize
    /// into a temp file inside the same directory (mode 0600) and
    /// rename it over `<key>.json` — on a mid-write crash the
    /// previous-good manifest + a tumbling `<key>.json.<pid>.tmp`
    /// fragment is all that remains, a broken `<key>.json` never
    /// does — and after that the in-memory view is updated
    /// (the map only updates the value after it has been synced
    /// to disk).
    fn write_stored(&self, key: &str, stored: &StoredProcess) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(stored)
            .with_context(|| format!("failed to serialize state '{key}'"))?;

        let target = self.path_for_key(key);
        let tmp = {
            let file_name = target
                .file_name()
                .and_then(|s| s.to_str())
                .expect("target file name set by path_for_key");
            self.dir.join(format!("{file_name}.{}.tmp", self.pid))
        };

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .with_context(|| format!("failed to open temp state file '{}'", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("failed to set mode 0600 on '{}'", tmp.display()))?;
        }
        use std::io::Write;
        file.write_all(&bytes)
            .with_context(|| format!("failed to write temp state file '{}'", tmp.display()))?;
        file.sync_data()
            .with_context(|| format!("failed to sync temp state file '{}'", tmp.display()))?;
        drop(file);
        std::fs::rename(&tmp, &target).with_context(|| {
            format!(
                "failed to replace '{}' with temp '{}'",
                target.display(),
                tmp.display()
            )
        })?;

        self.entries
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.to_string(), stored.clone());
        Ok(())
    }

    /// Load the manifest under `key` (a fresh value, so the caller may
    /// hold it past any store mutation).
    ///
    /// `None` for an unknown or invalid key; corrupted files never reach
    /// the in-memory view ([`Store::new`] filters them out and logs a
    /// skip), so corruption is logged, never fatal.
    pub fn get(&self, key: &str) -> Option<StoredProcess> {
        if !is_valid_key(key) {
            return None;
        }
        self.entries
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .cloned()
    }

    /// Every manifest, keyed-sorted for determinism.
    pub fn list(&self) -> Vec<StoredProcess> {
        let map = self.entries.read().unwrap_or_else(|p| p.into_inner());
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        keys.iter().map(|k| map[*k].clone()).collect()
    }

    /// Remove the manifest for `key`.
    ///
    /// Idempotent: deleting a key with no file — including an invalid
    /// key — is a no-op, not an error.
    pub fn delete(&self, key: &str) -> Result<()> {
        if !is_valid_key(key) {
            return Ok(());
        }
        let path = self.path_for_key(key);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                bail!("failed to remove state file '{}': {e}", path.display())
            }
        }
        let mut map = self.entries.write().unwrap_or_else(|p| p.into_inner());
        map.remove(key);
        Ok(())
    }

    fn path_for_key(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

fn is_valid_key(key: &str) -> bool {
    !key.is_empty() && !key.starts_with('/') && !key.contains('/') && !key.contains("..")
}

#[allow(dead_code)] // Called by the T2-facing insert() only.
fn check_key(key: &str) -> Result<()> {
    if !is_valid_key(key) {
        bail!("invalid config key: {key:?}");
    }
    Ok(())
}

#[allow(dead_code)] // Called by the T2-facing insert() only.
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A request with every one of the 12 wire fields filled.
    fn full_req() -> LoadModelRequest {
        let mut params = HashMap::new();
        params.insert("num_ctx".to_string(), "8192".to_string());
        params.insert("n_gpu_layers".to_string(), "-1".to_string());
        let mut env = HashMap::new();
        env.insert("CUDA_VISIBLE_DEVICES".to_string(), "1".to_string());
        env.insert("OMP_NUM_THREADS".to_string(), "16".to_string());
        LoadModelRequest {
            provider_name: "vllm".to_string(),
            model_path: "owner/repo/model-Q4_K_M.gguf".to_string(),
            gpu_variant: "cuda".to_string(),
            params,
            model_name: "my-model".to_string(),
            command: "/usr/local/bin/llama-server".to_string(),
            args: vec![
                "--model".to_string(),
                "my-model.gguf".to_string(),
                "--port".to_string(),
                "18080".to_string(),
            ],
            env,
            health_url: "http://127.0.0.1:18080/health".to_string(),
            health_timeout_ms: 45_000,
            gpu_device: "GPU1".to_string(),
            docker_config_json:
                r#"{"image":"vllm/vllm-openai:latest","container_port":18080,"host_port":118080}"#
                    .to_string(),
        }
    }

    /// Default request: every field empty / 0.
    fn empty_req() -> LoadModelRequest {
        LoadModelRequest::default()
    }

    /// A fresh store creates `<data_dir>/state` with mode 0700.
    #[test]
    #[cfg(unix)]
    fn test_state_dir_created_with_mode_0700() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        Store::new(dir.path()).unwrap();
        let meta = std::fs::metadata(dir.path().join("state")).expect("state dir created");
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o700,
            "state dir must be 0700"
        );
    }

    /// A persisted manifest is mode 0600 (launch specs can carry secrets).
    #[test]
    #[cfg(unix)]
    fn test_manifest_file_mode_is_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        store.insert("model-a", &full_req(), true).unwrap();
        let meta = std::fs::metadata(dir.path().join("state").join("model-a.json"))
            .expect("manifest created");
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "manifest must be 0600"
        );
    }

    /// All 12 wire fields round-trip through the on-disk manifest
    /// verbatim, plus control-block defaults on a first insert.
    #[test]
    fn test_roundtrip_all_12_fields() {
        let dir = tempfile::tempdir().unwrap();
        let req = full_req();
        let store = Store::new(dir.path()).unwrap();
        store.insert("model-full", &req, true).unwrap();

        let got = store.get("model-full").expect("manifest must exist");
        // The 12 wire fields, verbatim.
        assert_eq!(got.provider_name, req.provider_name);
        assert_eq!(got.model_path, req.model_path);
        assert_eq!(got.gpu_variant, req.gpu_variant);
        assert_eq!(got.params, req.params);
        assert_eq!(got.model_name, req.model_name);
        assert_eq!(got.command, req.command);
        assert_eq!(got.args, req.args);
        assert_eq!(got.env, req.env);
        assert_eq!(got.health_url, req.health_url);
        assert_eq!(got.health_timeout_ms, req.health_timeout_ms);
        assert_eq!(got.gpu_device, req.gpu_device);
        assert_eq!(got.docker_config_json, req.docker_config_json);
        // Control block defaults.
        assert!(got.desired, "desired recorded as given");
        assert!(!got.user_flagged, "user_flagged defaults to false");
        assert_eq!(got.max_restarts, DEFAULT_MAX_RESTARTS, "defaults to 10");
        assert!(got.updated_at_ms > 0, "updated_at_ms set");
    }

    /// A fully default request round-trips too (empty/0 values stay
    /// empty/0 after the disk hop).
    #[test]
    fn test_roundtrip_empty_fields() {
        let dir = tempfile::tempdir().unwrap();
        let req = empty_req();
        let store = Store::new(dir.path()).unwrap();
        store.insert("model-empty", &req, false).unwrap();

        let got = store.get("model-empty").expect("manifest must exist");
        assert_eq!(got.provider_name, req.provider_name);
        assert_eq!(got.model_path, req.model_path);
        assert_eq!(got.gpu_variant, req.gpu_variant);
        assert!(got.params.is_empty());
        assert_eq!(got.model_name, req.model_name);
        assert_eq!(got.command, req.command);
        assert!(got.args.is_empty());
        assert!(got.env.is_empty());
        assert_eq!(got.health_url, req.health_url);
        assert_eq!(got.health_timeout_ms, req.health_timeout_ms);
        assert_eq!(got.gpu_device, req.gpu_device);
        assert_eq!(got.docker_config_json, req.docker_config_json);
        assert!(!got.desired);
    }

    /// A crash mid-write (temp file left behind, rename never happened)
    /// cannot corrupt the store: after a daemon restart the previous
    /// good manifest is what's read back, and the orphan tmp is swept.
    #[test]
    fn test_crash_mid_write_keeps_last_good_version() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = full_req();
        let store = Store::new(dir.path()).unwrap();
        store.insert("model-a", &v1, true).unwrap();

        // Simulate a killed re-insert: a garbage temp file whose target
        // rename never happened.
        let orphan = dir
            .path()
            .join("state")
            .join(format!("model-a.json.{}.tmp", std::process::id()));
        std::fs::write(&orphan, b"{ partial json,_process_killed").unwrap();

        // Daemon restart → reloads from disk: the prior good manifest.
        let store_after = Store::new(dir.path()).unwrap();
        let got = store_after.get("model-a").expect("manifest must survive");
        assert_eq!(got.provider_name, v1.provider_name);
        assert_eq!(got.model_path, v1.model_path);
        assert_eq!(got.params, v1.params);
        assert_eq!(got.args, v1.args);
        assert!(!orphan.exists(), "orphan tmp is swept on construction");
    }

    /// `list()` survives with tmp files present and never surfaces
    /// orphan tmp files (documented behavior: `*.tmp` names are ignored
    /// by the loader and the in-memory view holds only parsed
    /// manifests). A re-insert whose temp write name equals the orphan's
    /// overwrites and renames it away; a foreign-named orphan is swept
    /// on the next [`Store::new`].
    #[test]
    fn test_orphaned_tmp_ignored_and_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let state_dir: PathBuf = dir.path().to_path_buf().join("state");
        let foreign = state_dir.join("model-x.json.orphan-other-pid.tmp");
        std::fs::write(&foreign, b"garbage").unwrap();

        // list() does not break and never surfaces tmp files.
        assert!(store.list().is_empty(), "no manifests yet");

        // Self-named orphan: exactly the temp name THIS daemon would
        // write for `model-y` — the re-insert overwrites + renames it.
        let self_named = state_dir.join(format!("model-y.json.{}.tmp", std::process::id()));
        std::fs::write(&self_named, b"garbage").unwrap();
        store.insert("model-y", &full_req(), true).unwrap();
        assert!(
            !self_named.exists(),
            "self-named orphan tmp overwritten and renamed away"
        );
        let got = store.get("model-y").expect("manifest written");
        assert_eq!(got.model_name, "my-model");

        // Foreign-named orphan is ignored until the next construction.
        assert_eq!(store.list().len(), 1, "only model-y in list");
        drop(store);
        let store = Store::new(dir.path()).unwrap();
        assert!(!foreign.exists(), "foreign orphan swept on construction");
        assert_eq!(store.list().len(), 1);
    }

    /// Path-traversal keys are rejected: `..`, absolute (`/x`) and
    /// embedded-separator keys all fail on insert (no file created), and
    /// get/list/delete stay safe no-ops, not panics.
    #[test]
    fn test_key_validation_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();

        for bad in ["..", "/etc/passwd", "a/../b", "a/b", ""] {
            assert!(
                store.insert(bad, &full_req(), true).is_err(),
                "insert({bad:?}) must be rejected"
            );
        }
        // No manifest files for any of those keys.
        let entries: Vec<_> = std::fs::read_dir(dir.path().join("state"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            entries.is_empty(),
            "no files for rejected keys: {entries:?}"
        );

        // get / list / delete stay safe no-ops (not panics, not errors).
        assert!(store.get("../evil").is_none());
        assert_eq!(store.list().len(), 0);
        store.delete("../evil").unwrap();
        store.delete("/etc/passwd").unwrap();
    }

    /// A corrupted manifest is logged and skipped — never fatal, never
    /// surfaced by list()/get().
    #[test]
    fn test_corrupted_manifest_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        store.insert("good-model", &full_req(), true).unwrap();
        std::fs::write(
            dir.path().join("state").join("broken.json"),
            b"this is { not json",
        )
        .unwrap();

        // Daemon restart: no panic, the bad file is skipped (logged).
        let store_after = Store::new(dir.path()).unwrap();
        assert!(
            store_after.get("broken").is_none(),
            "corrupt file must not surface"
        );
        let all = store_after.list();
        assert_eq!(all.len(), 1, "only the good manifest is listed");
        assert_eq!(all[0].model_name, "my-model");
    }

    /// A fresh store with no manifests lists empty.
    #[test]
    fn test_list_on_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        assert!(store.list().is_empty(), "fresh store lists nothing");
    }
}
