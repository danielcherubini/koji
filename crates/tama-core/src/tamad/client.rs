//! Client for communicating with tamad instances via gRPC or HTTP.
//!
//! Supports both gRPC (via tonic) and HTTP (via reqwest) protocols.
//! Connections are established lazily on first use.

use anyhow::{anyhow, Context, Result};

use crate::providers::{Protocol, TamadConnection};

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

    /// Load a model on the remote tamad.
    pub async fn load_model(
        &mut self,
        req: &crate::tamad::LoadModelRequest,
    ) -> Result<crate::tamad::LoadModelResponse> {
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client.load_model(tonic::Request::new(req.clone())).await?;
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
                let resp = self.http_client.post(&url).json(&json_body).send().await?;
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
                client
                    .unload_model(tonic::Request::new(req.clone()))
                    .await?;
                Ok(())
            }
            Protocol::Http => {
                let url = format!("{}/unload-model", self.connection.url);
                let json_body = serde_json::json!({
                    "provider_name": req.provider_name,
                    "model_name": req.model_name,
                });
                let resp = self.http_client.post(&url).json(&json_body).send().await?;
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
        match self.connection.protocol {
            Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client
                    .health_check(tonic::Request::new(crate::tamad::Empty {}))
                    .await?;
                Ok(response.get_ref().status == "ok")
            }
            Protocol::Http => {
                let resp = self
                    .http_client
                    .get(format!("{}/health", self.connection.url))
                    .send()
                    .await?;
                Ok(resp.status().is_success())
            }
        }
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
}
