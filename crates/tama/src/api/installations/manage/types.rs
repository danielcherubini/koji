use serde::Deserialize;

/// Query params for POST /tama/v1/backends/:name/update
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// Query params for DELETE /tama/v1/backends/:name/versions/:version
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoveVersionQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// Query params for POST /tama/v1/backends/:name/activate
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivateQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// POST /tama/v1/backends/:name/default-args
/// Update default_args for a backend in the backend_configs DB table.
#[derive(Deserialize)]
pub struct UpdateDefaultArgsRequest {
    pub default_args: Vec<String>,
}

/// Query params for POST /tama/v1/backends/:name/default-args
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DefaultArgsQuery {
    pub gpu_variant: String,
}

/// POST /tama/v1/backends/:name/default-env
/// Update default_env for a backend in the backend_configs DB table.
#[derive(Deserialize)]
pub struct UpdateDefaultEnvRequest {
    pub default_env: Vec<String>,
}

/// Query params for POST /tama/v1/backends/:name/default-env
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DefaultEnvQuery {
    pub gpu_variant: String,
}

/// Query params for POST /tama/v1/backends/:name/source
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SourceQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// PATCH /tama/v1/backends/:name
/// Consolidated backend config update for default_args, default_env, and health_check_url.
#[derive(Deserialize)]
pub struct BackendPatchBody {
    pub default_args: Option<Vec<String>>,
    pub default_env: Option<Vec<String>>,
    pub health_check_url: Option<String>, // None=preserve, Some(value)=set (clear via existing POST endpoints)
}

/// POST /tama/v1/backends/:name/rename
/// Rename a backend across every table that carries its display name, preserving
/// its stable logical_id so config (default args/env) and models survive intact.
#[derive(Deserialize)]
pub struct RenameBackendRequest {
    pub name: String,
}
