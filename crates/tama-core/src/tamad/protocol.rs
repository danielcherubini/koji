use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadModelRequest {
    pub provider_name: String,
    pub model_path: String,
    pub gpu_variant: String,
    pub params: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadModelResponse {
    pub endpoint_url: String,
    pub pid: i32,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnloadModelRequest {
    pub provider_name: String,
    pub model_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub engine: String,
    pub version: String,
    pub status: String,
    pub gpu_variant: String,
}
