//! CLI parsing for the `tama` binary (plan-190 Task 10).
//!
//! Bare `tama` (no subcommand) runs the proxy server exactly as before.
//! `tama migrate` is the one-time v2→v3 cutover tool.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// Tama — local AI server with automatic backend management.
#[derive(Debug, Parser)]
#[command(name = "tama", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// One-time migration from the v2 SQLite database to Postgres
    Migrate(MigrateArgs),
    /// Headless administration on the proxy side (plan-193 T6): the live
    /// model rows, model load / unload, and engine-log tails
    Admin(AdminArgs),
}

/// Arguments for `tama admin` (plan-193 T6).
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct AdminArgs {
    #[command(subcommand)]
    pub verb: AdminVerb,
}

/// The verbs `tama admin` takes (plan-193 T6).
///
/// Exit codes: `0` on success; `2` when the key is not found (no live
/// row, never loaded, or an odd key — odd keys are not-found, not
/// mis-use); `13` when the model's restart budget is exhausted — the
/// CLI literal for the wire word `budget_exhausted` (wire `503`
/// maps the clue: there is no numeric code on the wire); any other
/// non-zero otherwise.
#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum AdminVerb {
    /// Print every live-model wire row as one line of JSON
    Status,
    /// Ensure the model is loaded (idempotent: an already-alive row
    /// never re-issues a second `LoadModel`)
    Load {
        /// Model config key (or alias)
        config_key: String,
    },
    /// Unload a loaded model
    Unload {
        /// Model config key
        config_key: String,
    },
    /// Tail the model's engine log (container-engine only: the wire
    /// `Logs` tails the `tama-<key>` container the backend runs in;
    /// native-backend logs are not captured in this plan)
    Logs {
        /// Model config key
        config_key: String,
    },
}

/// Arguments for `tama migrate`.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct MigrateArgs {
    /// Path to the v2 SQLite database (tama.db)
    #[arg(long)]
    pub sqlite: PathBuf,

    /// Postgres connection URL (postgres://user:pass@host:port/db)
    #[arg(long)]
    pub db: String,

    /// Name of the env var holding the Postgres password (recommended over
    /// embedding it in --db)
    #[arg(long)]
    pub password_env: Option<String>,

    /// Only report per-table counts; write nothing
    #[arg(long)]
    pub dry_run: bool,

    /// Allow re-running against a populated target and overwrite an existing
    /// bootstrap config.toml (backed up to config.toml.bak-<ts> first)
    #[arg(long)]
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bare_run_has_no_subcommand() {
        let cli = Cli::parse_from(["tama"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_migrate_args_parse() {
        let cli = Cli::parse_from([
            "tama",
            "migrate",
            "--sqlite",
            "/tmp/tama.db",
            "--db",
            "postgres://tama@127.0.0.1:5432/tama",
            "--password-env",
            "TAMA_DB_PASSWORD",
            "--dry-run",
            "--force",
        ]);
        let Some(Command::Migrate(args)) = cli.command else {
            panic!("expected migrate subcommand");
        };
        assert_eq!(args.sqlite, PathBuf::from("/tmp/tama.db"));
        assert_eq!(args.db, "postgres://tama@127.0.0.1:5432/tama");
        assert_eq!(args.password_env.as_deref(), Some("TAMA_DB_PASSWORD"));
        assert!(args.dry_run);
        assert!(args.force);
    }

    #[test]
    fn test_admin_status_parses() {
        let cli = Cli::parse_from(["tama", "admin", "status"]);
        assert_eq!(
            cli.command,
            Some(Command::Admin(AdminArgs {
                verb: AdminVerb::Status
            }))
        );
    }

    #[test]
    fn test_admin_load_and_unload_take_the_config_key() {
        let cli = Cli::parse_from(["tama", "admin", "load", "qwen3"]);
        assert_eq!(
            cli.command,
            Some(Command::Admin(AdminArgs {
                verb: AdminVerb::Load {
                    config_key: "qwen3".to_string()
                }
            }))
        );

        let cli = Cli::parse_from(["tama", "admin", "unload", "deepseek-r1"]);
        assert_eq!(
            cli.command,
            Some(Command::Admin(AdminArgs {
                verb: AdminVerb::Unload {
                    config_key: "deepseek-r1".to_string()
                }
            }))
        );
    }

    #[test]
    fn test_admin_logs_takes_the_config_key() {
        let cli = Cli::parse_from(["tama", "admin", "logs", "llama_cpp_1"]);
        assert_eq!(
            cli.command,
            Some(Command::Admin(AdminArgs {
                verb: AdminVerb::Logs {
                    config_key: "llama_cpp_1".to_string()
                }
            }))
        );
    }
}
