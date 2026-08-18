use std::sync::Arc;

use anyhow::Result;

use crate::proxy::ProxyState;

pub mod handlers;
pub mod start;
pub(super) mod start_tamad;
mod verify;

#[cfg(test)]
mod tests;

pub use handlers::{handle_pull_job_stream, handle_tama_get_pull_job, handle_tama_pull_model};
pub use start::start_pull_from_queue;
#[cfg(test)]
pub(crate) use verify::_setup_model_after_pull_with_config;

/// Enqueue a pull in the database queue.
///
/// Creates a `pull_queue` DB row with status='queued' and returns immediately.
/// Does NOT start the pull — the queue processor picks it up and starts it.
/// If `pull_queue` is None (no DB configured), this is a no-op.
pub async fn enqueue_pull(
    state: &Arc<ProxyState>,
    job_id: String,
    repo_id: String,
    filename: &str,
    display_name: Option<&str>,
    quant: Option<&str>,
    context_length: Option<u32>,
) -> Result<(), anyhow::Error> {
    if let Some(ref svc) = state.pull_queue() {
        svc.enqueue(
            &job_id,
            &repo_id,
            filename,
            display_name,
            "model",
            quant,
            context_length,
        )
        .await?;
    }
    Ok(())
}
