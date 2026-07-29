pub use crate::types::enums::{CompactionDevice, LogLevel, RestartPolicy};

impl From<LogLevel> for tracing::Level {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Debug => tracing::Level::DEBUG,
            LogLevel::Info => tracing::Level::INFO,
            LogLevel::Warn => tracing::Level::WARN,
            LogLevel::Error => tracing::Level::ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_to_tracing() {
        assert_eq!(tracing::Level::from(LogLevel::Debug), tracing::Level::DEBUG);
        assert_eq!(tracing::Level::from(LogLevel::Info), tracing::Level::INFO);
        assert_eq!(tracing::Level::from(LogLevel::Warn), tracing::Level::WARN);
        assert_eq!(tracing::Level::from(LogLevel::Error), tracing::Level::ERROR);
    }
}
