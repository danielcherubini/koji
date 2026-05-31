use leptos::prelude::*;

/// Card wrapper for form sections with title and optional description.
///
/// Replaces the repeated `<div class="card"><h2>...</h2><p class="text-muted">...</p>`
/// pattern in config_editor.rs and similar form pages.
#[component]
pub fn SectionCard(
    /// Section title rendered as <h2>.
    title: String,
    /// Optional description rendered as <p class="text-muted"> below the title.
    #[prop(default = None)]
    description: Option<String>,
    /// Form fields or other content.
    children: Children,
) -> impl IntoView {
    view! {
        <div class="card">
            <h2>{title}</h2>
            {description.map(|d| view! { <p class="text-muted">{d}</p> })}
            {children()}
        </div>
    }
}
