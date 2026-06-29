//! Config command handler
//!
//! Handles `tama config show/edit/path` commands.

use anyhow::Result;
use tama_core::config::Config;

/// View or edit configuration
pub fn cmd_config(config: &Config, command: crate::cli::ConfigCommands) -> Result<()> {
    match command {
        crate::cli::ConfigCommands::Show => {
            let toml_str = toml::to_string_pretty(config)?;
            println!("{}", toml_str);
        }
        crate::cli::ConfigCommands::Edit => {
            let db_path = Config::config_dir()?.join("tama.db");
            println!(
                "Configuration is stored in the SQLite database at: {}",
                db_path.display()
            );
            println!("Use `tama config set <key> <value>` to modify individual settings.");
        }
        crate::cli::ConfigCommands::Path => {
            let path = Config::config_dir()?.join("tama.db");
            println!("{}", path.display());
        }
    }
    Ok(())
}
