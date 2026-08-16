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

#[derive(Debug, Subcommand)]
pub enum Command {
    /// One-time migration from the v2 SQLite database to Postgres
    Migrate(MigrateArgs),
}

/// Arguments for `tama migrate`.
#[derive(Debug, Args, Clone)]
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
}
