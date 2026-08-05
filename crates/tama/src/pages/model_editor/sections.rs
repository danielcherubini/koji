// ── Section Navigation ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Settings,
    Context,
    Sampling,
    Files,
    Advanced,
}

impl Section {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Context => "Context",
            Self::Sampling => "Sampling",
            Self::Files => "Files",
            Self::Advanced => "Advanced",
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            Self::Settings => "⚙️",
            Self::Context => "🖥️",
            Self::Sampling => "🎲",
            Self::Files => "📁",
            Self::Advanced => "🔧",
        }
    }
}
