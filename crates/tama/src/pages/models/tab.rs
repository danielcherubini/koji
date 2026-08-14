// ── Tab Navigation ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Models,
    Aliases,
    Providers,
}

impl Tab {
    pub(crate) const ALL: [Tab; 3] = [Tab::Models, Tab::Aliases, Tab::Providers];

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Models => "Models",
            Self::Aliases => "Aliases",
            Self::Providers => "Providers",
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            Self::Models => "📦",
            Self::Aliases => "🏷️",
            Self::Providers => "🔌",
        }
    }
}
