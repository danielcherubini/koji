//! Self-registration: tamad announces itself to the proxy at startup
//! and periodically, via an idempotent upsert keyed by name.
//!
//! If the proxy is unreachable the daemon must never fail to serve —
//! errors are logged and retried on the next interval tick.

use anyhow::{anyhow, Result};
use tracing::debug;
use tracing::warn;

/// Registration interval: 5 minutes.
const REGISTER_INTERVAL_SECS: u64 = 300;

/// Registers this tamad with the proxy's management API.
pub struct Registrar {
    client: reqwest::Client,
    /// Proxy base URL (from `TAMA_URL`).
    url: String,
    /// Proxy management token (from `TAMA_TOKEN`).
    token: String,
    /// Name of this tamad.
    name: String,
    /// URL the proxy should use to reach this tamad.
    public_url: String,
    /// Transport protocol: "grpc" or "http".
    protocol: String,
    /// This tamad's own bearer token, stored by the proxy for inbound auth.
    tamad_token: String,
}

impl Registrar {
    /// Create a registrar for the given proxy endpoint and tamad identity.
    pub fn new(
        url: String,
        token: String,
        name: String,
        public_url: String,
        protocol: String,
        tamad_token: String,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
            token,
            name,
            public_url,
            protocol,
            tamad_token,
        }
    }

    /// Attempt a single idempotent registration (upsert by name).
    ///
    /// Success is a 200 (already registered, updated) or 201 (created).
    pub(crate) async fn register_once(&self) -> Result<()> {
        let body = serde_json::json!({
            "name": self.name,
            "url": self.public_url,
            "protocol": self.protocol,
            "token": self.tamad_token,
        });
        let resp = self
            .client
            .post(format!("{}/tama/v1/tamads", self.url.trim_end_matches('/')))
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() {
            if status == reqwest::StatusCode::CREATED {
                debug!(name = %self.name, "Self-registered with proxy (created)");
            } else {
                debug!(name = %self.name, "Self-registration refreshed with proxy");
            }
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(anyhow!(
                "proxy registration failed with status {}: {}",
                status,
                text
            ))
        }
    }

    /// Register immediately, then every 300 seconds forever.
    ///
    /// Errors are logged and the loop continues — the tamad must never
    /// fail to serve because the proxy is down.
    pub async fn run_loop(self) {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(REGISTER_INTERVAL_SECS));
        loop {
            if let Err(e) = self.register_once().await {
                warn!(error = %e, "Self-registration attempt failed; will retry");
            }
            interval.tick().await;
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_registrar(url: &str) -> Registrar {
        Registrar::new(
            url.to_string(),
            "proxy-token".to_string(),
            "my-tamad".to_string(),
            "grpc://my-tamad:50051".to_string(),
            "grpc".to_string(),
            "tamad-token".to_string(),
        )
    }

    /// register_once posts the full identity with the proxy bearer token.
    #[tokio::test]
    async fn test_register_once_sends_body_and_auth() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/tama/v1/tamads"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&mock)
            .await;

        make_registrar(&mock.uri())
            .register_once()
            .await
            .expect("registration should succeed on 201");

        let req = mock
            .received_requests()
            .await
            .unwrap()
            .pop()
            .expect("proxy should receive one request");
        assert_eq!(req.method, "POST");
        assert_eq!(req.url.path(), "/tama/v1/tamads");
        assert_eq!(
            req.headers
                .get("authorization")
                .map(|s| s.to_str().unwrap().to_string()),
            Some("Bearer proxy-token".to_string()),
            "proxy management token must be sent as bearer auth"
        );

        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["name"], "my-tamad");
        assert_eq!(body["url"], "grpc://my-tamad:50051");
        assert_eq!(body["protocol"], "grpc");
        assert_eq!(body["token"], "tamad-token");
    }

    /// 200 (already registered) is also success.
    #[tokio::test]
    async fn test_register_once_accepts_200() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/tama/v1/tamads"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        make_registrar(&mock.uri())
            .register_once()
            .await
            .expect("registration should succeed on 200");
    }

    /// A 500 from the proxy is an error, not a panic — run_loop keeps going.
    #[tokio::test]
    async fn test_register_once_survives_500() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/tama/v1/tamads"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&mock)
            .await;

        let err = make_registrar(&mock.uri())
            .register_once()
            .await
            .expect_err("registration should fail on 500");
        assert!(
            err.to_string().contains("500"),
            "error should mention the status: {}",
            err
        );
    }
}
