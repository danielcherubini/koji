# OAuth2/OIDC Login Plan

**Goal:** Add native OAuth2 login to Tama's web UI, replacing the Caddy forward_auth dependency with a standard authorization code flow against any OAuth2/OIDC provider (e.g., Authentik).

**Architecture:** The auth middleware gains a session cookie check (priority 1) before existing bearer token and Caddy header checks. Three new routes (`/login`, `/login/callback`, `/logout`) implement the OAuth2 authorization code flow using the `oauth2` crate v5. Sessions are stateless signed cookies (no server-side session store). Config is stored in the existing `app_proxy` DB table via a new migration.

**Tech Stack:** `oauth2` crate v5 (with `reqwest` feature), axum's built-in `cookie` crate (via `cookie` dependency), existing `reqwest` client from ProxyState.

---

### Task 1: OAuth2 config types, DB migration, and config persistence

**Context:**
The OAuth2 login needs configuration (client ID, secret, endpoints, scopes, session TTL). This lives alongside the existing `authenticator_url` in `ProxyConfig` so that bearer token API auth and OIDC browser auth can coexist. The DB schema needs new columns in `app_proxy`, and both the core and WASM config mirrors need the new fields.

**Files:**
- Modify: `crates/tama-core/src/config/types/proxy.rs`
- Modify: `crates/tama-core/src/config/types/mod.rs`
- Modify: `crates/tama-core/src/db/queries/app_config_queries.rs`
- Create: `crates/tama-core/src/db/migrations/_0035_add_oauth2_config.rs`
- Modify: `crates/tama-core/src/db/migrations.rs` (register migration — this is a module file, NOT `migrations/mod.rs`)
- Modify: `crates/tama/src/types/config/proxy.rs` (WASM mirror)
- Modify: `crates/tama/src/types/config/mod.rs` (WASM mirror — if StructuredConfigBody needs updating)
- Modify: `Cargo.toml` (workspace — add `oauth2` and `cookie` deps)
- Modify: `crates/tama-core/Cargo.toml` (add `oauth2` with `reqwest` feature, add `cookie`)
- Test: `crates/tama-core/src/db/queries/app_config_queries.rs` (existing test module — add OAuth2 roundtrip test)
- Test: `crates/tama-core/src/db/migrations/migrations_tests.rs` (add v35 column test)

**What to implement:**

1. **New `OAuth2Config` struct** in `crates/tama-core/src/config/types/proxy.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OAuth2Config {
    pub enabled: bool,
    pub client_id: String,
    /// Supports env var interpolation: "${ENV_VAR_NAME}" is resolved at startup.
    pub client_secret: String,
    pub authorize_url: String,
    pub token_url: String,
    /// Optional — used to fetch user claims after token exchange.
    pub userinfo_url: Option<String>,
    /// Optional — RP-initiated logout endpoint.
    pub logout_url: Option<String>,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub session_ttl_secs: u64,
}

impl Default for OAuth2Config {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: String::new(),
            client_secret: String::new(),
            authorize_url: String::new(),
            token_url: String::new(),
            userinfo_url: None,
            logout_url: None,
            redirect_uri: String::new(),
            scopes: vec!["openid".to_string(), "profile".to_string(), "email".to_string()],
            session_ttl_secs: 86_400, // 24 hours
        }
    }
}
```

2. **Add `oauth2: OAuth2Config` field** to `ProxyConfig` in `crates/tama-core/src/config/types/proxy.rs`. Update `Default` impl to include `oauth2: OAuth2Config::default()`.

3. **Add `resolve_env_vars` helper** to `ProxyConfig` (or a standalone function in the module) that resolves `${VAR_NAME}` patterns in `oauth2.client_secret`:

```rust
impl ProxyConfig {
    /// Resolve environment variable references in OAuth2 client_secret.
    /// "${VAR_NAME}" is replaced with the value of VAR_NAME at runtime.
    /// If the env var is not set, the original string is kept (with a warning).
    pub fn resolve_env_vars(&mut self) {
        if self.oauth2.enabled {
            self.oauth2.client_secret = resolve_env_var_ref(&self.oauth2.client_secret);
        }
    }
}

fn resolve_env_var_ref(value: &str) -> String {
    if let Some(inner) = value.strip_prefix("${").and_then(|s| s.strip_suffix("}")) {
        std::env::var(inner).unwrap_or_else(|_| {
            tracing::warn!("Environment variable '{}' not set, using original value", inner);
            value.to_string()
        })
    } else {
        value.to_string()
    }
}
```

Call `resolve_env_vars()` during config loading (in `Config::from_db` after the proxy record is read, or in `ProxyState::new`).

4. **DB migration `_0035_add_oauth2_config.rs`** — Add columns to `app_proxy`:

```sql
ALTER TABLE app_proxy ADD COLUMN oauth2_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE app_proxy ADD COLUMN oauth2_client_id TEXT NOT NULL DEFAULT '';
ALTER TABLE app_proxy ADD COLUMN oauth2_client_secret TEXT NOT NULL DEFAULT '';
ALTER TABLE app_proxy ADD COLUMN oauth2_authorize_url TEXT NOT NULL DEFAULT '';
ALTER TABLE app_proxy ADD COLUMN oauth2_token_url TEXT NOT NULL DEFAULT '';
ALTER TABLE app_proxy ADD COLUMN oauth2_userinfo_url TEXT;
ALTER TABLE app_proxy ADD COLUMN oauth2_logout_url TEXT;
ALTER TABLE app_proxy ADD COLUMN oauth2_redirect_uri TEXT NOT NULL DEFAULT '';
ALTER TABLE app_proxy ADD COLUMN oauth2_scopes TEXT NOT NULL DEFAULT '["openid","profile","email"]';
ALTER TABLE app_proxy ADD COLUMN oauth2_session_ttl_secs INTEGER NOT NULL DEFAULT 86400;
```

5. **Register migration** in `crates/tama-core/src/db/migrations.rs` — add:
```rust
mod _0035_add_oauth2_config;
```
to the module declarations, add `_0035_add_oauth2_config::MIGRATION,` to the `MIGRATIONS` array, and update `LATEST_VERSION` from `34` to `35`.

6. **Update `app_config_queries.rs`** — Add new fields to `ProxyRecord`, `upsert_proxy`, `get_proxy`, and `seed_defaults`. The `oauth2_scopes` field is stored as JSON (same pattern as `authenticator_skip_paths`).

7. **Update `Config::from_db`** in `crates/tama-core/src/config/types/mod.rs` — Map new proxy record fields to `OAuth2Config`.

8. **Update `Config::to_db`** — Persist `OAuth2Config` fields via `upsert_proxy`.

9. **WASM mirror** — Add `oauth2: OAuth2Config` to `crates/tama/src/types/config/proxy.rs` (the `ProxyConfig` struct used for JSON serialization in the web UI). Update `StructuredConfigBody` conversion if needed.

10. **Cargo dependencies** — Add to workspace `Cargo.toml`:
```toml
oauth2 = { version = "5", features = ["reqwest"] }
cookie = { version = "0.18", features = ["secure"] }
```
Add to `crates/tama-core/Cargo.toml`:
```toml
oauth2.workspace = true
cookie.workspace = true
```

**Steps:**
- [ ] Write failing test: `test_oauth2_proxy_roundtrip` in `app_config_queries.rs` that expects the new OAuth2 columns
- [ ] Run `cargo nextest run --package tama-core -- app_config_queries`
  - Did it fail with compilation error (missing fields)? If it passed unexpectedly, stop and investigate.
- [ ] Create migration `_0035_add_oauth2_config.rs` with ALTER TABLE statements
- [ ] Register migration in `db/migrations/mod.rs`
- [ ] Add `OAuth2Config` struct to `config/types/proxy.rs`
- [ ] Add `oauth2: OAuth2Config` field to `ProxyConfig` and update `Default`
- [ ] Add `resolve_env_vars` method to `ProxyConfig`
- [ ] Update `ProxyRecord` in `app_config_queries.rs` with new fields
- [ ] Update `upsert_proxy` and `get_proxy` functions
- [ ] Update `seed_defaults` to include OAuth2 defaults
- [ ] Update `Config::from_db` to map OAuth2 fields
- [ ] Update `Config::to_db` to persist OAuth2 fields
- [ ] Call `resolve_env_vars()` during config loading (in `ProxyState::new` or `Config::from_db`)
- [ ] Update WASM mirror `ProxyConfig` in `crates/tama/src/types/config/proxy.rs`
- [ ] Add `oauth2` and `cookie` dependencies to workspace and tama-core Cargo.toml
- [ ] Add migration test in `migrations_tests.rs` for v35 columns
- [ ] Run `cargo nextest run --package tama-core -- app_config_queries`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo nextest run --package tama-core -- migrations_tests`
  - Did migration tests pass? If not, fix and re-run.
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add OAuth2 config types and DB migration v35"

**Acceptance criteria:**
- [ ] `OAuth2Config` struct exists with all fields and sensible defaults
- [ ] `ProxyConfig` has `oauth2: OAuth2Config` field
- [ ] Migration v35 adds all OAuth2 columns to `app_proxy`
- [ ] `upsert_proxy` / `get_proxy` round-trip OAuth2 fields correctly
- [ ] `Config::from_db` constructs `OAuth2Config` from DB record
- [ ] `Config::to_db` persists `OAuth2Config` to DB
- [ ] `${ENV_VAR}` resolution works for `client_secret`
- [ ] WASM mirror `ProxyConfig` serializes/deserializes OAuth2 fields
- [ ] All existing tests pass (no regressions)
- [ ] `cargo check --workspace` succeeds

---

### Task 2: Session cookies, OAuth2 login/callback/logout handlers, and auth middleware integration

**Context:**
This is the core of the feature — implementing the OAuth2 authorization code flow and integrating session cookies into the auth middleware. The flow: user hits any protected route → gets 401 (or redirect to `/login`) → `/login` redirects to provider → user authenticates → provider redirects to `/login/callback` → Tama exchanges code for tokens → sets signed session cookie → redirects to `/tama`. Subsequent requests are authenticated via the session cookie.

**Files:**
- Modify: `crates/tama-core/src/proxy/auth.rs` (extend with session + OAuth2)
- Modify: `crates/tama-core/src/proxy/types.rs` (add `cookie_key: cookie::Key` field to `ProxyState` struct)
- Modify: `crates/tama-core/src/proxy/state.rs` (initialize `cookie_key` in `ProxyState::new`)
- Modify: `crates/tama-core/src/proxy/server/router.rs` (add login routes, update skip paths)
- Modify: `crates/tama/src/router.rs` (add login routes to web UI router)
- Test: `crates/tama-core/src/proxy/auth.rs` (existing test module — add session + OAuth2 tests)

**What to implement:**

1. **Session claims type** in `auth.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionClaims {
    sub: String,          // User ID from provider
    username: String,     // Display name
    email: Option<String>,
    iat: i64,             // Issued at (Unix timestamp)
    exp: i64,             // Expiration (Unix timestamp)
}

impl SessionClaims {
    fn new(username: String, email: Option<String>, ttl_secs: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            sub: username.clone(), // Use username as sub if no provider ID available
            username,
            email,
            iat: now,
            exp: now + ttl_secs as i64,
        }
    }

    fn is_valid(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        now < self.exp
    }
}
```

2. **Cookie helpers** in `auth.rs`:

```rust
const SESSION_COOKIE_NAME: &str = "tama_session";
const CSRF_STATE_COOKIE_NAME: &str = "tama_oauth2_state";

/// Create a signed cookie jar from the ProxyState's signing key.
fn cookie_jar(state: &ProxyState) -> cookie::Jar {
    cookie::Jar::new(&state.cookie_key, cookie::Key::generate())
}

/// Extract and validate session claims from request cookies.
fn extract_session(req: &Request, state: &ProxyState) -> Option<SessionClaims> {
    let jar = cookie_jar(state);
    jar.get(SESSION_COOKIE_NAME)
        .and_then(|c| serde_json::from_str(c.value()).ok())
        .filter(|claims: &SessionClaims| claims.is_valid())
}

/// Build a Set-Cookie header for the session.
fn session_cookie(claims: &SessionClaims, is_secure: bool) -> cookie::Cookie {
    let value = serde_json::to_string(claims).unwrap_or_default();
    let mut c = cookie::Cookie::build((SESSION_COOKIE_NAME, value))
        .path("/")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(claims.exp - claims.iat));
    if is_secure {
        c = c.secure(true);
    }
    c.finish()
}
```

3. **`cookie_key` in `ProxyState`** — Add field to the `ProxyState` struct in `types.rs`:

```rust
pub(crate) cookie_key: cookie::Key,
```

Initialize in `ProxyState::new` in `state.rs`:
```rust
cookie_key: cookie::Key::generate(),
```

Note: `ProxyState` fields are `pub(crate)` so `auth.rs` (same crate) can access `state.cookie_key` directly.

4. **OAuth2 client builder** — Function that constructs an `oauth2::BasicClient` from config:

```rust
use oauth2::{BasicClient, AuthUrl, TokenUrl, ClientId, ClientSecret, RedirectUrl};

fn build_oauth2_client(config: &ProxyConfig) -> Result<BasicClient, anyhow::Error> {
    let oauth2 = &config.oauth2;
    Ok(BasicClient::new(ClientId::new(oauth2.client_id.clone()))
        .set_client_secret(ClientSecret::new(oauth2.client_secret.clone()))
        .set_auth_uri(AuthUrl::new(oauth2.authorize_url.clone())?)
        .set_token_uri(TokenUrl::new(oauth2.token_url.clone())?)
        .set_redirect_uri(RedirectUrl::new(oauth2.redirect_uri.clone())?))
}
```

5. **`/login` handler** — Generate authorization URL with CSRF state, store state in signed cookie, redirect to provider:

```rust
pub async fn handle_login(
    State(state): State<Arc<ProxyState>>,
) -> impl IntoResponse {
    let config = state.config.read().await;
    if !config.oauth2.enabled {
        return (StatusCode::SERVICE_UNAVAILABLE, "OAuth2 login is not configured").into_response();
    }

    let oauth2_client = match build_oauth2_client(&config) {
        Ok(c) => c,
        Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, format!("OAuth2 misconfigured: {}", e)).into_response(),
    };

    let mut auth_request = oauth2_client.authorize_url(CsrfToken::new_random);
    for scope in &config.oauth2.scopes {
        auth_request = auth_request.add_scope(oauth2::Scope::new(scope.clone()));
    }
    let (url, csrf_state) = auth_request.url();

    // Store CSRF state in a short-lived signed cookie (5 min TTL)
    let state_value = csrf_state.secret().clone();
    let state_cookie = cookie::Cookie::build((CSRF_STATE_COOKIE_NAME, state_value))
        .path("/login/callback")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(300))
        .finish();

    drop(config);

    (
        StatusCode::FOUND,
        [
            (axum::http::header::LOCATION, url.to_string()),
            (axum::http::header::SET_COOKIE, state_cookie.encoded().to_string()),
        ],
    )
        .into_response()
}
```

6. **`/login/callback` handler** — Verify CSRF state, exchange code for tokens, optionally fetch userinfo, create session, redirect to `/tama`:

```rust
pub async fn handle_login_callback(
    State(state): State<Arc<ProxyState>>,
    cookie_header: Option<axum::extract::CookieJar>, // or raw HeaderMap
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let config = state.config.read().await;
    let oauth2 = config.oauth2.clone();
    drop(config);

    // Extract and verify CSRF state from cookie
    let jar = cookie_jar(&state);
    let expected_state = match jar.get(CSRF_STATE_COOKIE_NAME) {
        Some(c) => c.value().to_string(),
        None => return redirect_to_login_error("state", "CSRF state missing"),
    };

    // Extract code and state from query params
    let code = match query.get("code") {
        Some(c) => oauth2::AuthorizationCode::new(c.clone()),
        None => return redirect_to_login_error("code", "Authorization code missing"),
    };
    let returned_state = match query.get("state") {
        Some(s) => s.clone(),
        None => return redirect_to_login_error("state", "State parameter missing"),
    };

    // Verify CSRF state matches
    if returned_state != expected_state {
        return redirect_to_login_error("state", "CSRF state mismatch");
    }

    // Exchange code for tokens
    let oauth2_client = match build_oauth2_client(&config) {
        Ok(c) => c,
        Err(e) => return redirect_to_login_error("config", &e.to_string()),
    };

    let http_client = state.client.clone(); // reqwest::Client from ProxyState
    let token_result = oauth2_client
        .exchange_code(code)
        .request_async(&http_client)
        .await;

    let token_response = match token_result {
        Ok(t) => t,
        Err(e) => return redirect_to_login_error("token", &format!("{}", e)),
    };

    // Fetch userinfo if configured
    let (username, email) = if let Some(ref userinfo_url) = oauth2.userinfo_url {
        fetch_userinfo(&state.client, userinfo_url, token_response.access_token().secret()).await
    } else {
        // Fallback: use token response extra fields or defaults
        ("unknown".to_string(), None)
    };

    // Create session claims
    let claims = SessionClaims::new(username, email, oauth2.session_ttl_secs);
    let session = session_cookie(&claims, should_set_secure(/* from request */));

    // Redirect to /tama with session cookie
    (
        StatusCode::FOUND,
        [
            (axum::http::header::LOCATION, "/tama"),
            (axum::http::header::SET_COOKIE, session.encoded().to_string()),
        ],
    )
        .into_response()
}

async fn fetch_userinfo(
    client: &reqwest::Client,
    url: &str,
    access_token: &str,
) -> (String, Option<String>) {
    // Call userinfo endpoint and extract preferred_username, email, sub
    // Return ("username", Some("email")) or fallbacks
}

fn redirect_to_login_error(reason: &str, description: &str) -> axum::response::Response {
    let url = format!("/login/error?reason={}&description={}", reason, urlencoding::description);
    (StatusCode::FOUND, [(axum::http::header::LOCATION, url)]).into_response()
}
```

7. **`/logout` handler** — Clear session cookie, optionally redirect to provider logout:

```rust
pub async fn handle_logout(
    State(state): State<Arc<ProxyState>>,
) -> impl IntoResponse {
    let config = state.config.read().await;
    let logout_url = config.oauth2.logout_url.clone();
    drop(config);

    // Create expired session cookie to clear it
    let cleared = cookie::Cookie::build((SESSION_COOKIE_NAME, ""))
        .path("/")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(0))
        .finish();

    if let Some(url) = logout_url {
        (
            StatusCode::FOUND,
            [
                (axum::http::header::LOCATION, url),
                (axum::http::header::SET_COOKIE, cleared.encoded().to_string()),
            ],
        )
            .into_response()
    } else {
        (
            StatusCode::FOUND,
            [
                (axum::http::header::LOCATION, "/tama"),
                (axum::http::header::SET_COOKIE, cleared.encoded().to_string()),
            ],
        )
            .into_response()
    }
}
```

8. **Update `auth_middleware`** — Add session cookie check as the FIRST check (before bearer token):

```rust
// 1. If no auth configured (neither OAuth2 nor authenticator_url), pass through
if !auth_configured {
    return next.run(req).await;
}

// 2. Check skip_paths
if skip_paths.iter().any(|p| path.starts_with(p.as_str())) {
    return next.run(req).await;
}

// 3. NEW: Check session cookie (OIDC login)
if let Some(claims) = extract_session(&req, &proxy_state) {
    debug!("Authenticated user via session cookie: {}", claims.username);
    return next.run(req).await;
}

// 4. Check bearer token (existing)
// ... existing code ...

// 5. Check Caddy forward_auth header (existing)
// ... existing code ...

// 6. No valid auth — for browser requests, redirect to /login
// For API requests (Accept: application/json), return 401
let is_browser = req.headers()
    .get(axum::http::header::ACCEPT)
    .and_then(|v| v.to_str().ok())
    .map(|v| v.contains("text/html"))
    .unwrap_or(false);

if is_browser && oauth2_enabled {
    (StatusCode::FOUND, [(axum::http::header::LOCATION, "/login")]).into_response()
} else {
    (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response()
}
```

9. **Router changes** in `crates/tama-core/src/proxy/server/router.rs`:
- Add `/login` GET route (pointing to `handle_login`)
- Add `/login/callback` GET route (pointing to `handle_login_callback`)
- Add `/logout` GET route (pointing to `handle_logout`)
- These routes must be defined BEFORE the auth middleware layer, OR the auth middleware must handle them specially (skip_paths approach is simpler — add `/login` to default skip paths, `/login/callback` too)

**Recommended approach:** Add the login routes to the router and add `/login`, `/login/callback`, `/login/error` to the default `authenticator_skip_paths`. The `/logout` route should be behind auth (only logged-in users can logout).

In `build_router` and `build_unified_router`, add before the wildcard routes:
```rust
.route("/login", get(handle_login))
.route("/login/callback", get(handle_login_callback))
.route("/logout", get(handle_logout))
```

Update default skip paths in `ProxyConfig::default()`:
```rust
authenticator_skip_paths: vec![
    "/health".to_string(),
    "/metrics".to_string(),
    "/login".to_string(),
    "/login/callback".to_string(),
    "/login/error".to_string(),
],
```

10. **Web UI router** (`crates/tama/src/router.rs`) — The login routes are in the proxy router (shared), so they're already available in the unified router. No changes needed here unless you want `/login` to render a custom page (not needed — it's a redirect).

**Steps:**
- [ ] Write failing test: `test_session_cookie_auth_passes` in `auth.rs` tests — a request with a valid session cookie should pass auth
- [ ] Run `cargo nextest run --package tama-core -- auth::tests::test_session_cookie_auth_passes`
  - Did it fail? If it passed unexpectedly, stop and investigate.
- [ ] Add `SessionClaims` struct and cookie helpers to `auth.rs`
- [ ] Add `pub(crate) cookie_key: cookie::Key` field to `ProxyState` struct in `types.rs`
- [ ] Initialize `cookie_key: cookie::Key::generate()` in `ProxyState::new()` in `state.rs`
- [ ] Implement `build_oauth2_client` helper
- [ ] Implement `handle_login` — redirect to provider with CSRF state cookie
- [ ] Implement `handle_login_callback` — verify state, exchange code, set session, redirect
- [ ] Implement `handle_logout` — clear session cookie, redirect
- [ ] Implement `fetch_userinfo` helper
- [ ] Update `auth_middleware` — add session cookie check as priority 1
- [ ] Update `auth_middleware` — redirect browser requests to `/login` on 401 (when OAuth2 enabled)
- [ ] Add `/login`, `/login/callback`, `/logout` routes to `build_router` and `build_unified_router`
- [ ] Update default `authenticator_skip_paths` to include login routes
- [ ] Run `cargo nextest run --package tama-core -- auth::tests`
  - Did all auth tests pass? If not, fix and re-run.
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: implement OAuth2 login flow with session cookies"

**Acceptance criteria:**
- [ ] `SessionClaims` serializes/deserializes correctly with expiry validation
- [ ] `ProxyState` has `cookie_key` field initialized with random key
- [ ] `/login` generates authorization URL with CSRF state and redirects to provider
- [ ] `/login/callback` verifies CSRF state, exchanges code for tokens, sets session cookie
- [ ] `/login/callback` fetches userinfo when `userinfo_url` is configured
- [ ] `/logout` clears session cookie and redirects
- [ ] Auth middleware checks session cookie before bearer token and Caddy header
- [ ] Browser 401s redirect to `/login` when OAuth2 is enabled
- [ ] API 401s still return JSON `401 Unauthorized`
- [ ] Login routes are in skip paths (not blocked by auth middleware)
- [ ] All existing auth tests pass (bearer token, Caddy header, fail-open)
- [ ] New session cookie tests pass

---

### Task 3: Web UI config editor for OAuth2 settings

**Context:**
The config editor page needs fields for the new OAuth2 configuration so users can enable and configure login through the web UI. The fields should be conditionally shown when OAuth2 is enabled, similar to how `authenticator_skip_paths` is shown when `authenticator_url` is set.

**Files:**
- Modify: `crates/tama/src/pages/config_editor/forms/proxy/advanced.rs`
- Modify: `crates/tama/src/types/config/proxy.rs` (already done in Task 1 — verify fields are present)

**What to implement:**

Add a new section in `ProxyAdvancedFields` component, after the existing "Authenticator URL" section:

```rust
// OAuth2/OIDC Login section
<div>
    <label>"OAuth2 Login Enabled"</label>
    <input
        type="checkbox"
        prop:checked=move || get_proxy().oauth2.enabled
        on:change=move |ev| {
            let checked = ev.target.checkedin().unwrap_or(false);
            config.update(|c| if let Some(c) = c { c.proxy.oauth2.enabled = checked; });
        }
    />
</div>

<Show when=move || get_proxy().oauth2.enabled>
    <div class="oauth2-config" style="border:1px solid #ddd;padding:1rem;margin-top:0.5rem;border-radius:0.5rem;">
        <h3>"OAuth2/OIDC Provider Configuration"</h3>

        <div>
            <label>"Client ID"</label>
            <input type="text" prop:value=move || get_proxy().oauth2.client_id.clone() /* ... */ />
        </div>

        <div>
            <label>"Client Secret"</label>
            <input type="password" prop:value=move || get_proxy().oauth2.client_secret.clone() /* ... */ />
            <p class="text-muted">"Supports ${ENV_VAR} syntax for environment variable references."</p>
        </div>

        <div>
            <label>"Authorize URL"</label>
            <input type="text" placeholder="https://auth.example.com/application/o/authorize/" /* ... */ />
        </div>

        <div>
            <label>"Token URL"</label>
            <input type="text" placeholder="https://auth.example.com/application/o/token/" /* ... */ />
        </div>

        <div>
            <label>"Userinfo URL (optional)"</label>
            <input type="text" placeholder="https://auth.example.com/application/o/userinfo/" /* ... */ />
        </div>

        <div>
            <label>"Logout URL (optional)"</label>
            <input type="text" placeholder="https://auth.example.com/application/o/app-slug/end-session/" /* ... */ />
        </div>

        <div>
            <label>"Redirect URI"</label>
            <input type="text" placeholder="http://localhost:11434/login/callback" /* ... */ />
        </div>

        <div>
            <label>"Scopes (comma-separated)"</label>
            <input type="text" placeholder="openid,profile,email" /* ... */ />
        </div>

        <div>
            <label>"Session TTL (seconds)"</label>
            <input type="number" min="300" /* ... */ />
            <p class="text-muted">"How long a login session lasts. Default: 86400 (24 hours)."</p>
        </div>
    </div>
</Show>
```

Use the existing `target_value` helper from `crate::utils` for input handling. Follow the same pattern as the existing `authenticator_url` / `authenticator_skip_paths` fields for reactivity.

**Steps:**
- [ ] Add OAuth2 enabled checkbox to `ProxyAdvancedFields` component
- [ ] Add conditional OAuth2 configuration section (shown when enabled)
- [ ] Wire all input fields to `config.update()` with proper type conversion
- [ ] Ensure scopes field parses comma-separated string to `Vec<String>` on save
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add OAuth2 config fields to web UI config editor"

**Acceptance criteria:**
- [ ] OAuth2 enabled checkbox toggles the config section visibility
- [ ] All OAuth2 fields are editable and reactive
- [ ] Scopes field accepts comma-separated input and converts to `Vec<String>`
- [ ] Client secret field is type="password"
- [ ] Optional fields (userinfo_url, logout_url) accept empty values
- [ ] Config save persists OAuth2 settings to DB
- [ ] Config load restores OAuth2 settings from DB
- [ ] `cargo check --workspace` succeeds

---

### Verification (after all tasks)

Run the full gate:
```bash
cargo check --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo nextest run --workspace
```

Manual verification checklist:
- [ ] Start Tama with OAuth2 configured → `/login` redirects to provider
- [ ] Complete login at provider → redirected to `/tama` with session cookie
- [ ] Subsequent requests use session cookie (no bearer token needed)
- [ ] `/logout` clears session and redirects
- [ ] Bearer token auth still works for API clients
- [ ] Caddy `X-Authentik-Username` header still works as fallback
- [ ] Config editor saves/loads OAuth2 settings correctly
- [ ] `${ENV_VAR}` resolution works for client_secret
