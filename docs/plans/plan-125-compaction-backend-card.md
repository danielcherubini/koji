# Compaction Backend Card Plan

**Goal:** Add a compaction card to the backends page showing status and an enable/disable toggle.

**Architecture:** Extend `BackendListResponse` with a `compaction` field containing compaction config + runtime status. The backends page renders a dedicated compaction card (not using `BackendCard`) with enable/disable toggle, device/port display, and running status. Toggling enable updates config and triggers start/stop via the existing lifecycle.

**Tech Stack:** Rust (axum API), Leptos (WASM frontend), existing compaction lifecycle from PR #116.

---

### Task 1: Add `CompactionCardDto` and extend `BackendListResponse`

**Context:** The backends list API needs to return compaction status alongside regular backends. Compaction is embedded (always "installed"), so we need a separate DTO that captures compaction-specific fields (enabled, device, port, running status).

**Files:**
- Modify: `crates/tama-web/src/api/backends/types.rs`
- Modify: `crates/tama-web/src/api/backends/list.rs`

**What to implement:**

In `api/backends/types.rs`, add a new struct:

```rust
/// DTO for the compaction backend card (embedded, always installed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionCardDto {
    /// Whether compaction is enabled in config.
    pub enabled: bool,
    /// Compute device (e.g. "cpu", "cuda", "mps").
    pub device: String,
    /// Fixed port or null if auto-assigned.
    pub port: Option<u16>,
    /// Whether the compaction backend is currently running (Ready in model registry).
    pub running: bool,
    /// Server URL if running (e.g. "http://127.0.0.1:18962").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
}
```

Extend `BackendListResponse`:

```rust
pub struct BackendListResponse {
    pub active_job: Option<ActiveJobDto>,
    pub backends: Vec<BackendCardDto>,
    pub custom: Vec<BackendCardDto>,
    #[serde(default)]
    pub available: Vec<String>,
    /// Compaction backend status (embedded, always "installed").
    pub compaction: CompactionCardDto,
}
```

In `api/backends/list.rs`, in `list_backends()`, populate the compaction field:

```rust
// Get compaction config
let compaction_config = state.config.read().await.compaction.clone();

// Check if compaction backend is running (in model registry as "compaction")
let (compaction_running, compaction_url) = {
    let models = state.models.read().await;
    if let Some(model_state) = models.get("compaction") {
        if model_state.is_ready() {
            (true, model_state.backend_url().map(|u| u.to_string()))
        } else {
            (false, None)
        }
    } else {
        (false, None)
    }
};

let compaction_card = CompactionCardDto {
    enabled: compaction_config.enabled,
    device: compaction_config.device,
    port: compaction_config.port,
    running: compaction_running,
    server_url: compaction_url,
    request_timeout_ms: compaction_config.request_timeout_ms,
};
```

Add `compaction: compaction_card` to the response JSON.

**Steps:**
- [ ] Add `CompactionCardDto` struct to `api/backends/types.rs`
- [ ] Add `compaction` field to `BackendListResponse`
- [ ] Populate compaction field in `list_backends()` handler
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add CompactionCardDto to backends list API"

**Acceptance criteria:**
- [ ] `CompactionCardDto` exists with enabled, device, port, running, server_url, request_timeout_ms fields
- [ ] `BackendListResponse` includes compaction field
- [ ] GET /tama/v1/backends returns compaction status
- [ ] Clippy clean

---

### Task 2: Add POST endpoint to toggle compaction config

**Context:** The frontend needs an API endpoint to toggle compaction enabled/disabled. This updates the config and triggers start/stop of the compaction backend.

**Files:**
- Create: `crates/tama-web/src/api/backends/compaction.rs`
- Modify: `crates/tama-web/src/api/backends/mod.rs`
- Modify: `crates/tama-web/src/proxy/server/router.rs` (add route)

**What to implement:**

Create `api/backends/compaction.rs` with:

```rust
//! Compaction backend management endpoints.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use tama_core::proxy::ProxyState;

#[derive(Debug, Deserialize)]
pub struct CompactionToggleRequest {
    pub enabled: bool,
    pub device: Option<String>,
    pub port: Option<Option<u16>>,
    pub request_timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CompactionToggleResponse {
    pub enabled: bool,
    pub running: bool,
}

/// POST /tama/v1/backends/compaction
/// Toggle compaction config and trigger start/stop.
pub async fn update_compaction(
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<CompactionToggleRequest>,
) -> impl IntoResponse {
    // Update config
    {
        let mut config = state.config.write().await;
        if let Some(device) = &req.device {
            config.compaction.device = device.clone();
        }
        if let Some(port) = &req.port {
            config.compaction.port = *port;
        }
        if let Some(timeout) = &req.request_timeout_ms {
            config.compaction.request_timeout_ms = *timeout;
        }
        let was_enabled = config.compaction.enabled;
        config.compaction.enabled = req.enabled;

        // Persist config to disk — follow existing pattern from save_structured_config
        if let Some(ref config_path) = config.loaded_from {
            let config_dir = config_path.parent().unwrap_or(config_path);
            let toml_path = config_dir.join("config.toml");
            if let Ok(toml_str) = toml::to_string_pretty(&*config) {
                let _ = tokio::fs::write(&toml_path, toml_str).await;
            }
        }

        // If enabling and not already running, try to start
        if req.enabled && !was_enabled {
            drop(config);
            // Try to load compaction backend (best effort — don't fail the toggle)
            if let Err(e) = state.load_compaction_backend().await {
                tracing::warn!("Failed to start compaction backend: {}", e);
            }
        }
        // If disabling and was running, we could stop it but there's no unload_compaction_backend()
        // The compaction backend will be cleaned up on shutdown. For now, just update config.
    }

    // Check current running status
    let running = {
        let models = state.models.read().await;
        models
            .get("compaction")
            .map(|s| s.is_ready())
            .unwrap_or(false)
    };

    (StatusCode::OK, Json(CompactionToggleResponse { enabled: req.enabled, running })).into_response()
}
```

Add to `api/backends/mod.rs`:
```rust
pub mod compaction;
```

Add route in `proxy/server/router.rs`:
```rust
// In the backends routes section
.route("/backends/compaction", post(backends::compaction::update_compaction))
```

**Steps:**
- [ ] Create `api/backends/compaction.rs` with the toggle endpoint
- [ ] Add `pub mod compaction;` to `api/backends/mod.rs`
- [ ] Add route in router.rs
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add POST /tama/v1/backends/compaction toggle endpoint"

**Acceptance criteria:**
- [ ] POST /tama/v1/backends/compaction accepts {enabled, device?, port?, request_timeout_ms?}
- [ ] Endpoint updates config and persists to disk
- [ ] Enabling triggers `load_compaction_backend()`
- [ ] Returns current enabled + running status
- [ ] Clippy clean

---

### Task 3: Add compaction card to backends page UI

**Context:** The backends page frontend needs to render a compaction card using the `compaction` field from `BackendListResponse`. The card shows status, config, and an enable/disable toggle.

**Files:**
- Modify: `crates/tama-web/src/pages/backends.rs`

**What to implement:**

In `pages/backends.rs`:

1. Add `compaction` field to `BackendListResponse` frontend struct:
```rust
use crate::components::backend_card::BackendCardDto;
use crate::api::backends::types::CompactionCardDto;

#[derive(Debug, Clone, Deserialize, Default)]
struct BackendListResponse {
    #[serde(default)]
    backends: Vec<BackendCardDto>,
    #[serde(default)]
    custom: Vec<BackendCardDto>,
    #[serde(default)]
    available: Vec<String>,
    #[serde(default)]
    compaction: CompactionCardDto,
}
```

2. Add a request struct and callback for toggling compaction:
```rust
#[derive(Debug, Clone, Serialize)]
struct CompactionToggleRequest {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<Option<u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_timeout_ms: Option<u64>,
}

let on_compaction_toggle = Callback::new(move |enabled: bool| {
    action_error.set(None);
    let req = CompactionToggleRequest {
        enabled,
        device: None,
        port: None,
        request_timeout_ms: None,
    };
    wasm_bindgen_futures::spawn_local(async move {
        match post_request("/tama/v1/backends/compaction").json(&req).unwrap().send().await {
            Ok(resp) if resp.ok() => {
                refresh_tick.update(|n| *n += 1);
            }
            Ok(resp) => {
                let text = resp.text().await.unwrap_or_default();
                action_error.set(Some(format!("Toggle failed: {text}")));
            }
            Err(e) => action_error.set(Some(format!("Toggle request failed: {e}"))),
        }
    });
});
```

3. Add the compaction card view BEFORE the backend cards section (or after, as a separate section):

```rust
{/* Compaction card */}
{move || {
    let comp = compaction.get();
    view! {
        <div class="card" style="margin-bottom:1rem;border-left:3px solid if comp.running { "#22c55e" } else { "#475569" };">
            <div style="display:flex;justify-content:space-between;align-items:center;">
                <div>
                    <h3 style="margin:0;">"LLMLingua Compaction"</h3>
                    <p class="text-muted">
                        {if comp.running { "Running" } else if comp.enabled { "Enabled (not running)" } else { "Disabled" }}
                        {if let Some(url) = &comp.server_url { format!(" — {}", url) } else { "".to_string() }}
                    </p>
                    <p class="text-muted" style="font-size:0.8rem;">
                        "Device: " {&comp.device}
                        {if let Some(p) = comp.port { format!(", Port: {}", p) } else { ", Port: auto".to_string() }}
                    </p>
                </div>
                <label class="form-check" style="display:flex;align-items:center;gap:0.5rem;">
                    <input
                        type="checkbox"
                        class="form-check-input"
                        prop:checked=move || comp.enabled
                        on:change=move |ev| {
                            let enabled = leptos::events::event_target_checked(&ev);
                            on_compaction_toggle.call(enabled);
                        }
                    />
                    <span class="form-check-label">"Enable"</span>
                </label>
            </div>
        </div>
    }
}.into_any()}
```

4. Also remove "compaction" from the "+ Add Backend" dropdown if it was added (it shouldn't be there since it's always embedded).

**Steps:**
- [ ] Add `compaction` field to frontend `BackendListResponse` struct
- [ ] Add `on_compaction_toggle` callback
- [ ] Add compaction card view with toggle, status, and config display
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add compaction card to backends page"

**Acceptance criteria:**
- [ ] Compaction card renders on backends page
- [ ] Card shows enabled/disabled status, device, port
- [ ] Toggle switches enabled state and calls API
- [ ] Running status shown with green accent border
- [ ] Clippy clean

---

### Task 4: Export `CompactionCardDto` and verify round-trip

**Context:** `CompactionCardDto` needs to be exported from the API module so the frontend can use it. Also verify the full round-trip: API returns compaction data, frontend renders it, toggle works.

**Files:**
- Modify: `crates/tama-web/src/api/backends/mod.rs`
- Modify: `crates/tama-web/src/api/mod.rs` (if needed for re-export)

**What to implement:**

In `api/backends/mod.rs`, ensure `CompactionCardDto` is re-exported:
```rust
pub use types::CompactionCardDto;
```

Or if the frontend imports directly from types, ensure the path works.

Verify the full workspace builds and all tests pass.

**Steps:**
- [ ] Export `CompactionCardDto` from api module
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "fix: export CompactionCardDto, verify round-trip"

**Acceptance criteria:**
- [ ] Full workspace builds and clippy clean
- [ ] All tests pass
- [ ] Frontend can import and use `CompactionCardDto`

---

## Rollout

1. Tasks 1-2 can be done in parallel (API changes are independent)
2. Task 3 depends on Tasks 1-2 (needs API response + toggle endpoint)
3. Task 4 is verification, runs after Task 3
