use leptos::prelude::*;

/// Alert variant — determines colors and default icon.
#[derive(Debug, Clone, Copy, Default)]
pub enum AlertVariant {
    Success, // green
    Error,   // red
    Warning, // amber/yellow
    #[default]
    Info, // blue
}

impl AlertVariant {
    fn css_class(self) -> &'static str {
        match self {
            AlertVariant::Success => "alert alert--success",
            AlertVariant::Error => "alert alert--error",
            AlertVariant::Warning => "alert alert--warning",
            AlertVariant::Info => "alert alert--info",
        }
    }

    fn default_icon(self) -> &'static str {
        match self {
            AlertVariant::Success => "✓",
            AlertVariant::Error => "✗",
            AlertVariant::Warning => "⚠",
            AlertVariant::Info => "ℹ",
        }
    }
}

/// Colored alert banner for displaying status messages.
#[component]
pub fn AlertBanner(
    /// Alert type that determines colors and default icon.
    #[prop(default = AlertVariant::Info)]
    variant: AlertVariant,
    /// Optional custom icon. Defaults to variant-specific icon.
    #[prop(default = None)]
    icon: Option<String>,
    /// Alert message content.
    children: Children,
) -> impl IntoView {
    let icon_text = icon.unwrap_or_else(|| variant.default_icon().to_string());
    view! {
        <div class={variant.css_class()}>
            <span class="alert__icon">{icon_text}</span>
            <span>{children()}</span>
        </div>
    }
}
