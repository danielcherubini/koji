// ── Section Navigation ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Settings,
    Hardware,
    Sampling,
    Files,
    Advanced,
}

impl Section {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Hardware => "Hardware",
            Self::Sampling => "Sampling",
            Self::Files => "Files",
            Self::Advanced => "Advanced",
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            Self::Settings => "⚙️",
            Self::Hardware => "🖥️",
            Self::Sampling => "🎲",
            Self::Files => "📁",
            Self::Advanced => "🔧",
        }
    }
}
