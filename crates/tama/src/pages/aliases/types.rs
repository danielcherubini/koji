use serde::{Deserialize, Serialize};

/// Alias data returned by the backend API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alias {
    pub id: i64,
    pub name: String,
    pub model_id: i64,
    pub model_name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Model option for the dropdown selector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelOption {
    pub id: i64,
    pub label: String,
}
