mod handlers;
mod opencode;
mod utils;

/// Internal capability flags for a loaded backend.
#[derive(Debug, Clone, Copy, Default)]
struct ModelCapabilities {
    tool_call: bool,
    reasoning: bool,
}

// Re-export only the public handlers — internal helpers stay private.
pub use handlers::{
    handle_tama_cancel_load, handle_tama_get_model, handle_tama_list_models,
    handle_tama_load_model, handle_tama_unload_model,
};
pub use opencode::handle_opencode_list_models;
pub use utils::{capitalize_first, generate_display_name};

#[cfg(test)]
mod tests {
    mod cancel;
    mod capabilities;
    mod helpers;
    mod opencode;
}
