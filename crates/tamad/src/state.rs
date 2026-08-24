//! Tamad state: identity, directories, and the persisted bearer token.
//!
//! The token is generated once on first run and stored at
//! `<data_dir>/tamad.token` (mode 0600) so it stays stable across restarts.
//!
//! The [`store::Store`] persists per-model launch specs + lifecycle
//! control blocks under `<data_dir>/state/` (plan-193 T1).

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use rand::Rng;
use tracing::{info, warn};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub mod store;

/// Runtime state for the tamad daemon.
///
/// Built once at startup from CLI args + environment; the token is
/// generated on first run and persisted in `data_dir` so it survives
/// restarts.
pub struct TamadState {
    /// Name of this tamad (identity key for self-registration).
    pub name: String,
    /// URL the proxy should use to reach this tamad.
    pub public_url: String,
    /// Transport protocol: "grpc" or "http".
    pub protocol: String,
    /// Directory where model weights are stored (disk-sampling target for stats).
    pub models_dir: PathBuf,
    /// Directory for tamad-local data (token file, etc.).
    pub data_dir: PathBuf,
    /// Proxy base URL (from `TAMA_URL`); `None` disables self-registration.
    pub proxy_url: Option<String>,
    /// Proxy management token (from `TAMA_TOKEN`); `None` disables self-registration.
    pub proxy_token: Option<String>,
    /// This tamad's bearer token, persisted at `<data_dir>/tamad.token`.
    token: String,
    /// Per-model lifecycle store on host disk, `<data_dir>/state/` (T1).
    /// First production read lands with the T2 respawn sweep.
    #[allow(dead_code)]
    pub store: Arc<store::Store>,
}

impl TamadState {
    /// Build state from CLI args.
    ///
    /// - `--name` defaults to the local hostname
    /// - `--public-url` defaults to `grpc://<name>:<port>` (or `http://`
    ///   when the protocol is `http`)
    /// - `--models-dir` defaults to `$HOME/.tama/models`
    /// - `--data-dir` defaults to `$HOME/.tama`
    ///
    /// Self-registration is enabled only when both `TAMA_URL` and
    /// `TAMA_TOKEN` are present in the environment.
    pub fn from_cli(args: &crate::CliArgs) -> Result<Self> {
        let home = env::var("HOME").context("HOME environment variable is not set")?;
        let data_dir = args
            .data_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(&home).join(".tama"));
        let models_dir = args
            .models_dir
            .clone()
            .unwrap_or_else(|| data_dir.join("models"));

        let name = match &args.name {
            Some(n) => n.clone(),
            None => hostname::get()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "localhost".to_string()),
        };

        let port = args
            .addr
            .rsplit_once(':')
            .map(|(_, p)| p.to_string())
            .unwrap_or_else(|| "50051".to_string());
        let scheme = if args.protocol == "http" {
            "http"
        } else {
            "grpc"
        };
        let public_url = args
            .public_url
            .clone()
            .unwrap_or_else(|| format!("{}://{}:{}", scheme, name, port));

        let proxy_url = env::var("TAMA_URL").ok().filter(|s| !s.is_empty());
        let proxy_token = env::var("TAMA_TOKEN").ok().filter(|s| !s.is_empty());
        if proxy_url.is_none() || proxy_token.is_none() {
            warn!(
                "TAMA_URL/TAMA_TOKEN not fully set — self-registration disabled; \
                 tamad will serve locally for manual registration"
            );
        }

        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("Failed to create data dir '{}'", data_dir.display()))?;
        let token_path = data_dir.join("tamad.token");
        let token = if token_path.exists() {
            let existing = std::fs::read_to_string(&token_path)
                .with_context(|| format!("Failed to read token file '{}'", token_path.display()))?;
            existing.trim().to_string()
        } else {
            let mut rng = rand::rng();
            let bytes: [u8; 32] = rng.random();
            let fresh: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
            let mut opts = std::fs::OpenOptions::new();
            opts.create_new(true);
            opts.write(true);
            #[cfg(unix)]
            opts.mode(0o600);
            opts.open(&token_path).with_context(|| {
                format!("Failed to create token file '{}'", token_path.display())
            })?;
            std::fs::write(&token_path, &fresh).with_context(|| {
                format!("Failed to write token file '{}'", token_path.display())
            })?;
            fresh
        };
        info!(token_path = %token_path.display(), "Tamad token ready (persisted)");

        // Per-model persistent store (plan-193 T1): created right next to the
        // token-file setup; `Store::new` makes <data_dir>/state (0700) and
        // reloads the persisted manifests (corrupted ones are logged +
        // skipped — never fatal at boot).
        let store = store::Store::new(&data_dir)
            .with_context(|| format!("failed to open store in '{}'", data_dir.display()))?;

        Ok(Self {
            name,
            public_url,
            protocol: args.protocol.clone(),
            models_dir,
            data_dir,
            proxy_url,
            proxy_token,
            token,
            store: Arc::new(store),
        })
    }

    /// This tamad's bearer token.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Root of this host's backend install directories (plan-191 Task 7):
    /// `<data-dir>/install`, mirroring the layout the proxy used to manage
    /// directly (`install/<backend_type>/<gpu_variant>/<version>`).
    pub fn install_dir(&self) -> PathBuf {
        self.data_dir.join("install")
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn test_args(data_dir: &std::path::Path) -> crate::CliArgs {
        crate::CliArgs {
            addr: "127.0.0.1:50051".to_string(),
            protocol: "grpc".to_string(),
            name: Some("test-box".to_string()),
            public_url: None,
            models_dir: None,
            data_dir: Some(data_dir.to_path_buf()),
        }
    }

    /// Token file is created once, stable across two from_cli calls with
    /// the same data dir, and has mode 0600.
    #[test]
    fn test_token_created_once_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let args = test_args(dir.path());

        let s1 = TamadState::from_cli(&args).unwrap();
        let token1 = s1.token().to_string();
        assert_eq!(token1.len(), 64, "token should be 64 hex chars");
        assert!(
            token1.chars().all(|c| c.is_ascii_hexdigit()),
            "token should be lowercase hex: {}",
            token1
        );

        // Second "restart" with the same data dir reuses the persisted token.
        let s2 = TamadState::from_cli(&args).unwrap();
        assert_eq!(s2.token(), token1, "token must be stable across restarts");

        let meta = std::fs::metadata(dir.path().join("tamad.token")).unwrap();
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "token file must be mode 0600"
        );
    }

    /// `--public-url` defaults from the protocol and the port in `--addr`.
    #[test]
    fn test_public_url_defaults_from_protocol_and_port() {
        let dir = tempfile::tempdir().unwrap();
        let args = test_args(dir.path());

        let s = TamadState::from_cli(&args).unwrap();
        assert_eq!(s.name, "test-box");
        assert_eq!(s.public_url, "grpc://test-box:50051");

        let mut http_args = args.clone();
        http_args.protocol = "http".to_string();
        let s = TamadState::from_cli(&http_args).unwrap();
        assert_eq!(s.public_url, "http://test-box:50051");
    }

    /// Explicit `--public-url` and `--models-dir` override the defaults.
    #[test]
    fn test_explicit_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let args = crate::CliArgs {
            addr: "0.0.0.0:60060".to_string(),
            protocol: "grpc".to_string(),
            name: Some("box".to_string()),
            public_url: Some("grpc://gpu1.lan:60060".to_string()),
            models_dir: Some(dir.path().join("weights")),
            data_dir: Some(dir.path().to_path_buf()),
        };

        let s = TamadState::from_cli(&args).unwrap();
        assert_eq!(s.public_url, "grpc://gpu1.lan:60060");
        assert_eq!(s.models_dir, dir.path().join("weights"));
        assert_eq!(s.data_dir, dir.path());
    }
}
