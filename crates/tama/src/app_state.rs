//! Application state combining ProxyState and WebState.
//!
//! ProxyState lives in tama-core and contains core proxy logic.
//! WebState lives in this crate and contains web UI-specific state.
//! This wrapper holds both for shutdown cleanup in main.rs.

use std::sync::Arc;

use tama_core::proxy::ProxyState;

use crate::web_types::WebState;

/// Combined application state for the tama server.
///
/// Holds both the core proxy state and web UI state.
/// Used only for shutdown cleanup (unloading TTS backends, killing job children).
#[derive(Clone)]
pub struct AppState {
    /// Core proxy state from tama-core.
    pub state: Arc<ProxyState>,
    /// Web UI state (jobs, capabilities, etc.).
    pub web_state: Arc<WebState>,
}

impl AppState {
    /// Create a new AppState with the given ProxyState and WebState.
    pub fn new(state: Arc<ProxyState>, web_state: Arc<WebState>) -> Self {
        Self { state, web_state }
    }
}
