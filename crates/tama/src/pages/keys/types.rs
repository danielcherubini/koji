use serde::{Deserialize, Serialize};

/// API key record returned by GET /tama/v1/keys and PATCH /tama/v1/keys/:id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    pub key_prefix: String,  // e.g. "tama_aB3dEfGh"
    pub scopes: Vec<String>, // e.g. ["inference", "management-read"]
    pub created_by: String,
    pub created_at: String, // RFC 3339
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub expires_at: Option<String>,
}

/// Response from POST /tama/v1/keys — includes the plaintext key (returned once).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateKeyResponse {
    pub id: i64,
    pub name: String,
    pub key: String, // Plaintext — returned ONCE
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// All available scopes (used for checkbox labels).
pub const AVAILABLE_SCOPES: &[(&str, &str)] = &[
    ("inference", "Allow making inference requests"),
    ("management-read", "Allow reading management endpoints"),
    ("management-write", "Allow writing management endpoints"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_deserialization() {
        let json = r#"{"id":1,"name":"k","key_prefix":"tama_aB3dEfGh","scopes":["inference","management-read"],"created_by":"admin","created_at":"2024-01-01T00:00:00Z","last_used_at":null,"revoked_at":null,"expires_at":null}"#;
        let key: ApiKey = serde_json::from_str(json).unwrap();
        assert_eq!(key.id, 1);
        assert_eq!(key.name, "k");
        assert_eq!(key.scopes, vec!["inference", "management-read"]);
        assert!(key.revoked_at.is_none());
    }

    #[test]
    fn test_api_key_deserialization_with_expiry() {
        let json = r#"{"id":2,"name":"expiring","key_prefix":"tama_xY9wAbCd","scopes":["management-write"],"created_by":"admin","created_at":"2024-01-01T00:00:00Z","last_used_at":"2024-06-01T00:00:00Z","revoked_at":null,"expires_at":"2025-12-31T23:59:59Z"}"#;
        let key: ApiKey = serde_json::from_str(json).unwrap();
        assert_eq!(key.name, "expiring");
        assert_eq!(key.scopes, vec!["management-write"]);
        assert_eq!(key.last_used_at, Some("2024-06-01T00:00:00Z".to_string()));
        assert_eq!(key.expires_at, Some("2025-12-31T23:59:59Z".to_string()));
    }

    #[test]
    fn test_create_key_response_deserialization() {
        let json = r#"{"id":2,"name":"new-key","key":"tama_abcdefghijklmnopqrstuvwxyz123456","scopes":["inference"],"expires_at":null,"created_at":"2024-01-01T00:00:00Z"}"#;
        let resp: CreateKeyResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.name, "new-key");
        assert!(resp.key.starts_with("tama_"));
        assert!(resp.expires_at.is_none());
    }

    #[test]
    fn test_create_key_response_with_expiry() {
        let json = r#"{"id":3,"name":"temp-key","key":"tama_1234567890abcdefghijklmnopqrstuvwxyz","scopes":["inference","management-read"],"expires_at":"2025-06-01T00:00:00Z","created_at":"2024-01-01T00:00:00Z"}"#;
        let resp: CreateKeyResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.name, "temp-key");
        assert_eq!(resp.scopes.len(), 2);
        assert_eq!(resp.expires_at, Some("2025-06-01T00:00:00Z".to_string()));
    }
}
