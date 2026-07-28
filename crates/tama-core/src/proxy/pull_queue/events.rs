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

impl crate::sse::ToSseEvent for PullEvent {}
