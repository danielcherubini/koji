mod handlers;
mod opencode;
mod utils;

/// Internal capability flags for a loaded backend.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ModelCapabilities {
    pub(crate) tool_call: bool,
    pub(crate) reasoning: bool,
}

pub use handlers::*;
pub use opencode::*;
pub use utils::*;

#[cfg(test)]
mod tests {
    mod cancel;
    mod capabilities;
    mod helpers;
    mod opencode;
}
