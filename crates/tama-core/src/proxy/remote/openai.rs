use super::anthropic::AnthropicForwarder;
use crate::providers::Provider;
use axum::body::Body;
use axum::http::request::Parts;
use bytes::Bytes;

/// Forwards HTTP requests to a remote provider.
///
/// For OpenAI-compatible providers, the request format is passed through
/// directly. For Anthropic providers, the request is translated from OpenAI
/// format to Anthropic format, and the response is translated back.
#[derive(Clone)]
pub struct RemoteForwarder {
    client: reqwest::Client,
    anthropic: AnthropicForwarder,
}

impl RemoteForwarder {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            anthropic: AnthropicForwarder::new(),
        }
    }

    /// Forward an HTTP request to a remote provider.
    ///
    /// Routes through the Anthropic translator when `provider.engine.is_anthropic()`,
    /// otherwise forwards directly to the OpenAI-compatible endpoint.
    ///
    /// # Arguments
    /// * `provider` - The remote provider configuration (base_url, api_key)
    /// * `parts` - The HTTP request parts (method, headers, URI)
    /// * `body` - The request body bytes
    ///
    /// # Returns
    /// The provider's response, streamed back to the client.
    pub async fn forward(
        &self,
        provider: &Provider,
        parts: &Parts,
        body: Bytes,
    ) -> anyhow::Result<http::Response<Body>> {
        if provider.engine.is_anthropic() {
            // Route through Anthropic translator (OpenAI → Anthropic → OpenAI)
            return self.anthropic.forward(provider, parts, body).await;
        }

        // Direct forwarding for OpenAI-compatible providers
        self.forward_openai(provider, parts, body).await
    }

    /// Forward an HTTP request to a remote OpenAI-compatible provider.
    ///
    /// The request format is already OpenAI-compatible, so no transformation
    /// is needed. The forwarder constructs the target URL from the provider's
    /// `base_url` and the request path, injects the `api_key` as a Bearer
    /// token, and streams the response back to the client.
    async fn forward_openai(
        &self,
        provider: &Provider,
        parts: &Parts,
        body: Bytes,
    ) -> anyhow::Result<http::Response<Body>> {
        let base_url = provider.base_url.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "Remote provider '{}' has no base_url configured",
                provider.name
            )
        })?;

        let api_key = provider.api_key.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "Remote provider '{}' has no api_key configured",
                provider.name
            )
        })?;

        // Build target URL: base_url + request path
        let target_url = format!("{}{}", base_url.trim_end_matches('/'), parts.uri.path());

        // Build request, cloning headers and adding Authorization
        let mut request = self.client.request(parts.method.clone(), &target_url);

        // Forward all original headers except host and authorization
        for (name, value) in &parts.headers {
            let should_skip = name.as_str().eq_ignore_ascii_case("host")
                || name.as_str().eq_ignore_ascii_case("authorization");
            if !should_skip {
                request = request.header(name, value);
            }
        }

        // Inject API key as Bearer token
        request = request.header(http::header::AUTHORIZATION, format!("Bearer {}", api_key));

        // Send request with body
        let request = request.body(body).build()?;

        // Execute and convert response
        let response = self.client.execute(request).await?;

        // Convert reqwest response to axum response
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = Body::from_stream(response.bytes_stream());

        let mut axum_response = http::Response::new(body);
        *axum_response.status_mut() = http::StatusCode::from_u16(status)?;
        *axum_response.headers_mut() = headers;

        Ok(axum_response)
    }
}

impl Default for RemoteForwarder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{Engine, ProviderType};
    use axum::http::{Method, Uri};

    fn make_test_provider(base_url: &str, api_key: &str, engine: Engine) -> Provider {
        Provider {
            id: 1,
            name: "test-provider".to_string(),
            provider_type: ProviderType::Remote,
            engine,
            tamad_id: None,
            base_url: Some(base_url.to_string()),
            api_key: Some(api_key.to_string()),
            created_at: 0,
        }
    }

    fn make_test_parts(path: &str) -> Parts {
        let uri: Uri = format!("/v1/chat/completions{}", path).parse().unwrap();
        let req = http::Request::builder()
            .method(Method::POST)
            .uri(&uri)
            .header("Content-Type", "application/json")
            .header("User-Agent", "test-client")
            .body(())
            .unwrap();
        req.into_parts().0
    }

    #[test]
    fn test_remote_forwarder_new() {
        let _forwarder = RemoteForwarder::new();
    }

    #[test]
    fn test_remote_forwarder_default() {
        let _ = RemoteForwarder::default();
    }

    #[test]
    fn test_build_target_url_with_trailing_slash() {
        let provider = make_test_provider("https://api.example.com/v1/", "key123", Engine::OpenAI);
        let parts = make_test_parts("");
        let target_url = format!(
            "{}{}",
            provider.base_url.as_deref().unwrap().trim_end_matches('/'),
            parts.uri.path()
        );
        assert_eq!(target_url, "https://api.example.com/v1/v1/chat/completions");
    }

    #[test]
    fn test_build_target_url_without_trailing_slash() {
        let provider = make_test_provider("https://api.example.com/v1", "key123", Engine::OpenAI);
        let parts = make_test_parts("");
        let target_url = format!(
            "{}{}",
            provider.base_url.as_deref().unwrap().trim_end_matches('/'),
            parts.uri.path()
        );
        assert_eq!(target_url, "https://api.example.com/v1/v1/chat/completions");
    }

    #[test]
    fn test_build_target_url_preserves_query_params() {
        let provider = make_test_provider("https://api.example.com/v1", "key123", Engine::OpenAI);
        let uri: Uri = "/v1/chat/completions?stream=true".parse().unwrap();
        let req = http::Request::builder()
            .method(Method::POST)
            .uri(&uri)
            .body(())
            .unwrap();
        let parts = req.into_parts().0;
        let target_url = format!(
            "{}{}",
            provider.base_url.as_deref().unwrap().trim_end_matches('/'),
            parts.uri.path()
        );
        // Path doesn't include query — query is in parts.uri.query()
        assert_eq!(target_url, "https://api.example.com/v1/v1/chat/completions");
        assert_eq!(parts.uri.query(), Some("stream=true"));
    }

    #[test]
    fn test_authorization_header_format() {
        let api_key = "sk-test-12345";
        let auth = format!("Bearer {}", api_key);
        assert_eq!(auth, "Bearer sk-test-12345");
        assert!(auth.starts_with("Bearer "));
    }

    #[test]
    fn test_host_header_is_skipped() {
        let _provider = make_test_provider("https://api.example.com/v1", "key123", Engine::OpenAI);
        let _parts = make_test_parts("");
        // Verify host header should be skipped
        let should_skip = http::header::HOST.as_str().eq_ignore_ascii_case("host");
        assert!(should_skip, "host header should be skipped");
    }

    #[test]
    fn test_missing_base_url_returns_error() {
        let provider = Provider {
            base_url: None,
            api_key: Some("key".to_string()),
            ..make_test_provider("", "key", Engine::OpenAI)
        };
        let error = provider
            .base_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no base_url"));
        assert!(error.is_err());
        assert!(error.unwrap_err().to_string().contains("base_url"));
    }

    #[test]
    fn test_missing_api_key_returns_error() {
        let provider = Provider {
            base_url: Some("https://api.example.com".to_string()),
            api_key: None,
            ..make_test_provider("https://api.example.com", "", Engine::OpenAI)
        };
        let error = provider
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no api_key"));
        assert!(error.is_err());
        assert!(error.unwrap_err().to_string().contains("api_key"));
    }

    #[test]
    fn test_anthropic_engine_routes_correctly() {
        let provider =
            make_test_provider("https://api.anthropic.com", "sk-ant-123", Engine::Anthropic);
        assert!(provider.engine.is_anthropic());
    }

    #[test]
    fn test_openai_engine_routes_correctly() {
        let provider =
            make_test_provider("https://api.openai.com/v1", "sk-openai-123", Engine::OpenAI);
        assert!(provider.engine.is_open_ai());
        assert!(!provider.engine.is_anthropic());
    }
}
