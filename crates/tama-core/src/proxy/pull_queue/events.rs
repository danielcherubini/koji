/// Events emitted by the pull queue service during lifecycle transitions.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", rename_all = "PascalCase")]
pub enum PullEvent {
    Started {
        job_id: String,
        repo_id: String,
        filename: String,
        total_bytes: Option<u64>,
    },
    Progress {
        job_id: String,
        bytes_pulled: u64,
        total_bytes: Option<u64>,
    },
    Verifying {
        job_id: String,
        filename: String,
    },
    Completed {
        job_id: String,
        filename: String,
        size_bytes: u64,
        duration_ms: u64,
    },
    Failed {
        job_id: String,
        filename: String,
        error: String,
    },
    Cancelled {
        job_id: String,
        filename: String,
    },
    Queued {
        job_id: String,
        repo_id: String,
        filename: String,
    },
}

impl PullEvent {
    /// Serialize into an SSE event: the `event:` name is the variant name and
    /// the JSON data is the internally-tagged payload (includes the `"event"` key).
    pub fn to_sse_event(&self) -> Result<axum::response::sse::Event, serde_json::Error> {
        let value = serde_json::to_value(self)?;
        let name = value
            .get("event")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let json_str = serde_json::to_string(&value)?;
        Ok(axum::response::sse::Event::default()
            .event(name)
            .data(json_str))
    }
}
