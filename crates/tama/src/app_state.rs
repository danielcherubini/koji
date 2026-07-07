//! Application state combining ProxyState and WebState.
//!
//! ProxyState lives in tama-core and contains core proxy logic.
//! WebState lives in this crate and contains web UI-specific state.
//! This wrapper holds both for convenient passing to handlers.

use std::ops::Deref;
use std::sync::Arc;

use tama_core::proxy::ProxyState;

use crate::web_types::WebState;

/// Combined application state for the tama server.
///
/// Holds both the core proxy state and web UI state.
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

    /// Get a reference to the inner ProxyState.
    pub fn proxy_state(&self) -> &Arc<ProxyState> {
        &self.state
    }

    /// Get a reference to the inner WebState.
    pub fn web_state_ref(&self) -> &Arc<WebState> {
        &self.web_state
    }

    // ── WebState convenience accessors ──

    /// Returns the job manager, if available.
    pub fn web_jobs(&self) -> Option<Arc<crate::web_types::JobManager>> {
        self.web_state.jobs.clone()
    }

    /// Returns the capabilities cache, if available.
    pub fn web_capabilities(&self) -> Option<Arc<crate::web_types::CapabilitiesCache>> {
        self.web_state.capabilities.clone()
    }

    /// Returns a clone of the update checker.
    pub fn web_update_checker(&self) -> Arc<tama_core::updates::UpdateChecker> {
        Arc::clone(&self.web_state.update_checker)
    }

    /// Returns the current binary version string.
    pub fn web_binary_version(&self) -> String {
        self.web_state.binary_version.clone()
    }

    /// Sets the binary version string.
    pub fn set_binary_version(&mut self, version: &str) {
        let mut inner = (*self.web_state).clone();
        inner.binary_version = version.to_string();
        self.web_state = Arc::new(inner);
    }

    /// Returns a clone of the update broadcast sender.
    pub fn web_update_tx(
        &self,
    ) -> Arc<tokio::sync::Mutex<Option<tokio::sync::broadcast::Sender<String>>>> {
        Arc::clone(&self.web_state.update_tx)
    }

    /// Returns a clone of the upload lock.
    pub fn web_upload_lock(
        &self,
    ) -> Arc<tokio::sync::RwLock<std::collections::HashMap<String, crate::web_types::UploadEntry>>>
    {
        Arc::clone(&self.web_state.upload_lock)
    }
}

impl Deref for AppState {
    type Target = Arc<ProxyState>;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
