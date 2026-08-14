// ── Tab Navigation ──────────────────────────────────────────────────────────────

use leptos::prelude::*;

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

/// Shared tab pills navigation, rendered after each tab's page-header.
#[component]
pub fn TabPills(active_tab: RwSignal<Tab>) -> impl IntoView {
    view! {
        <div class="model-editor-pills">
            {Tab::ALL.map(|tab| {
                let t = tab;
                view! {
                    <button
                        class="model-editor-pill"
                        class:model-editor-pill--active=move || active_tab.get() == t
                        on:click=move |_| active_tab.set(t)
                    >
                        <span>{t.icon()}</span>
                        <span>{t.name()}</span>
                    </button>
                }
            })}
        </div>
    }
}
