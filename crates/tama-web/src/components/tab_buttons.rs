use leptos::prelude::*;

/// Tab button definition — key and display label.
#[derive(Debug, Clone)]
pub struct TabButton {
    pub key: String,
    pub label: String,
}

/// Tab navigation buttons with active/inactive styling.
///
/// Presenter-controlled: the caller manages the active tab signal and
/// renders tab content conditionally. This component only renders the buttons.
#[component]
pub fn TabButtons(
    /// Currently active tab key.
    active: Signal<String>,
    /// Tab definitions — key and display label.
    tabs: Vec<TabButton>,
    /// Called with the tab key when clicked.
    on_select: Callback<String>,
    /// CSS class for active button. Default: "btn btn-sm btn-primary".
    #[prop(default = "btn btn-sm btn-primary".into())]
    active_class: String,
    /// CSS class for inactive button. Default: "btn btn-sm btn-outline-secondary".
    #[prop(default = "btn btn-sm btn-outline-secondary".into())]
    inactive_class: String,
) -> impl IntoView {
    view! {
        <div class="tab-buttons">
            {tabs.into_iter().map(|tab| {
                let key1 = tab.key.clone();
                let key2 = tab.key.clone();
                let label = tab.label.clone();
                let ac = active_class.clone();
                let ic = inactive_class.clone();
                view! {
                    <button
                        class=move || if active.get() == key1 { ac.clone() } else { ic.clone() }
                        on:click=move |_| on_select.run(key2.clone())
                    >
                        {label}
                    </button>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}
