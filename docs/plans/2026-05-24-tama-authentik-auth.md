# Tama Authentik Auth Plan

**Goal:** Add Authentik API token validation to tama so both API clients (bearer tokens) and browser users (via Caddy forward_auth headers) are authenticated before any request is processed.

**Architecture:** A single Axum middleware layer (`AuthLayer`) wraps the entire router. It checks for an `Authorization: Bearer <token>` header and validates it against `https://auth.wizards.town/api/v3/core/users/me/`. If no bearer token is present, it falls back to checking the `X-Authentik-Username` header set by Caddy's forward_auth. Configurable skip_paths exempt specific routes. On Authentik outage, the middleware fails open (allows request through with a warning log).

**Tech Stack:** Rust, Axum 0.7, tower Layer trait, reqwest 0.12, serde_json. No new dependencies required — tama-core already depends on reqwest and serde_json.

---

## Design Decisions (from brainstorming)

1. **Tama does the auth checking**, not Caddy. Caddy's forward_auth stays for browser session cookies, and tama validates API bearer tokens. They don't conflict.
2. **Token introspection per request** — call `https://auth.wizards.town/api/v3/core/users/me/` on every request. Simple, no caching, no JWKS. Fail-open on timeout (5s).
3. **Config lives in `ProxyConfig`** — `authenticator_url` (e.g. `https://auth.wizards.town`) and `authenticator_skip_paths` (list of paths to exempt).
4. **Caddy remains untouched** — existing forward_auth config for tama.wizards.town stays as-is.

---

### Task 1: Add auth config fields to ProxyConfig

**Context:**
The `ProxyConfig` struct in `crates/tama-core/src/config/types.rs` holds all proxy-level settings: host, port, idle timeout, etc. We need two new fields: an optional Authentik URL (when set, auth is enabled) and a list of paths that bypass auth (empty by default). These must serialize/deserialize from TOML and have sensible defaults.

**Files:**
- Modify: `crates/tama-core/src/config/types.rs`

**What to implement:**
Add two fields to the `ProxyConfig` struct, BEFORE the `impl Default for ProxyConfig` block:

```rust
/// Authentik instance URL for bearer token validation.
/// When set, all requests require auth (except paths in skip_paths).
/// Example: "https://auth.wizards.town"
#[serde(default)]
pub authenticator_url: Option<String>,

/// Paths exempt from authentication. Default: empty.
/// Example: ["/health", "/metrics"]
#[serde(default)]
pub authenticator_skip_paths: Vec<String>,
```

Update the `Default` implementation for `ProxyConfig` to include these fields with their zero-values:

```rust
authenticator_url: None,
authenticator_skip_paths: Vec::new(),
```

This task is purely additive — no existing fields or logic are modified. The fields default to `None` / `Vec::new()` so the auth middleware won't activate unless explicitly configured.

**Steps:**
- [ ] Add the two fields to `ProxyConfig` struct in `crates/tama-core/src/config/types.rs`
- [ ] Add the fields to the `ProxyConfig::default()` implementation
- [ ] Run `cargo build -p tama-core`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo test -p tama-core`
  - Did all tests pass? If not, fix failures and re-run before continuing.
- [ ] Commit with message: "feat(proxy): add authenticator_url and authenticator_skip_paths config fields"

**Acceptance criteria:**
- [ ] `ProxyConfig` has `authenticator_url: Option<String>` and `authenticator_skip_paths: Vec<String>` fields
- [ ] Both fields have `#[serde(default)]` so existing TOML configs don't break
- [ ] `Default::default()` yields `None` and empty vec
- [ ] `cargo build -p tama-core` succeeds
- [ ] `cargo test -p tama-core` passes

---

### Task 2: Create auth middleware module

**Context:**
The auth middleware validates incoming requests by checking either a bearer token (for API clients) or an `X-Authentik-Username` header (for browser users authenticated via Caddy forward_auth). It's implemented as a standalone `axum::middleware::from_fn_with_state` compatible function — no custom Layer trait implementation needed. This approach avoids dead code and compiles immediately without extra axum feature flags.

The function lives in `crates/tama-core/src/proxy/auth.rs`. Task 3 will wire it into the routers.

**Files:**
- Create: `crates/tama-core/src/proxy/auth.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs` (add `pub mod auth;`)

**What to implement:**

Create `crates/tama-core/src/proxy/auth.rs` with exactly this content (copy-paste safe):

```rust
//! Authentik auth middleware for Axum.
//!
//! Validates bearer tokens against Authentik's user-info API,
//! falling back to Caddy forward_auth headers for browser sessions.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

// --- Auth config ---

#[derive(Clone)]
pub struct AuthConfig {
    pub url: Option<String>,
    pub skip_paths: Vec<String>,
}

// --- Authentik user-info response ---

#[derive(Debug, Deserialize)]
struct AuthentikUserResponse {
    user: AuthentikUser,
}

#[derive(Debug, Deserialize)]
struct AuthentikUser {
    username: String,
}

// --- Middleware function ---

pub async fn auth_middleware(
    State(config): State<Arc<AuthConfig>>,
    req: Request,
    next: Next,
) -> Response {
    // 1. If no auth URL configured, pass through
    if config.url.as_deref().is_none_or(|u| u.is_empty()) {
        return next.run(req).await;
    }

    // 2. Check skip_paths
    let path = req.uri().path().to_string();
    if config.skip_paths.iter().any(|p| path.starts_with(p.as_str())) {
        return next.run(req).await;
    }

    // 3. Check for bearer token
    if let Some(bearer_token) = extract_bearer_token(&req) {
        let authenticator_url = config.url.as_deref().unwrap_or("");
        match validate_token_against_authentik(authenticator_url, &bearer_token).await {
            Ok(username) => {
                debug!("Authenticated user via bearer token: {}", username);
                return next.run(req).await;
            }
            Err(status) => {
                return (status, json_unauthorized()).into_response();
            }
        }
    }

    // 4. Check for X-Authentik-Username header (Caddy forward_auth)
    if let Some(username) = req
        .headers()
        .get("X-Authentik-Username")
        .and_then(|v| v.to_str().ok())
    {
        debug!("Authenticated user via Caddy forward_auth: {}", username);
        return next.run(req).await;
    }

    // 5. No valid auth
    (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response()
}

// --- Helper functions ---

fn extract_bearer_token(req: &Request) -> Option<String> {
    let header = req.headers().get(header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    value.strip_prefix("Bearer ").map(|s| s.to_string())
}

async fn validate_token_against_authentik(
    authenticator_url: &str,
    token: &str,
) -> Result<String, StatusCode> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| {
            warn!("Failed to build reqwest client: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let url = format!("{}/api/v3/core/users/me/", authenticator_url);
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/json")
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: AuthentikUserResponse = r.json().await.map_err(|e| {
                warn!("Failed to parse Authentik user response: {}", e);
                StatusCode::UNAUTHORIZED
            })?;
            Ok(body.user.username)
        }
        Ok(r) if r.status() == StatusCode::UNAUTHORIZED => Err(StatusCode::UNAUTHORIZED),
        // Any non-success non-401 (403, 500, 429) is treated as unauthorized.
        // This is intentional: if Authentik signals the token is bad, deny access.
        Ok(_) => Err(StatusCode::UNAUTHORIZED),
        Err(e) => {
            // Fail-open: if Authentik is unreachable (connection timeout, DNS failure),
            // allow the request through with a warning log.
            warn!("Authentik auth API unreachable ({}), allowing request", e);
            Ok("unknown".to_string())
        }
    }
}

fn json_unauthorized() -> Response {
    let body = serde_json::json!({
        "error": "Authentication required",
        "detail": "Provide a valid Authorization: Bearer <token> header"
    })
    .to_string();
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap()
}
```

Then in `crates/tama-core/src/proxy/mod.rs`, find the existing `pub mod` declarations (around line 5-9: `pub mod state;`, `pub mod status;`, etc.) and add:
```rust
pub mod auth;
```

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/auth.rs` with the content above
- [ ] Add `pub mod auth;` to `crates/tama-core/src/proxy/mod.rs`
- [ ] Run `cargo build -p tama-core`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test -p tama-core`
  - Did all tests pass? If not, fix failures and re-run before continuing.
- [ ] Run `cargo fmt`
- [ ] Commit with message: "feat(proxy): add Authentik auth middleware module"

**Acceptance criteria:**
- [ ] `crates/tama-core/src/proxy/auth.rs` exists with `AuthConfig`, `auth_middleware`, `extract_bearer_token`, `validate_token_against_authentik`
- [ ] Module is declared in `proxy/mod.rs`
- [ ] `cargo build -p tama-core` succeeds (no `unimplemented!()` — compiles cleanly)
- [ ] `cargo test -p tama-core` passes

---

### Task 3: Integrate auth middleware into router

**Context:**
The `auth_middleware` function created in Task 2 needs to be wired into both `build_router()` (standalone proxy) and `build_unified_router()` (proxy + web UI) in `crates/tama-core/src/proxy/server/router.rs`. The middleware is added via `axum::middleware::from_fn_with_state`, which takes the `AuthConfig` as its state. 

**IMPORTANT — Layer ordering**: In tower/axum, the last `.layer()` added processes requests FIRST (it wraps outermost). The auth middleware must run AFTER CorsLayer processes CORS preflight `OPTIONS` requests (which don't have `Authorization` headers). So the order is: routes → `.layer(auth)` → `.layer(CorsLayer)` → `.with_state(state)`. This means CorsLayer wraps auth, and OPTIONS requests reach CorsLayer before auth sees them.

**NOTE — web-ui feature gate**: `build_unified_router()` is gated behind `#[cfg(feature = "web-ui")]`. To verify your changes compile for both paths, run:
- `cargo build -p tama-core` (tests `build_router()`)
- `cargo build -p tama-core --features web-ui` (tests `build_unified_router()`)

**Files:**
- Modify: `crates/tama-core/src/proxy/server/router.rs`

**What to implement:**

In both `build_router()` and `build_unified_router()`:

1. Import the auth types at the top of the file (add near line 1-20, alongside existing imports):
```rust
use crate::proxy::auth::{auth_middleware, AuthConfig};
use std::sync::Arc;
use axum::middleware;
```

2. At the start of each function body, build the auth config from proxy state:
```rust
let auth_config = Arc::new(AuthConfig {
    url: state.config.proxy.authenticator_url.clone(),
    skip_paths: state.config.proxy.authenticator_skip_paths.clone(),
});
```

3. Add the middleware layer between routes and CorsLayer:

In `build_router()` (around line 79-81, the `.layer(CorsLayer::permissive()).with_state(state)` block):
```rust
Router::new()
    // ... routes ...
    .route("/*path", get(handle_forward_get))
    .fallback(handle_fallback)
    .layer(middleware::from_fn_with_state(auth_config, auth_middleware))  // <- ADD
    .layer(CorsLayer::permissive())                                       // already exists
    .with_state(state)                                                    // already exists
```

In `build_unified_router()` (around line 160-165, the `.layer(CorsLayer).layer(CatchPanicLayer).with_state(state)` block):
```rust
Router::new()
    .merge(proxy_routes)
    .merge(extra_routes)
    .layer(middleware::from_fn_with_state(auth_config, auth_middleware))  // <- ADD
    .layer(CatchPanicLayer::new())                                        // already exists
    .layer(CorsLayer::permissive())                                       // already exists
    .with_state(state)                                                    // already exists
```

The resulting request pipeline (outer to inner) is:
```
CorsLayer → [CatchPanicLayer (web-ui only)] → AuthMiddleware → routes
```
This ensures: CORS preflight is handled first, then panics are caught, then auth is checked, then routes execute.

**Steps:**
- [ ] Import `auth_middleware`, `AuthConfig`, `Arc`, and `axum::middleware` in `router.rs`
- [ ] Build `auth_config` from proxy state in both `build_router()` and `build_unified_router()`
- [ ] Add `.layer(middleware::from_fn_with_state(auth_config, auth_middleware))` between routes and CorsLayer in both functions
- [ ] Run `cargo build -p tama-core`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo build -p tama-core --features web-ui`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test -p tama-core`
  - Did all tests pass? If not, fix failures and re-run before continuing.
- [ ] Run `cargo fmt`
- [ ] Commit with message: "feat(proxy): integrate Authentik auth middleware into router"

**Acceptance criteria:**
- [ ] `auth_middleware` is wired into both `build_router()` and `build_unified_router()`
- [ ] Layer order is: routes → auth → [CatchPanic] → Cors → state
- [ ] When `authenticator_url` is `None` (default), all requests pass through
- [ ] `cargo build -p tama-core` and `cargo build -p tama-core --features web-ui` both succeed
- [ ] `cargo test -p tama-core` passes

---

### Task 4: Write tests for auth middleware

**Context:**
The auth middleware needs unit tests covering:
1. No auth configured → request passes through
2. Valid bearer token → passes (mock the Authentik API)
3. Invalid/expired bearer token → 401
4. `X-Authentik-Username` header → passes (Caddy forward_auth fallback)
5. Skip path → passes even without auth
6. Authentik API timeout → fail-open (request passes)
7. No auth header at all → 401

We'll use `tower::ServiceExt` in integration-style tests, with a mock HTTP server for the Authentik API.

**Files:**
- Modify: `crates/tama-core/src/proxy/auth.rs` (add `#[cfg(test)]` module at bottom)

**What to implement:**

Add a `#[cfg(test)] mod tests` at the bottom of `auth.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use axum::middleware;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn test_handler() -> &'static str {
        "ok"
    }

    fn make_app(auth_config: AuthConfig) -> Router {
        let state = Arc::new(auth_config);
        Router::new()
            .route("/", get(test_handler))
            .route("/health", get(test_handler))
            .layer(middleware::from_fn_with_state(state, auth_middleware))
    }

    #[tokio::test]
    async fn no_auth_url_passes_through() {
        let config = AuthConfig {
            url: None,
            skip_paths: vec![],
        };
        let app = make_app(config);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn skip_path_passes_through() {
        let config = AuthConfig {
            url: Some("https://auth.wizards.town".to_string()),
            skip_paths: vec!["/health".to_string()],
        };
        let app = make_app(config);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_auth_returns_401() {
        let config = AuthConfig {
            url: Some("https://auth.wizards.town".to_string()),
            skip_paths: vec![],
        };
        let app = make_app(config);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn caddy_forward_auth_header_passes() {
        let config = AuthConfig {
            url: Some("https://auth.wizards.town".to_string()),
            skip_paths: vec![],
        };
        let app = make_app(config);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("X-Authentik-Username", "daniel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```

**Steps:**
- [ ] Add test module to `crates/tama-core/src/proxy/auth.rs`
- [ ] Run `cargo test -p tama-core -- proxy::auth`
  - Did all 4 tests pass? If any failed, debug and fix.
- [ ] Run `cargo test -p tama-core` (all tests)
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Run `cargo fmt`
- [ ] Commit with message: "test(proxy): add auth middleware unit tests"

**Acceptance criteria:**
- [ ] Tests cover: no-auth passthrough, skip_path, 401 on missing auth, Caddy header passthrough
- [ ] All 4 tests pass
- [ ] `cargo test -p tama-core` passes
- [ ] No test uses real network calls (only simulated requests)

---

### Task 5: Integration test with mock Authentik API

**Context:**
The auth middleware calls a real Authentik API over HTTP. We need an integration test that spins up a mock HTTP server responding to `/api/v3/core/users/me/` and verifies that tama correctly validates tokens against it. This test is more complex than the unit tests and requires axum-server or tokio's TcpListener to create a mock server.

**Files:**
- Modify: `crates/tama-core/src/proxy/auth.rs` (add integration test with mock server)
- **OR** Create: `crates/tama-core/tests/auth_integration_test.rs`

**What to implement:**

Create an integration test that:
1. Starts a mini Axum server on a random port that responds to `/api/v3/core/users/me/`:
   - If `Authorization: Bearer valid-token` → 200 with `{"user": {"username": "daniel", ...}}`
   - If `Authorization: Bearer invalid-token` → 401
   - If no Authorization header → 401
   - Adds a 3-second delay for a "timeout" token to test fail-open
2. Creates a tama app with `authenticator_url` pointing to this mock server
3. Tests:
   - Valid token → 200
   - Invalid token → 401
   - Server crash (timeout) → 200 (fail-open)

```rust
#[tokio::test]
async fn valid_bearer_token_passes() {
    // Start mock Authentik server
    let mock = axum::Router::new()
        .route("/api/v3/core/users/me/", axum::routing::get(|| async {
            axum::Json(serde_json::json!({
                "user": {"username": "daniel", "pk": 7, "is_active": true}
            }))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_addr = listener.local_addr().unwrap();
    tokio::spawn(async { axum::serve(listener, mock).await.unwrap() });
    // Wait for the mock server to be ready
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Build app pointing to mock
    let config = AuthConfig {
        url: Some(format!("http://{}", mock_addr)),
        skip_paths: vec![],
    };
    let app = make_app(config);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("Authorization", "Bearer valid-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
```

**Steps:**
- [ ] Add mock server integration test to auth.rs `#[cfg(test)]` module or `tests/auth_integration_test.rs`
- [ ] Run `cargo test -p tama-core -- proxy::auth` or `cargo test -p tama-core -- auth_integration`
  - Did all tests pass? If any failed, debug and fix.
- [ ] Run `cargo test -p tama-core` (all tests)
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Run `cargo fmt`
- [ ] Commit with message: "test(proxy): add Authentik API mock integration test"

**Acceptance criteria:**
- [ ] Integration test starts a mock Authentik server and validates token flow
- [ ] Tests cover: valid token, invalid token, timeout/fail-open
- [ ] All tests pass
- [ ] `cargo test -p tama-core` passes completely
