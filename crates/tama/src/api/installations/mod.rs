pub mod capabilities;
pub mod compaction;
pub mod install;
pub mod jobs;
pub mod list;
pub mod manage;
pub mod register;
pub mod tamad_job;
pub mod types;

// Re-export all public types and functions for backward compatibility
pub use capabilities::*;
pub use install::*;
pub use jobs::*;
pub use list::*;
pub use manage::*;
pub use register::*;
pub use types::*;

/// Returns true if a path parameter contains separators or traversal sequences.
pub fn is_path_traversal(value: &str) -> bool {
    value.contains('/') || value.contains('\\') || value.contains("..")
}

/// Reject path parameters containing separators/traversal with the canonical
/// 400 ValidationError response. `field` is the human-readable parameter name
/// (e.g. "backend name", "version", "gpu_variant").
#[allow(clippy::result_large_err)]
pub fn reject_traversal(value: &str, field: &str) -> Result<(), axum::response::Response> {
    if is_path_traversal(value) {
        Err(crate::api::error::error_response(
            axum::http::StatusCode::BAD_REQUEST,
            format!(
                "Invalid {}: path separators or traversal sequences not allowed",
                field
            ),
            Some("ValidationError"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn test_is_path_traversal() {
        // Should accept normal values
        assert!(!is_path_traversal("llama_cpp"));
        assert!(!is_path_traversal("1.2.3"));
        assert!(!is_path_traversal("cuda"));

        // Should reject path separators and traversal
        assert!(is_path_traversal("a/b"));
        assert!(is_path_traversal("a\\b"));
        assert!(is_path_traversal(".."));
        assert!(is_path_traversal("a..b"));
    }

    #[test]
    fn test_reject_traversal_returns_400() {
        // Invalid value → Err with 400 status
        let resp = reject_traversal("../x", "backend name").unwrap_err();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Valid value → Ok
        assert!(reject_traversal("llama_cpp", "backend name").is_ok());
    }
}
