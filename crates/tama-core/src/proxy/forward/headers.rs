use axum::http::HeaderMap;

/// Hop-by-hop headers that should be stripped from forwarded requests.
const REQUEST_SKIP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "transfer-encoding",
    "upgrade",
    "trailer",
    "host",
];

/// Hop-by-hop headers (plus content-length) that should be stripped from forwarded responses.
const RESPONSE_SKIP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "transfer-encoding",
    "upgrade",
    "trailer",
    "content-length",
];

/// Filter request headers, removing hop-by-hop headers.
pub fn filter_request_headers(headers: &HeaderMap) -> HeaderMap {
    let mut filtered = HeaderMap::new();
    for (key, value) in headers {
        if !REQUEST_SKIP_HEADERS.contains(&key.as_str()) && value.to_str().is_ok() {
            filtered.insert(key.clone(), value.clone());
        }
    }
    filtered
}

/// Strip hop-by-hop and content-length headers from a response.
pub fn strip_response_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for (key, value) in headers {
        if RESPONSE_SKIP_HEADERS.contains(&key.as_str()) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            result.push((key.as_str().to_string(), v.to_string()));
        }
    }
    result
}
