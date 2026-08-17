//! Bootstrap `config.toml` — the only on-disk config file in v3 (plan-190).
//!
//! In v3 the app config lives in the database (Postgres). The on-disk
//! `config.toml` is a *bootstrap* file that may contain ONLY a `[database]`
//! table. It is parsed by this small dedicated loader — NOT the full
//! `Config` serde path — and used by `main.rs` to build the Postgres pool.
//! The app never writes this file (only `tama migrate` does).

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;

const CONFIG_FILE: &str = "config.toml";
const DATABASE_TABLE: &str = "database";

/// The percent-encode set for userinfo (user/password) in a URL:
/// everything except unreserved characters and sub-delims (`!$&'()*+,;=`).
const USERINFO_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'|')
    .add(b'}')
    .add(b':')
    .add(b'@')
    .add(b'/')
    .add(b'?')
    .add(b'[')
    .add(b']');

fn default_db_host() -> String {
    "127.0.0.1".to_string()
}

fn default_db_port() -> u16 {
    5432
}

fn default_db_name() -> String {
    "tama".to_string()
}

fn default_db_user() -> String {
    "tama".to_string()
}

/// Postgres connection settings from the `[database]` bootstrap table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// Hostname or IP of the Postgres server.
    #[serde(default = "default_db_host")]
    pub host: String,
    /// Postgres port.
    #[serde(default = "default_db_port")]
    pub port: u16,
    /// Database name.
    #[serde(default = "default_db_name")]
    pub name: String,
    /// Database user.
    #[serde(default = "default_db_user")]
    pub user: String,
    /// Password — either a literal or a `"${ENV_VAR}"` reference.
    #[serde(default)]
    pub password: String,
}

impl DatabaseConfig {
    /// The Postgres DSN (`postgres://user:pass@host:port/name`) with
    /// user and password percent-encoded. The password is used as-is
    /// (resolve it first via [`Self::resolved_password`]).
    pub fn dsn(&self) -> String {
        let user = percent_encoding::utf8_percent_encode(&self.user, USERINFO_ENCODE_SET);
        let password = percent_encoding::utf8_percent_encode(&self.password, USERINFO_ENCODE_SET);
        format!(
            "postgres://{user}:{password}@{}:{}/{}",
            self.host, self.port, self.name
        )
    }

    /// Resolve the password, failing if a `${VAR}` reference is not set.
    pub fn resolved_password(&self) -> anyhow::Result<String> {
        resolve_env_var_ref_required(&self.password, "database.password")
    }
}

/// Load the bootstrap `config.toml` from `config_dir`.
///
/// - `config.toml` absent → `Ok(None)` (Postgres disabled — valid during
///   the SQLite→Postgres migration stages).
/// - `config.toml` present with only a `[database]` table → `Ok(Some(cfg))`.
/// - `config.toml` present with any other top-level tables → `Err` listing
///   them. This catches v2 users whose legacy `config.toml` was never
///   renamed — exactly the file `tama migrate` must not clobber.
pub fn load_bootstrap(config_dir: &Path) -> anyhow::Result<Option<DatabaseConfig>> {
    let path = config_dir.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read bootstrap file {}", path.display()))?;
    let doc: toml::Value = content
        .parse()
        .with_context(|| format!("Failed to parse bootstrap file {}", path.display()))?;
    let table = doc
        .as_table()
        .with_context(|| "bootstrap config.toml must be a TOML table")?;

    let unknown: Vec<&str> = table
        .keys()
        .filter(|k| k.as_str() != DATABASE_TABLE)
        .map(|k| k.as_str())
        .collect();
    if !unknown.is_empty() {
        anyhow::bail!(
            "bootstrap config.toml contains unexpected table(s): {unknown:?} — \
             only a [database] table is allowed. The app config now lives in the \
             database; if this is a legacy v2 config.toml, run `tama migrate` instead"
        );
    }

    let Some(database) = table.get(DATABASE_TABLE) else {
        // File exists but has no [database] table — Postgres disabled.
        return Ok(None);
    };

    let cfg = database
        .clone()
        .try_into()
        .with_context(|| format!("invalid [database] table in {}", path.display()))?;
    Ok(Some(cfg))
}

/// Resolve a `${VAR_NAME}` reference, failing if the env var is not set.
///
/// Plain values (no `${}`) pass through unchanged. Unlike the private
/// `resolve_env_var_ref` used for OAuth (warn-and-continue), a missing
/// env var here is a hard error — the daemon cannot start without the
/// Postgres password.
pub fn resolve_env_var_ref_required(value: &str, field: &str) -> anyhow::Result<String> {
    if let Some(var_name) = value.strip_prefix("${").and_then(|s| s.strip_suffix("}")) {
        match std::env::var(var_name) {
            Ok(val) => Ok(val),
            Err(_) => {
                anyhow::bail!("{field} references env var {var_name} which is not set")
            }
        }
    } else {
        Ok(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No config.toml → Postgres disabled.
    #[test]
    fn test_load_bootstrap_absent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_bootstrap(dir.path()).unwrap();
        assert!(result.is_none());
    }

    /// A [database] table parses with explicit values.
    #[test]
    fn test_load_bootstrap_parses_database_table() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[database]
host = "db.example.com"
port = 6432
name = "tama_prod"
user = "tama_admin"
password = "secret"
"#,
        )
        .unwrap();
        let cfg = load_bootstrap(dir.path())
            .unwrap()
            .expect("bootstrap should parse");
        assert_eq!(cfg.host, "db.example.com");
        assert_eq!(cfg.port, 6432);
        assert_eq!(cfg.name, "tama_prod");
        assert_eq!(cfg.user, "tama_admin");
        assert_eq!(cfg.password, "secret");
    }

    /// Omitted fields get defaults.
    #[test]
    fn test_load_bootstrap_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[database]\n").unwrap();
        let cfg = load_bootstrap(dir.path())
            .unwrap()
            .expect("bootstrap should parse");
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 5432);
        assert_eq!(cfg.name, "tama");
        assert_eq!(cfg.user, "tama");
        assert_eq!(cfg.password, "");
    }

    /// A stray [proxy] table is rejected with an error that names it and
    /// mentions the database.
    #[test]
    fn test_load_bootstrap_rejects_stray_tables() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[database]\n\n[proxy]\nport = 8080\n",
        )
        .unwrap();
        let err = load_bootstrap(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("proxy"), "should list the stray table: {msg}");
        assert!(
            msg.to_lowercase().contains("database"),
            "should hint that app config lives in the database: {msg}"
        );
    }

    /// A stray table with no [database] table at all is also rejected.
    #[test]
    fn test_load_bootstrap_rejects_legacy_file_without_database() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[backends]\nllama_cpp = { url = \"http://localhost:8080\" }\n",
        )
        .unwrap();
        let err = load_bootstrap(dir.path()).unwrap_err();
        assert!(err.to_string().contains("backends"));
    }

    /// An empty bootstrap file (no tables) means Postgres disabled.
    #[test]
    fn test_load_bootstrap_empty_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "").unwrap();
        assert!(load_bootstrap(dir.path()).unwrap().is_none());
    }

    /// A malformed [database] value is an error.
    #[test]
    fn test_load_bootstrap_invalid_type_is_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[database]\nport = \"not-a-number\"\n",
        )
        .unwrap();
        assert!(load_bootstrap(dir.path()).is_err());
    }

    /// Unknown fields inside [database] are rejected.
    #[test]
    fn test_load_bootstrap_rejects_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[database]\nssl_mode = \"disable\"\n",
        )
        .unwrap();
        assert!(load_bootstrap(dir.path()).is_err());
    }

    /// `${VAR}` is resolved when the env var is set.
    #[test]
    fn test_resolve_env_var_ref_required_set_var() {
        std::env::set_var("TAMA_DB_PW_TEST_SET", "s3cret");
        let result =
            resolve_env_var_ref_required("${TAMA_DB_PW_TEST_SET}", "database.password").unwrap();
        assert_eq!(result, "s3cret");
        std::env::remove_var("TAMA_DB_PW_TEST_SET");
    }

    /// `${VAR}` with the env var unset fails, naming the variable.
    #[test]
    fn test_resolve_env_var_ref_required_unset_var() {
        std::env::remove_var("TAMA_DB_PW_TEST_MISSING");
        let err = resolve_env_var_ref_required("${TAMA_DB_PW_TEST_MISSING}", "database.password")
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("TAMA_DB_PW_TEST_MISSING"),
            "error must name the missing var: {msg}"
        );
    }

    /// A plain value passes through unchanged.
    #[test]
    fn test_resolve_env_var_ref_required_plain_value() {
        let result = resolve_env_var_ref_required("p@ss:w/rd", "database.password").unwrap();
        assert_eq!(result, "p@ss:w/rd");
    }

    /// The DSN percent-encodes user and password.
    #[test]
    fn test_dsn_percent_encodes_credentials() {
        let cfg = DatabaseConfig {
            host: "localhost".to_string(),
            port: 5432,
            name: "tama".to_string(),
            user: "u@ser".to_string(),
            password: "p@ss:w/rd".to_string(),
        };
        assert_eq!(
            cfg.dsn(),
            "postgres://u%40ser:p%40ss%3Aw%2Frd@localhost:5432/tama"
        );
    }

    /// `resolved_password` resolves `${VAR}` and fails when unset.
    #[test]
    fn test_resolved_password() {
        std::env::set_var("TAMA_DB_PW_TEST_CFG", "env-pass");
        let cfg = DatabaseConfig {
            host: "h".to_string(),
            port: 5432,
            name: "db".to_string(),
            user: "u".to_string(),
            password: "${TAMA_DB_PW_TEST_CFG}".to_string(),
        };
        assert_eq!(cfg.resolved_password().unwrap(), "env-pass");
        std::env::remove_var("TAMA_DB_PW_TEST_CFG");
        assert!(cfg.resolved_password().is_err());

        let plain = DatabaseConfig {
            password: "literal".to_string(),
            ..cfg
        };
        assert_eq!(plain.resolved_password().unwrap(), "literal");
    }
}
