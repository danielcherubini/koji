use super::*;
use axum::http::{header::HeaderName, HeaderMap, HeaderValue};

#[test]
fn test_filter_request_headers_strips_dangerous_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("host", HeaderValue::from_static("localhost:8080"));
    headers.insert("connection", HeaderValue::from_static("keep-alive"));
    headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
    headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
    headers.insert("upgrade", HeaderValue::from_static("websocket"));
    headers.insert("proxy-authenticate", HeaderValue::from_static("Basic"));
    headers.insert(
        "proxy-authorization",
        HeaderValue::from_static("Bearer token"),
    );
    headers.insert("te", HeaderValue::from_static("trailers"));
    headers.insert("trailer", HeaderValue::from_static("X-Signature"));

    let filtered = filter_request_headers(&headers);

    assert!(!filtered.contains_key("host"));
    assert!(!filtered.contains_key("connection"));
    assert!(!filtered.contains_key("keep-alive"));
    assert!(!filtered.contains_key("transfer-encoding"));
    assert!(!filtered.contains_key("upgrade"));
    assert!(!filtered.contains_key("proxy-authenticate"));
    assert!(!filtered.contains_key("proxy-authorization"));
    assert!(!filtered.contains_key("te"));
    assert!(!filtered.contains_key("trailer"));
}

#[test]
fn test_filter_request_headers_passes_safe_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", HeaderValue::from_static("Mozilla/5.0"));
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
    headers.insert("accept", HeaderValue::from_static("text/event-stream"));

    let filtered = filter_request_headers(&headers);

    assert_eq!(filtered.get("user-agent").unwrap(), "Mozilla/5.0");
    assert_eq!(filtered.get("content-type").unwrap(), "application/json");
    assert_eq!(filtered.get("authorization").unwrap(), "Bearer secret");
    assert_eq!(filtered.get("accept").unwrap(), "text/event-stream");
}

#[test]
fn test_filter_request_headers_skips_invalid_utf8() {
    let mut headers = HeaderMap::new();
    // Insert a header with invalid UTF-8 value — should be skipped
    headers.insert(
        HeaderName::from_static("x-custom"),
        HeaderValue::from_bytes(b"\xff\xfe").unwrap(),
    );
    headers.insert("content-type", HeaderValue::from_static("text/plain"));

    let filtered = filter_request_headers(&headers);

    // Invalid UTF-8 header should be skipped, valid one should pass
    assert!(!filtered.contains_key("x-custom"));
    assert_eq!(filtered.get("content-type").unwrap(), "text/plain");
}

#[test]
fn test_filter_request_headers_empty_input() {
    let headers = HeaderMap::new();
    let filtered = filter_request_headers(&headers);
    assert!(filtered.is_empty());
}

#[test]
fn test_strip_response_headers_removes_hop_by_hop() {
    let mut headers = HeaderMap::new();
    headers.insert("connection", HeaderValue::from_static("keep-alive"));
    headers.insert("content-length", HeaderValue::from_static("1234"));
    headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
    headers.insert("x-custom", HeaderValue::from_static("value"));

    let stripped = strip_response_headers(&headers);

    let keys: Vec<&str> = stripped.iter().map(|(k, _)| k.as_str()).collect();
    assert!(!keys.contains(&"connection"));
    assert!(!keys.contains(&"content-length"));
    assert!(!keys.contains(&"transfer-encoding"));
    assert!(keys.contains(&"x-custom"));
    assert_eq!(
        stripped.iter().find(|(k, _)| k == "x-custom").unwrap().1,
        "value"
    );
}

#[test]
fn test_strip_response_headers_passes_safe_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("x-request-id", HeaderValue::from_static("abc123"));

    let stripped = strip_response_headers(&headers);

    assert_eq!(stripped.len(), 2);
    assert!(stripped
        .iter()
        .any(|(k, v)| k == "content-type" && v == "application/json"));
    assert!(stripped
        .iter()
        .any(|(k, v)| k == "x-request-id" && v == "abc123"));
}

#[test]
fn test_strip_response_headers_empty_input() {
    let headers = HeaderMap::new();
    let stripped = strip_response_headers(&headers);
    assert!(stripped.is_empty());
}
