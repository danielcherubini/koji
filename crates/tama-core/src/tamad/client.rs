//! Client for communicating with tamad instances via gRPC or HTTP.
//!
//! Supports both gRPC (via tonic) and HTTP (via reqwest) protocols.
//! Connections are established lazily on first use.

use anyhow::{anyhow, Context, Result};

use crate::providers::{Protocol, TamadConnection};
use crate::tamad::SystemStats;

/// Client for a single tamad instance.
///
/// Creates connections lazily — `channel` is `None` until the first
/// gRPC call, and the HTTP client is always available.
pub struct TamadClient {
    connection: TamadConnection,
    channel: Option<tonic::transport::Channel>,
    http_client: reqwest::Client,
}

impl TamadClient {
    /// Create a new TamadClient from a connection record.
    ///
    /// Does not establish any network connection — channels are created
    /// lazily on first use.
    pub fn new(connection: &TamadConnection) -> Self {
        Self {
            connection: connection.clone(),
            channel: None,
            http_client: reqwest::Client::new(),
        }
    }

    /// Ensure a gRPC channel is connected, then return a reference to it.
    ///
    /// Translates `grpc://` to `http://` for tonic's endpoint parser.
    async fn ensure_channel(&mut self) -> Result<&tonic::transport::Channel> {
        if self.channel.is_none() && self.connection.protocol.is_grpc() {
            let url = self.connection.url.replace("grpc://", "http://");
            let uri: http::Uri = url.parse().context("Invalid tamad URI")?;
            let endpoint =
                tonic::transport::Endpoint::new(uri).context("Failed to create endpoint")?;
            self.channel = Some(endpoint.connect().await?);
        }
        self.channel.as_ref().context("No gRPC channel available")
    }

    /// Wrap a gRPC request with the stored bearer token (if any).
    fn authed<T>(&self, message: T) -> tonic::Request<T> {
        let mut request = tonic::Request::new(message);
        if let Some(token) = &self.connection.token {
            if let Ok(value) = tonic::metadata::MetadataValue::try_from(format!("Bearer {}", token))
            {
                request.metadata_mut().insert(
                    tonic::metadata::MetadataKey::from_static("authorization"),
                    value,
                );
            }
        }
        request
    }

    /// Apply the stored bearer token to an HTTP request builder (if any).
    fn bearer(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.connection.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// Load a model on the remote tamad.
    pub async fn load_model(
        &mut self,
        req: &crate::tamad::LoadModelRequest,
    ) -> Result<crate::tamad::LoadModelResponse> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client.load_model(self.authed(req.clone())).await?;
                Ok(response.into_inner())
            }
            Protocol::Http => {
                let url = format!("{}/load-model", self.connection.url);
                let json_body = serde_json::json!({
                    "provider_name": req.provider_name,
                    "model_path": req.model_path,
                    "gpu_variant": req.gpu_variant,
                    "params": req.params,
                });
                let request = self.bearer(self.http_client.post(&url));
                let resp = request.json(&json_body).send().await?;
                if !resp.status().is_success() {
                    return Err(anyhow!(
                        "HTTP load-model failed with status {}: {}",
                        resp.status(),
                        resp.text().await.unwrap_or_default()
                    ));
                }
                let body: serde_json::Value = resp.json().await?;
                Ok(crate::tamad::LoadModelResponse {
                    endpoint_url: body["endpoint_url"].as_str().unwrap_or("").to_string(),
                    pid: body["pid"].as_i64().unwrap_or(0) as i32,
                    status: body["status"].as_str().unwrap_or("").to_string(),
                })
            }
        }
    }

    /// Unload a model on the remote tamad.
    pub async fn unload_model(&mut self, req: &crate::tamad::UnloadModelRequest) -> Result<()> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                client.unload_model(self.authed(req.clone())).await?;
                Ok(())
            }
            Protocol::Http => {
                let url = format!("{}/unload-model", self.connection.url);
                let json_body = serde_json::json!({
                    "provider_name": req.provider_name,
                    "model_name": req.model_name,
                });
                let request = self.bearer(self.http_client.post(&url));
                let resp = request.json(&json_body).send().await?;
                if !resp.status().is_success() {
                    return Err(anyhow!(
                        "HTTP unload-model failed with status {}: {}",
                        resp.status(),
                        resp.text().await.unwrap_or_default()
                    ));
                }
                Ok(())
            }
        }
    }

    /// Check the health of the tamad instance.
    ///
    /// Returns `true` if the tamad reports status "ok", `false` otherwise.
    /// Connection errors (network unreachable, etc.) propagate as `Err`.
    pub async fn health_check(&mut self) -> Result<bool> {
        Ok(self.health_check_full().await?.status == "ok")
    }

    /// Fetch the tamad's full `HealthResponse` (status + version).
    ///
    /// gRPC: the `HealthCheck` RPC. HTTP: the `/health` liveness endpoint,
    /// which returns `{status, version}` JSON. Used by the pool to cache the
    /// tamad's version per successful connection (plan-191 Task 9).
    pub async fn health_check_full(&mut self) -> Result<crate::tamad::HealthResponse> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client
                    .health_check(self.authed(crate::tamad::Empty {}))
                    .await?;
                Ok(response.into_inner())
            }
            Protocol::Http => {
                let request = self.bearer(
                    self.http_client
                        .get(format!("{}/health", self.connection.url)),
                );
                let resp = request.send().await?;
                let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!({}));
                Ok(crate::tamad::HealthResponse {
                    status: body
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    version: body
                        .get("version")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
            }
        }
    }

    /// Open the long-lived `StreamStats` stream (gRPC protocol only).
    ///
    /// A fresh channel is opened per stream: the stream outlives every
    /// other RPC, and a stale cached channel would keep broken transport
    /// state across reconnects. Returns an error for HTTP-protocol
    /// connections — the pool treats that as "no stats stream" and the
    /// handle stays in its initial "unknown" state.
    pub async fn stream_stats(&self) -> Result<tonic::Streaming<SystemStats>> {
        let channel = self
            .fresh_channel()
            .await
            .context("StreamStats requires a gRPC connection")?;
        let mut client = crate::tamad::TamadServiceClient::new(channel);
        let response = client
            .stream_stats(self.authed(crate::tamad::StatsRequest {}))
            .await?;
        Ok(response.into_inner())
    }

    /// Dispatch a model pull on the remote tamad (plan-191 Task 6).
    ///
    /// Returns the tamad-side job id; stream its progress with
    /// [`stream_job`](Self::stream_job). HTTP-protocol connections are not
    /// supported (the job API is gRPC-only).
    pub async fn pull_model(&mut self, req: &crate::tamad::PullModelRequest) -> Result<String> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client.pull_model(self.authed(req.clone())).await?;
                Ok(response.into_inner().job_id)
            }
            Protocol::Http => Err(anyhow!(
                "pull_model requires a gRPC connection (got HTTP protocol)"
            )),
        }
    }

    /// Ask the remote tamad to cancel a running job (plan-191 follow-up B).
    ///
    /// Idempotent at the wire level: the tamad reports `cancelled = false`
    /// for unknown or already-terminal ids, so the caller may retry after a
    /// reconnect without side effects. HTTP-protocol connections are not
    /// supported (job API is gRPC-only).
    pub async fn cancel_job(&mut self, tamad_job_id: &str) -> Result<bool> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client
                    .cancel_job(self.authed(crate::tamad::CancelJobRequest {
                        job_id: tamad_job_id.to_string(),
                    }))
                    .await?;
                Ok(response.into_inner().cancelled)
            }
            Protocol::Http => Err(anyhow!(
                "cancel_job requires a gRPC connection (got HTTP protocol)"
            )),
        }
    }

    /// Dispatch a backend install on the remote tamad (plan-191 Task 7).
    ///
    /// Returns the tamad-side job id; stream its progress with
    /// [`stream_job`](Self::stream_job). HTTP-protocol connections are not
    /// supported (the job API is gRPC-only).
    pub async fn install_provider(
        &mut self,
        req: &crate::tamad::InstallProviderRequest,
    ) -> Result<String> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client.install_provider(self.authed(req.clone())).await?;
                Ok(response.into_inner().job_id)
            }
            Protocol::Http => Err(anyhow!(
                "install_provider requires a gRPC connection (got HTTP protocol)"
            )),
        }
    }

    /// Dispatch a backend update on the remote tamad (plan-191 Task 7).
    ///
    /// Returns the tamad-side job id; stream its progress with
    /// [`stream_job`](Self::stream_job).
    pub async fn update_provider(
        &mut self,
        req: &crate::tamad::UpdateProviderRequest,
    ) -> Result<String> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client.update_provider(self.authed(req.clone())).await?;
                Ok(response.into_inner().job_id)
            }
            Protocol::Http => Err(anyhow!(
                "update_provider requires a gRPC connection (got HTTP protocol)"
            )),
        }
    }

    /// Remove a backend install (files + processes) on the remote tamad
    /// (plan-191 Task 7, synchronous RPC).
    pub async fn remove_provider(
        &mut self,
        req: &crate::tamad::RemoveProviderRequest,
    ) -> Result<()> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                client.remove_provider(self.authed(req.clone())).await?;
                Ok(())
            }
            Protocol::Http => Err(anyhow!(
                "remove_provider requires a gRPC connection (got HTTP protocol)"
            )),
        }
    }

    /// Dispatch a benchmark on the remote tamad (plan-191 Task 8).
    ///
    /// Returns the tamad-side job id; stream its progress with
    /// [`stream_job`](Self::stream_job). The benchmark subprocesses run on
    /// the tamad's host (ADR-0010: the proxy spawns nothing).
    pub async fn run_benchmark(
        &mut self,
        req: &crate::tamad::RunBenchmarkRequest,
    ) -> Result<String> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client.run_benchmark(self.authed(req.clone())).await?;
                Ok(response.into_inner().job_id)
            }
            Protocol::Http => Err(anyhow!(
                "run_benchmark requires a gRPC connection (got HTTP protocol)"
            )),
        }
    }

    /// Open the long-lived `StreamJob` stream for a tamad job (gRPC only).
    ///
    /// A fresh channel is opened per stream (same rationale as
    /// `stream_stats`). The stream ends when the terminal job event is
    /// emitted; a stream that ends before a terminal event means the tamad
    /// went away mid-job.
    pub async fn stream_job(
        &self,
        job_id: &str,
    ) -> Result<tonic::Streaming<crate::tamad::JobEvent>> {
        let channel = self
            .fresh_channel()
            .await
            .context("StreamJob requires a gRPC connection")?;
        let mut client = crate::tamad::TamadServiceClient::new(channel);
        let response = client
            .stream_job(self.authed(crate::tamad::JobRequest {
                job_id: job_id.to_string(),
            }))
            .await?;
        Ok(response.into_inner())
    }

    /// Open a fresh gRPC channel (uncached — for long-lived streams).
    async fn fresh_channel(&self) -> Result<tonic::transport::Channel> {
        if !self.connection.protocol.is_grpc() {
            return Err(anyhow!(
                "no gRPC channel available for HTTP-protocol connection"
            ));
        }
        let url = self.connection.url.replace("grpc://", "http://");
        let uri: http::Uri = url.parse().context("Invalid tamad URI")?;
        let endpoint = tonic::transport::Endpoint::new(uri).context("Failed to create endpoint")?;
        Ok(endpoint.connect().await?)
    }

    /// Get a reference to the underlying connection record.
    pub fn connection(&self) -> &TamadConnection {
        &self.connection
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{atomic::AtomicUsize, Arc};

    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_http_connection(url: &str) -> TamadConnection {
        TamadConnection {
            id: "test-uuid".to_string(),
            name: "test-tamad".to_string(),
            url: url.to_string(),
            protocol: Protocol::Http,
            token: None,
            status: crate::providers::TamadStatus::Unknown,
        }
    }

    fn make_grpc_connection() -> TamadConnection {
        TamadConnection {
            id: "test-uuid".to_string(),
            name: "test-tamad".to_string(),
            url: "grpc://localhost:50051".to_string(),
            protocol: Protocol::Grpc,
            token: Some("secret".to_string()),
            status: crate::providers::TamadStatus::Unknown,
        }
    }

    // ── Construction tests ──

    #[test]
    fn test_tamad_client_new_http() {
        let conn = make_http_connection("http://localhost:8080");
        let client = TamadClient::new(&conn);
        assert!(client.connection().protocol.is_http());
        assert_eq!(client.connection().url, "http://localhost:8080");
        assert!(client.channel.is_none());
    }

    #[test]
    fn test_tamad_client_new_grpc() {
        let conn = make_grpc_connection();
        let client = TamadClient::new(&conn);
        assert!(client.connection().protocol.is_grpc());
        assert_eq!(client.connection().url, "grpc://localhost:50051");
        assert!(client.channel.is_none());
    }

    #[test]
    fn test_tamad_client_connection_ref() {
        let conn = make_http_connection("http://127.0.0.1:9090");
        let client = TamadClient::new(&conn);
        let conn_ref = client.connection();
        assert_eq!(conn_ref.id, "test-uuid");
        assert_eq!(conn_ref.name, "test-tamad");
    }

    // ── HTTP health check tests ──

    #[tokio::test]
    async fn test_health_check_http_healthy() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "ok",
                "version": "1.0.0"
            })))
            .mount(&mock_server)
            .await;

        let conn = make_http_connection(&mock_server.uri());
        let mut client = TamadClient::new(&conn);
        let result = client.health_check().await;
        assert!(result.is_ok(), "health check should succeed: {:?}", result);
        assert!(result.unwrap(), "health check should return true");
    }

    #[tokio::test]
    async fn test_health_check_http_unhealthy() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&mock_server)
            .await;

        let conn = make_http_connection(&mock_server.uri());
        let mut client = TamadClient::new(&conn);
        let result = client.health_check().await;
        assert!(result.is_ok(), "HTTP call should succeed even for 500");
        assert!(!result.unwrap(), "health check should return false for 500");
    }

    #[tokio::test]
    async fn test_health_check_http_not_found() {
        let mock_server = MockServer::start().await;
        // No mock mounted — returns 404
        let conn = make_http_connection(&mock_server.uri());
        let mut client = TamadClient::new(&conn);
        let result = client.health_check().await;
        assert!(result.is_ok(), "HTTP call should succeed even for 404");
        assert!(!result.unwrap(), "health check should return false for 404");
    }

    // ── HTTP load_model tests ──

    #[tokio::test]
    async fn test_load_model_http() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/load-model"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "endpoint_url": "http://localhost:8081/v1",
                "pid": 12345,
                "status": "loaded"
            })))
            .mount(&mock_server)
            .await;

        let conn = make_http_connection(&mock_server.uri());
        let mut client = TamadClient::new(&conn);

        // Build LoadModelRequest using the generated prost type
        let mut params = HashMap::new();
        params.insert("ctx_size".to_string(), "4096".to_string());
        let req = crate::tamad::LoadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_path: "/models/test.gguf".to_string(),
            gpu_variant: "cpu".to_string(),
            params,
            model_name: String::new(),
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            health_url: String::new(),
            health_timeout_ms: 0,
            gpu_device: String::new(),
            docker_config_json: String::new(),
        };
        let result = client.load_model(&req).await;
        assert!(result.is_ok(), "load_model should succeed: {:?}", result);
        let resp = result.unwrap();
        assert_eq!(resp.endpoint_url, "http://localhost:8081/v1");
        assert_eq!(resp.pid, 12345);
        assert_eq!(resp.status, "loaded");
    }

    #[tokio::test]
    async fn test_load_model_http_failure() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/load-model"))
            .respond_with(ResponseTemplate::new(500).set_body_string("load failed"))
            .mount(&mock_server)
            .await;

        let conn = make_http_connection(&mock_server.uri());
        let mut client = TamadClient::new(&conn);

        let req = crate::tamad::LoadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_path: "/models/test.gguf".to_string(),
            gpu_variant: "cpu".to_string(),
            params: HashMap::new(),
            model_name: String::new(),
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            health_url: String::new(),
            health_timeout_ms: 0,
            gpu_device: String::new(),
            docker_config_json: String::new(),
        };
        let result = client.load_model(&req).await;
        assert!(result.is_err(), "load_model should fail for 500");
    }

    // ── HTTP unload_model tests ──

    #[tokio::test]
    async fn test_unload_model_http() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/unload-model"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock_server)
            .await;

        let conn = make_http_connection(&mock_server.uri());
        let mut client = TamadClient::new(&conn);

        let req = crate::tamad::UnloadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_name: "test-model".to_string(),
        };
        let result = client.unload_model(&req).await;
        assert!(result.is_ok(), "unload_model should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn test_unload_model_http_failure() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/unload-model"))
            .respond_with(ResponseTemplate::new(500).set_body_string("unload failed"))
            .mount(&mock_server)
            .await;

        let conn = make_http_connection(&mock_server.uri());
        let mut client = TamadClient::new(&conn);

        let req = crate::tamad::UnloadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_name: "test-model".to_string(),
        };
        let result = client.unload_model(&req).await;
        assert!(result.is_err(), "unload_model should fail for 500");
    }

    // ── gRPC channel URL translation ──

    #[test]
    fn test_grpc_url_translation() {
        let conn = make_grpc_connection();
        // Verify the connection URL uses grpc:// scheme
        assert!(conn.url.starts_with("grpc://"));
        // The client should translate grpc:// to http:// for tonic
        let translated = conn.url.replace("grpc://", "http://");
        assert!(translated.starts_with("http://"));
        assert_eq!(translated, "http://localhost:50051");
    }

    // ── gRPC auth header ──

    /// A stored token is attached to gRPC requests as a bearer header.
    #[test]
    fn test_authed_sets_bearer_header() {
        let conn = make_grpc_connection();
        let client = TamadClient::new(&conn);
        let request = client.authed(crate::tamad::Empty {});
        let header = request
            .metadata()
            .get("authorization")
            .expect("authorization header must be set")
            .to_str()
            .unwrap();
        assert_eq!(header, "Bearer secret");
    }

    /// No stored token → no authorization header.
    #[test]
    fn test_authed_without_token_has_no_header() {
        let conn = make_http_connection("http://localhost:8080");
        let client = TamadClient::new(&conn);
        let request = client.authed(crate::tamad::Empty {});
        assert!(request.metadata().get("authorization").is_none());
    }

    /// HTTP calls send the stored token as a bearer header.
    #[tokio::test]
    async fn test_health_check_http_sends_bearer_token() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "ok"
            })))
            .mount(&mock_server)
            .await;

        let conn = TamadConnection {
            id: "test-uuid".to_string(),
            name: "test-tamad".to_string(),
            url: mock_server.uri(),
            protocol: Protocol::Http,
            token: Some("http-secret".to_string()),
            status: crate::providers::TamadStatus::Unknown,
        };
        let mut client = TamadClient::new(&conn);
        assert!(client.health_check().await.unwrap());

        let req = mock_server
            .received_requests()
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(
            req.headers
                .get("authorization")
                .map(|s| s.to_str().unwrap().to_string()),
            Some("Bearer http-secret".to_string())
        );
    }

    // ── Pull-model RPC (plan-191 Task 6) ──

    fn pull_req() -> crate::tamad::PullModelRequest {
        crate::tamad::PullModelRequest {
            repo_id: "org/model".into(),
            quants: vec!["m.gguf".into()],
            model_name: "m".into(),
            backend: "llama_cpp".into(),
            hf_token: "hf_tok".into(),
            repo_pull: false,
            dest_dir: String::new(),
        }
    }

    /// `pull_model` dispatches the request to the tamad (with auth) and
    /// returns the job id.
    #[tokio::test]
    async fn test_pull_model_returns_job_id() {
        let (keep_open, _) = tokio::sync::watch::channel(false);
        let stub = crate::tamad::pool::test_support::StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::new(keep_open),
            calls: Arc::new(AtomicUsize::new(0)),
            successes: Arc::new(AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-42".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_job_calls: Arc::new(AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        };
        let addr = crate::tamad::pool::test_support::start_stub(stub.clone()).await;
        let url = format!("grpc://{addr}");
        let conn = crate::tamad::pool::test_support::grpc_conn("uuid-c", "host-c", &url);
        let mut client = TamadClient::new(&conn);

        let job_id = client.pull_model(&pull_req()).await.unwrap();
        assert_eq!(job_id, "job-42");

        let recorded = stub.pull_requests.lock().await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].repo_id, "org/model");
        assert_eq!(recorded[0].hf_token, "hf_tok");
    }

    /// `pull_model` surfaces an unavailable tamad as an error.
    #[tokio::test]
    async fn test_pull_model_unavailable_tamad_errors() {
        let (keep_open, _) = tokio::sync::watch::channel(false);
        let stub = crate::tamad::pool::test_support::StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::new(keep_open),
            calls: Arc::new(AtomicUsize::new(0)),
            successes: Arc::new(AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-42".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(true)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_job_calls: Arc::new(AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        };
        let addr = crate::tamad::pool::test_support::start_stub(stub).await;
        let conn = crate::tamad::pool::test_support::grpc_conn(
            "uuid-c",
            "host-c",
            &format!("grpc://{addr}"),
        );
        let mut client = TamadClient::new(&conn);

        let err = client.pull_model(&pull_req()).await.unwrap_err();
        assert!(format!("{err:?}").to_lowercase().contains("unavailable"));
    }

    /// `stream_job` opens the job stream and delivers the scripted events;
    /// an unknown job id is rejected.
    #[tokio::test]
    async fn test_stream_job_delivers_events() {
        let (keep_open, _) = tokio::sync::watch::channel(false);
        let events = vec![
            crate::tamad::pool::test_support::job_event("job-42", 50, "downloading", "running"),
            crate::tamad::pool::test_support::terminal_success("job-42", r#"{"verified":true}"#),
        ];
        let stub = crate::tamad::pool::test_support::StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::new(keep_open),
            calls: Arc::new(AtomicUsize::new(0)),
            successes: Arc::new(AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-42".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(events)),
            stream_job_calls: Arc::new(AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        };
        let addr = crate::tamad::pool::test_support::start_stub(stub).await;
        let conn = crate::tamad::pool::test_support::grpc_conn(
            "uuid-c",
            "host-c",
            &format!("grpc://{addr}"),
        );
        let client = TamadClient::new(&conn);

        // Unknown job id → error.
        let err = client.stream_job("nope").await.unwrap_err();
        assert!(
            format!("{err:?}").to_lowercase().contains("notfound"),
            "expected not-found, got: {err:?}"
        );

        // Known job → the two scripted events arrive in order.
        let mut stream = client.stream_job("job-42").await.unwrap();
        let e1 = stream.message().await.unwrap().unwrap();
        assert_eq!(e1.progress, 50);
        assert_eq!(e1.status, "running");
        let e2 = stream.message().await.unwrap().unwrap();
        assert_eq!(e2.status, "succeeded");
        assert!(e2.result_json.contains("verified"));
    }
    // ── Install / update / remove RPCs (plan-191 Task 7) ──

    fn install_req() -> crate::tamad::InstallProviderRequest {
        crate::tamad::InstallProviderRequest {
            name: "llama_cpp".into(),
            engine: "llama_cpp".into(),
            version: "b9123".into(),
            gpu_variant: "cuda".into(),
            force: true,
            git_url: "https://example.com/repo.git".into(),
        }
    }

    fn update_req() -> crate::tamad::UpdateProviderRequest {
        crate::tamad::UpdateProviderRequest {
            name: "llama_cpp".into(),
            version: "b9999".into(),
            engine: "llama_cpp".into(),
            gpu_variant: "cuda".into(),
            git_url: String::new(),
        }
    }

    fn remove_req() -> crate::tamad::RemoveProviderRequest {
        crate::tamad::RemoveProviderRequest {
            name: "llama_cpp".into(),
            engine: "llama_cpp".into(),
            gpu_variant: "cuda".into(),
            version: String::new(),
        }
    }

    /// StubTamad with all job-dispatch fields at defaults.
    fn dispatch_stub() -> crate::tamad::pool::test_support::StubTamad {
        let (keep_open, _) = tokio::sync::watch::channel(false);
        crate::tamad::pool::test_support::StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::new(keep_open),
            calls: Arc::new(AtomicUsize::new(0)),
            successes: Arc::new(AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-pull".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_job_calls: Arc::new(AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        }
    }

    /// `install_provider` dispatches with auth and returns the tamad's job
    /// id; the request is recorded verbatim.
    #[tokio::test]
    async fn test_install_provider_client() {
        let stub = dispatch_stub();
        let addr = crate::tamad::pool::test_support::start_stub(stub.clone()).await;
        let conn = crate::tamad::pool::test_support::grpc_conn(
            "uuid-i",
            "host-i",
            &format!("grpc://{addr}"),
        );
        let mut client = TamadClient::new(&conn);

        let job_id = client.install_provider(&install_req()).await.unwrap();
        assert_eq!(job_id, "job-install");

        let reqs = stub.install_requests.lock().await;
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].engine, "llama_cpp");
        assert_eq!(reqs[0].version, "b9123");
        assert_eq!(reqs[0].gpu_variant, "cuda");
        assert!(reqs[0].force);
        assert_eq!(reqs[0].git_url, "https://example.com/repo.git");
    }

    /// `update_provider` dispatches and returns the tamad's job id.
    #[tokio::test]
    async fn test_update_provider_client() {
        let stub = dispatch_stub();
        let addr = crate::tamad::pool::test_support::start_stub(stub.clone()).await;
        let conn = crate::tamad::pool::test_support::grpc_conn(
            "uuid-u",
            "host-u",
            &format!("grpc://{addr}"),
        );
        let mut client = TamadClient::new(&conn);

        let job_id = client.update_provider(&update_req()).await.unwrap();
        assert_eq!(job_id, "job-update");
        let reqs = stub.update_requests.lock().await;
        assert_eq!(reqs[0].version, "b9999");
        assert!(reqs[0].git_url.is_empty());
    }

    /// `remove_provider` dispatches and returns `()` on success.
    #[tokio::test]
    async fn test_remove_provider_client() {
        let stub = dispatch_stub();
        let addr = crate::tamad::pool::test_support::start_stub(stub.clone()).await;
        let conn = crate::tamad::pool::test_support::grpc_conn(
            "uuid-r",
            "host-r",
            &format!("grpc://{addr}"),
        );
        let mut client = TamadClient::new(&conn);

        client.remove_provider(&remove_req()).await.unwrap();
        let reqs = stub.remove_requests.lock().await;
        assert_eq!(reqs[0].gpu_variant, "cuda");
        assert!(reqs[0].version.is_empty());
    }

    // ── RunBenchmark RPC (plan-191 Task 8) ──

    /// `run_benchmark` dispatches the request (with auth) and returns the
    /// stub's job id; the tamad-relative paths are recorded verbatim.
    #[tokio::test]
    async fn test_run_benchmark_client() {
        let stub = dispatch_stub();
        let addr = crate::tamad::pool::test_support::start_stub(stub.clone()).await;
        let conn = crate::tamad::pool::test_support::grpc_conn(
            "uuid-b",
            "host-b",
            &format!("grpc://{addr}"),
        );
        let mut client = TamadClient::new(&conn);

        let req = crate::tamad::RunBenchmarkRequest {
            model_name: "Test Model".into(),
            kind: "llama_bench".into(),
            config_json: "{}".into(),
            model_path_rel: "org/m/m-Q4_K_M.gguf".into(),
            binary_path_rel: "llama_cpp/cpu/b1/llama-server".into(),
        };
        let job_id = client.run_benchmark(&req).await.unwrap();
        assert_eq!(job_id, "job-bench-1");

        let reqs = stub.bench_requests.lock().await;
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].model_name, "Test Model");
        assert_eq!(reqs[0].model_path_rel, "org/m/m-Q4_K_M.gguf");
        assert_eq!(reqs[0].binary_path_rel, "llama_cpp/cpu/b1/llama-server");
    }

    /// Offline tamad: benchmark dispatch surfaces an error (no job id).
    #[tokio::test]
    async fn test_run_benchmark_offline_tamad_errors() {
        let stub = dispatch_stub();
        *stub.bench_dispatch_fail.lock().await = true;
        let addr = crate::tamad::pool::test_support::start_stub(stub.clone()).await;
        let conn = crate::tamad::pool::test_support::grpc_conn(
            "uuid-fo",
            "host-fo",
            &format!("grpc://{addr}"),
        );
        let mut client = TamadClient::new(&conn);

        let req = crate::tamad::RunBenchmarkRequest {
            model_name: "m".into(),
            kind: "llama_bench".into(),
            config_json: "{}".into(),
            model_path_rel: "org/m/m.gguf".into(),
            binary_path_rel: "llama_cpp/cpu/b1/llama-server".into(),
        };

        let err = client.run_benchmark(&req).await.unwrap_err();
        assert!(
            format!("{err:?}").to_lowercase().contains("unavailable"),
            "got: {err:?}"
        );
    }

    /// Offline tamad: install dispatch surfaces an error (no job id).
    #[tokio::test]
    async fn test_install_provider_offline_tamad_errors() {
        let stub = dispatch_stub();
        *stub.install_dispatch_fail.lock().await = true;
        let addr = crate::tamad::pool::test_support::start_stub(stub.clone()).await;
        let conn = crate::tamad::pool::test_support::grpc_conn(
            "uuid-fo",
            "host-fo",
            &format!("grpc://{addr}"),
        );
        let mut client = TamadClient::new(&conn);

        let err = client.install_provider(&install_req()).await.unwrap_err();
        assert!(
            format!("{err:?}").to_lowercase().contains("unavailable"),
            "got: {err:?}"
        );
    }
}
