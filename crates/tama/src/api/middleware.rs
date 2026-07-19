use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

/// Determine whether the Secure cookie flag should be set.
/// Only sets Secure when we can confirm HTTPS — either via X-Forwarded-Proto header
/// (when behind a TLS-terminating proxy) or when the host is explicitly known to use HTTPS.
/// Defaults to false for all other cases: setting Secure without confirming HTTPS causes
/// the browser to silently drop the cookie on HTTP connections, breaking CSRF protection.
fn should_set_secure(headers: &axum::http::HeaderMap) -> bool {
    // Check X-Forwarded-Proto first (set by reverse proxies like nginx, Caddy)
    if let Some(forwarded_proto) = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
    {
        return forwarded_proto.starts_with("https");
    }

    // No proxy header — we can't confirm HTTPS, so don't set Secure.
    // This avoids the common issue where non-localhost hosts get a Secure cookie
    // that browsers refuse to send over plain HTTP.
    false
}

/// CSRF token cookie name.
const CSRF_COOKIE_NAME: &str = "tama_csrf_token";
/// CSRF token header name expected on state-changing requests.
const CSRF_HEADER_NAME: &str = "X-CSRF-Token";

/// Generate a cryptographically random CSRF token (32 bytes, hex-encoded).
fn generate_csrf_token() -> String {
    // Use uuid v4 for randomness; encode as hex string (fixed 32 chars)
    let id = uuid::Uuid::new_v4();
    let (hi, lo) = id.as_u64_pair();
    format!("{:016x}{:016x}", hi, lo)
}

/// Enforce same-origin for state-changing methods (POST, DELETE, etc.).
///
/// - GET/HEAD/OPTIONS: generate and set CSRF token (cookie + header)
/// - POST/PUT/PATCH: verify CSRF double-submit (cookie matches header)
/// - DELETE: check Origin header matches Host header (legacy fallback)
pub async fn enforce_same_origin(
    req: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let method = req.method().clone();

    // Safe methods: generate CSRF token and set cookie + header
    if matches!(
        method,
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    ) {
        let token = generate_csrf_token();

        // Determine if Secure flag should be set (only when HTTPS confirmed via X-Forwarded-Proto)
        let is_secure = should_set_secure(req.headers());

        // Build cookie string — NO HttpOnly so JS can read it for CSRF double-submit.
        // Secure flag is conditional: only set on non-localhost hosts (HTTPS).
        let secure_attr = if is_secure { "; Secure" } else { "" };
        let set_cookie = format!(
            "{}={}; Path=/; SameSite=Lax{}",
            CSRF_COOKIE_NAME, token, secure_attr
        );

        let mut response = next.run(req).await;
        response
            .headers_mut()
            .insert(axum::http::header::SET_COOKIE, set_cookie.parse().unwrap());
        response
            .headers_mut()
            .insert(CSRF_HEADER_NAME, token.parse().unwrap());
        return Ok(response);
    }

    // POST/PUT/PATCH: verify CSRF double-submit (cookie + header must match)
    if matches!(
        method,
        axum::http::Method::POST | axum::http::Method::PUT | axum::http::Method::PATCH
    ) {
        let cookie_header = req
            .headers()
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Extract CSRF token from cookie
        let cookie_token = extract_csrf_cookie(cookie_header);

        // Get CSRF token from header
        let header_token = req
            .headers()
            .get(CSRF_HEADER_NAME)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        match (cookie_token, header_token) {
            // Both present and matching — full double-submit verification
            (Some(cookie_val), Some(header_val)) if cookie_val == header_val => {
                Ok(next.run(req).await)
            }
            // Neither present — allow through (e.g. for API calls using Bearer tokens).
            (None, None) => Ok(next.run(req).await),
            // Any other combination (one missing, or both present but mismatching) is rejected.
            _ => Err((StatusCode::FORBIDDEN, "CSRF token validation failed")),
        }
    } else if matches!(method, axum::http::Method::DELETE) {
        // DELETE: check Origin if present (legacy fallback for non-POST methods)
        if let Some(origin) = req.headers().get(axum::http::header::ORIGIN) {
            if let Some(host) = req.headers().get(axum::http::header::HOST) {
                let origin_str = origin.to_str().unwrap_or("");
                let host_str = host.to_str().unwrap_or("");

                let expected_origin = format!("http://{}", host_str);
                let expected_origin_ssl = format!("https://{}", host_str);

                if origin_str != expected_origin && origin_str != expected_origin_ssl {
                    return Err((StatusCode::FORBIDDEN, "Cross-origin requests not allowed"));
                }
            } else {
                return Err((StatusCode::FORBIDDEN, "Cross-origin requests not allowed"));
            }
        }
        Ok(next.run(req).await)
    } else {
        // Other methods pass through
        Ok(next.run(req).await)
    }
}

/// Extract CSRF token value from a Cookie header string.
fn extract_csrf_cookie(cookie_header: &str) -> Option<String> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            if key == CSRF_COOKIE_NAME {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{HeaderMap, Method, Request};
    use axum::middleware;
    use axum::Router;
    use tower::ServiceExt;

    /// Helper: build a request with the given method and optional headers.
    fn build_request(method: Method, headers: HeaderMap) -> Request<Body> {
        let mut req = Request::builder()
            .method(method)
            .uri("/test")
            .body(Body::empty())
            .unwrap();
        *req.headers_mut() = headers;
        req
    }

    /// Helper: create a test app with the CSRF middleware layer.
    fn test_app() -> Router {
        Router::new()
            .route(
                "/test",
                axum::routing::get(|| async { "ok" })
                    .post(|| async { "ok" })
                    .put(|| async { "ok" })
                    .patch(|| async { "ok" })
                    .delete(|| async { "ok" }),
            )
            .layer(middleware::from_fn(enforce_same_origin))
    }

    /// Helper: check if a response has a specific status code.
    async fn get_status(response: axum::http::Response<Body>) -> StatusCode {
        response.status()
    }

    // ---- GET / HEAD / OPTIONS: token generation ----

    #[tokio::test]
    async fn get_generates_csrf_token() {
        let req = build_request(Method::GET, HeaderMap::new());
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();

        let set_cookie = response
            .headers()
            .get(axum::http::header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("GET response should include Set-Cookie header");

        let csrf_header = response
            .headers()
            .get(CSRF_HEADER_NAME)
            .and_then(|v| v.to_str().ok())
            .expect("GET response should include X-CSRF-Token header");

        // Verify cookie name and token match
        assert!(set_cookie.starts_with(CSRF_COOKIE_NAME));
        let cookie_token = set_cookie
            .split(';')
            .next()
            .and_then(|part| part.split_once('='))
            .map(|(_, val)| val)
            .expect("Cookie should contain a token value");

        assert_eq!(cookie_token, csrf_header, "Cookie and header tokens must match");
        assert!(!set_cookie.contains("; Secure"), "Cookie should not be Secure when no HTTPS proxy detected");
    }

    #[tokio::test]
    async fn head_generates_csrf_token() {
        let req = build_request(Method::HEAD, HeaderMap::new());
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();

        assert!(response
            .headers()
            .contains_key(axum::http::header::SET_COOKIE));
        assert!(response.headers().contains_key(CSRF_HEADER_NAME));
    }

    // ---- POST: CSRF double-submit verification ----

    #[tokio::test]
    async fn post_matching_cookie_and_header_passes() {
        let token = generate_csrf_token();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("{}={}", CSRF_COOKIE_NAME, token).parse().unwrap(),
        );
        headers.insert(CSRF_HEADER_NAME, token.parse().unwrap());

        let req = build_request(Method::POST, headers);
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn post_no_cookie_no_header_passes() {
        let req = build_request(Method::POST, HeaderMap::new());
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn post_cookie_only_rejected() {
        let token = generate_csrf_token();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("{}={}", CSRF_COOKIE_NAME, token).parse().unwrap(),
        );

        let req = build_request(Method::POST, headers);
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn post_header_only_rejected() {
        let token = generate_csrf_token();
        let mut headers = HeaderMap::new();
        headers.insert(CSRF_HEADER_NAME, token.parse().unwrap());

        let req = build_request(Method::POST, headers);
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn post_mismatched_cookie_and_header_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("{}=cookie_value", CSRF_COOKIE_NAME)
                .parse()
                .unwrap(),
        );
        headers.insert(CSRF_HEADER_NAME, "different_header_value".parse().unwrap());

        let req = build_request(Method::POST, headers);
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::FORBIDDEN);
    }

    // ---- PUT: CSRF double-submit verification ----

    #[tokio::test]
    async fn put_matching_cookie_and_header_passes() {
        let token = generate_csrf_token();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("{}={}", CSRF_COOKIE_NAME, token).parse().unwrap(),
        );
        headers.insert(CSRF_HEADER_NAME, token.parse().unwrap());

        let req = build_request(Method::PUT, headers);
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn put_no_cookie_no_header_passes() {
        let req = build_request(Method::PUT, HeaderMap::new());
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::OK);
    }

    // ---- PATCH: CSRF double-submit verification ----

    #[tokio::test]
    async fn patch_matching_cookie_and_header_passes() {
        let token = generate_csrf_token();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("{}={}", CSRF_COOKIE_NAME, token).parse().unwrap(),
        );
        headers.insert(CSRF_HEADER_NAME, token.parse().unwrap());

        let req = build_request(Method::PATCH, headers);
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn patch_no_cookie_no_header_passes() {
        let req = build_request(Method::PATCH, HeaderMap::new());
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::OK);
    }

    // ---- DELETE: origin check ----

    #[tokio::test]
    async fn delete_with_matching_origin_passes() {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, "localhost:18910".parse().unwrap());
        headers.insert(
            axum::http::header::ORIGIN,
            "http://localhost:18910".parse().unwrap(),
        );

        let req = build_request(Method::DELETE, headers);
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn delete_without_origin_passes() {
        let req = build_request(Method::DELETE, HeaderMap::new());
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn delete_with_mismatched_origin_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, "example.com".parse().unwrap());
        headers.insert(
            axum::http::header::ORIGIN,
            "http://evil.com".parse().unwrap(),
        );

        let req = build_request(Method::DELETE, headers);
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(get_status(response).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn get_sets_secure_flag_behind_https_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", "https".parse().unwrap());

        let req = build_request(Method::GET, headers);
        let app = test_app();
        let response = app.oneshot(req).await.unwrap();

        let set_cookie = response
            .headers()
            .get(axum::http::header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("Response should have Set-Cookie");

        assert!(set_cookie.contains("; Secure"), "Cookie should be Secure when x-forwarded-proto is https");
    }

    #[test]
    fn test_should_set_secure() {
        let mut headers = HeaderMap::new();
        assert!(!should_set_secure(&headers));

        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(should_set_secure(&headers));

        headers.insert("x-forwarded-proto", "http".parse().unwrap());
        assert!(!should_set_secure(&headers));
    }

    #[test]
    fn test_generate_csrf_token_format() {
        let t1 = generate_csrf_token();
        let t2 = generate_csrf_token();
        assert_ne!(t1, t2);
        assert_eq!(t1.len(), 32);
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_extract_csrf_cookie() {
        assert_eq!(
            extract_csrf_cookie("a=1; tama_csrf_token=xyz; b=2"),
            Some("xyz".to_string())
        );
        assert_eq!(extract_csrf_cookie("other_cookie=123"), None);
    }
}
