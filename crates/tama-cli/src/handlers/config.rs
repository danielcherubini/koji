//! Config command handler
//!
//! Handles `tama config show` command.

use anyhow::Result;
use tama_core::config::Config;

/// View configuration
pub fn cmd_config(command: crate::cli::ConfigCommands) -> Result<()> {
    match command {
        crate::cli::ConfigCommands::Show => {
            let config = Config::load()?;
            let toml_str = toml::to_string_pretty(&config)?;
            println!("{}", toml_str);
        }
    }
    Ok(())
}
