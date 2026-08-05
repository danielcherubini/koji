// ── Section Navigation ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Settings,
    Context,
    Sampling,
    Files,
    Advanced,
    Vllm,
}

impl Section {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Context => "Context",
            Self::Sampling => "Sampling",
            Self::Files => "Files",
            Self::Advanced => "Advanced",
            Self::Vllm => "vLLM",
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            Self::Settings => "⚙️",
            Self::Context => "🖥️",
            Self::Sampling => "🎲",
            Self::Files => "📁",
            Self::Advanced => "🔧",
            Self::Vllm => "🤖",
        }
    }
}
