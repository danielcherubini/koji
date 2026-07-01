use leptos::prelude::*;

/// Generic two-line list card with left accent strip.
#[component]
pub fn ListCard(
    #[prop(default = None)] state: Option<ReadSignal<Option<String>>>,

    #[prop(default = None)] icon: Option<Children>,

    children: Children,

    #[prop(default = None)] actions: Option<Children>,

    #[prop(default = None)] line2: Option<Children>,
) -> impl IntoView {
    let card_class = move || {
        let state_val = state.as_ref().and_then(|s| s.get());
        match state_val {
            Some(ref s) if !s.is_empty() => format!("list-card list-card--{s}"),
            _ => "list-card".to_string(),
        }
    };

    view! {
        <div class=card_class>
            <div class="list-card__line1">
                {icon.map(|i| i())}
                <span class="list-card__content">{children()}</span>
                {actions.map(|a| a())}
            </div>
            {line2.map(|l| view! { <div class="list-card__line2">{l()}</div> })}
        </div>
    }
}
