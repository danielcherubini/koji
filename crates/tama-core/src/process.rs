//! Pure launch-spec helpers shared with the tamad.
//!
//! The process-spawning/host-inspection machinery (`is_process_alive`,
//! `kill_process*`, `check_health`, `configure_backend_command`, process
//! groups) moved to the tamad crate in plan-191 Task 10 (ADR-0010: the
//! proxy spawns nothing). What remains here is the small slice that is
//! pure string manipulation on a launch spec the proxy builds and ships
//! to a tamad.

/// Override a CLI flag's value in an argument list (e.g. --host, --port).
/// If the flag exists, replaces its value. If not, appends the flag and value.
pub fn override_arg(args: &mut Vec<String>, flag: &str, value: &str) {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        if pos + 1 < args.len() {
            args[pos + 1] = value.to_string();
        } else {
            args.push(value.to_string());
        }
    } else {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_override_arg_replaces_existing() {
        let mut args = vec!["--port".to_string(), "8080".to_string(), "-m".to_string()];
        override_arg(&mut args, "--port", "9000");
        assert_eq!(
            args,
            vec!["--port".to_string(), "9000".to_string(), "-m".to_string()]
        );
    }

    #[test]
    fn test_override_appends_missing_flag() {
        let mut args = vec!["-m".to_string()];
        override_arg(&mut args, "--host", "127.0.0.1");
        assert_eq!(
            args,
            vec![
                "-m".to_string(),
                "--host".to_string(),
                "127.0.0.1".to_string()
            ]
        );
    }
}
