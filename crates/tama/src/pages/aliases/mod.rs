mod api;
mod types;

use leptos::prelude::*;

use crate::components::alert_banner::{AlertBanner, AlertVariant};
use crate::components::list_card::ListCard;
use crate::components::modal::Modal;
use crate::utils::{rw_signal_to_signal, target_value};

use self::api::*;
use self::types::{Alias, ModelOption};

/// Validates an alias name against the allowed pattern.
/// Pattern: ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}$
/// Returns None if valid, or an error message if invalid.
fn validate_alias_name(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let len = bytes.len();

    if len == 0 {
        return Some("Alias name is required.".to_string());
    }
    if len > 128 {
        return Some("Alias name must be 128 characters or fewer.".to_string());
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return Some("Alias name must start with a letter or number.".to_string());
    }
    for &b in &bytes[1..] {
        if !b.is_ascii_alphanumeric() && b != b'_' && b != b'-' && b != b'.' {
            return Some(
                "Alias name can only contain letters, numbers, hyphens, underscores, and periods."
                    .to_string(),
            );
        }
    }
    None
}

/// Main Aliases page component.
/// Displays a card-based list of model aliases with create/edit/delete functionality.
#[component]
pub fn AliasesPage() -> impl IntoView {
    let aliases = RwSignal::new(Vec::<Alias>::new());
    let models = RwSignal::new(Vec::<ModelOption>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let show_create = RwSignal::new(false);
    let save_status = RwSignal::new(Option::<(bool, String)>::None);

    // Load aliases and models on mount
    let aliases_c = aliases;
    let models_c = models;
    let loading_c = loading;
    let error_c = error;
    wasm_bindgen_futures::spawn_local(async move {
        loading_c.set(true);
        error_c.set(None);

        // Fetch aliases
        match fetch_aliases().await {
            Ok(list) => {
                aliases_c.set(list);
            }
            Err(e) => {
                error_c.set(Some(format!("Failed to load aliases: {}", e)));
            }
        }

        // Fetch models for dropdown
        match fetch_models().await {
            Ok(model_list) => {
                models_c.set(model_list);
            }
            Err(e) => {
                error_c.update(|err| {
                    let msg = format!("Failed to load models: {}", e);
                    match err.as_mut() {
                        Some(existing) => existing.push_str(&format!("\n{}", msg)),
                        None => *err = Some(msg),
                    }
                });
            }
        }

        loading_c.set(false);
    });

    view! {
        <div class="page">
            <div class="page-header">
                <h1>"🏷️ Aliases"</h1>
                <button
                    class="btn btn-primary"
                    on:click=move |_| show_create.set(true)
                >
                    "+ New Alias"
                </button>
            </div>
            <p class="page-header__subtitle">"Custom model aliases - point a friendly name to any loaded model."</p>

            // Save status alerts
            {move || save_status.get().map(|(ok, msg)| {
                let variant = if ok { AlertVariant::Success } else { AlertVariant::Error };
                view! { <AlertBanner variant=variant>{msg}</AlertBanner> }
            })}

            // Loading state
            {move || {
                loading.get().then(|| {
                    view! {
                        <div class="card card--centered">
                            <span class="spinner">"Loading aliases..."</span>
                        </div>
                    }
                    .into_any()
                })
            }}

            // Error state
            {move || error.get().map(|e| view! {
                <AlertBanner variant=AlertVariant::Error>{e}</AlertBanner>
            }.into_any())}

            // Empty state (when not loading and no aliases)
            {move || {
                (!loading.get() && aliases.get().is_empty()).then(|| {
                    view! {
                        <div class="card card--centered">
                            <p class="text-muted">"No aliases yet. Click + New to create one."</p>
                        </div>
                    }
                    .into_any()
                })
            }}

            // Alias card list
            {move || {
                let items = aliases.get();
                if !items.is_empty() {
                    view! {
                        <div class="aliases-list">
                            {items.into_iter().map(|alias| {
                                view! {
                                    <AliasCard
                                        alias=alias
                                        aliases=aliases
                                        models=models
                                        save_status=save_status
                                    />
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    }
                    .into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Create modal
            <Modal
                open=rw_signal_to_signal(show_create)
                on_close=Callback::new(move |_| show_create.set(false))
                title="Create Alias".to_string()
            >
                <CreateAliasForm
                    models=models
                    aliases=aliases
                    save_status=save_status
                    on_close=Callback::new(move |_| show_create.set(false))
                />
            </Modal>
        </div>
    }
}

/// Individual alias card component.
#[component]
fn AliasCard(
    alias: Alias,
    aliases: RwSignal<Vec<Alias>>,
    models: RwSignal<Vec<ModelOption>>,
    save_status: RwSignal<Option<(bool, String)>>,
) -> impl IntoView {
    let alias_id = alias.id;
    let alias_name = alias.name.clone();
    let alias_enabled = alias.enabled;
    let alias_model_name = alias.model_name.clone();
    let alias_description = alias.description.clone();
    let alias_description_is_default =
        alias.description.as_deref() == Some("Default alias - routes to this model");
    let show_edit = RwSignal::new(false);

    let alias_name_for_title = alias_name.clone();
    let alias_name_for_delete = alias_name.clone();

    // Clone values for closures (need 'static for Children type)
    let alias_enabled_icon = alias_enabled;
    let show_edit_actions = show_edit;
    let aliases_actions = aliases;
    let save_status_actions = save_status;
    let alias_id_actions = alias_id;
    let alias_enabled_actions = alias_enabled;
    let alias_name_for_delete_actions = alias_name_for_delete;
    let alias_model_name_line2 = alias_model_name;
    let alias_description_line2 = alias_description;
    let alias_description_is_default_line2 = alias_description_is_default;

    view! {
        <ListCard
            state=Some(RwSignal::new(Some(if alias_enabled { "enabled".to_string() } else { "disabled".to_string() })).read_only())
            icon=Some(Box::new(move || view! {
                <span class=format!("alias-card__dot {}", if alias_enabled_icon { "alias-card__dot--enabled" } else { "alias-card__dot--disabled" })></span>
            }.into_any()))
            actions=Some(Box::new(move || view! {
                // Edit button
                <button class="btn-icon" title="Edit" on:click=move |_| show_edit_actions.set(true)>
                    "✏️"
                </button>
                // Toggle enable/disable
                <button
                    class="btn-icon"
                    title=if alias_enabled_actions { "Disable" } else { "Enable" }
                    on:click=move |_| {
                        let aliases_c = aliases_actions;
                        let save_status_c = save_status_actions;
                        wasm_bindgen_futures::spawn_local(async move {
                            match update_alias(alias_id_actions, None, None, None, Some(!alias_enabled_actions)).await {
                                Ok(updated) => {
                                    let mut list = aliases_c.get_untracked();
                                    if let Some(pos) = list.iter().position(|a| a.id == alias_id_actions) {
                                        list[pos] = updated;
                                        list.sort_by(|a, b| a.name.cmp(&b.name));
                                        aliases_c.set(list);
                                    }
                                    save_status_c.set(Some((
                                        true,
                                        format!(
                                            "Alias {}.",
                                            if alias_enabled_actions { "disabled" } else { "enabled" }
                                        ),
                                    )));
                                }
                                Err(e) => {
                                    save_status_c.set(Some((false, format!("Failed to toggle alias: {}", e))));
                                }
                            }
                        });
                    }
                >
                    {if alias_enabled_actions { "👁️" } else { "🚫" }}
                </button>
                // Delete button
                <button class="btn-icon btn-icon--danger" title="Delete" on:click=move |_| {
                    let name_for_confirm = alias_name_for_delete_actions.clone();
                    let confirmed = web_sys::window()
                        .and_then(|w| w.confirm_with_message(&format!("Delete alias \"{}\"? This cannot be undone.", name_for_confirm)).ok())
                        .unwrap_or(false);
                    if confirmed {
                        let name_for_status = alias_name_for_delete_actions.clone();
                        let aliases_c = aliases_actions;
                        let save_status_c = save_status_actions;
                        wasm_bindgen_futures::spawn_local(async move {
                            match delete_alias(alias_id_actions).await {
                                Ok(()) => {
                                    let mut list = aliases_c.get_untracked();
                                    list.retain(|a| a.id != alias_id_actions);
                                    aliases_c.set(list);
                                    save_status_c.set(Some((true, format!("Alias \"{}\" deleted.", name_for_status))));
                                }
                                Err(e) => {
                                    save_status_c.set(Some((false, format!("Failed to delete alias: {}", e))));
                                }
                            }
                        });
                    }
                }>
                    "🗑️"
                </button>
            }.into_any()))
            line2=Some(Box::new(move || view! {
                <span class="alias-card__target">
                    <span class="alias-card__target-arrow">"→"</span>
                    {alias_model_name_line2}
                </span>
                // Description (only if non-empty)
                {alias_description_line2.as_ref().map(|d| {
                    if d.is_empty() {
                        view! { <span/> }.into_any()
                    } else {
                        let desc = d.to_string();
                        view! { <span class="alias-card__description">{desc}</span> }.into_any()
                    }
                }).unwrap_or_else(|| view! { <span/> }.into_any())}
                // Default alias badge
                {if alias_description_is_default_line2 {
                    view! { <span class="badge-pill badge-pill--default">"Default alias"</span> }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
            }.into_any()))
        >
            // Children — just the alias name
            <span class="alias-card__name">{alias_name.clone()}</span>
        </ListCard>

        // Inline edit modal
        <Modal
            open=rw_signal_to_signal(show_edit)
            on_close=Callback::new(move |_| show_edit.set(false))
            title=move || format!("Edit: {}", alias_name_for_title)
        >
            <EditAliasForm
                alias=alias.clone()
                models=models
                aliases=aliases
                save_status=save_status
                on_close=Callback::new(move |_| show_edit.set(false))
            />
        </Modal>
    }
}

/// Form for creating a new alias.
#[component]
fn CreateAliasForm(
    models: RwSignal<Vec<ModelOption>>,
    aliases: RwSignal<Vec<Alias>>,
    save_status: RwSignal<Option<(bool, String)>>,
    on_close: Callback<(), ()>,
) -> impl IntoView {
    let name = RwSignal::new(String::new());
    let model_id = RwSignal::new(0i64);
    let description = RwSignal::new(String::new());
    let submit_error = RwSignal::new(Option::<String>::None);

    // Set initial model_id from first available model
    Effect::new(move |_| {
        let model_list = models.get();
        if let Some(first) = model_list.first() {
            model_id.set(first.id);
        }
    });

    let handle_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let name_val = name.get().trim().to_string();
        let desc_val = description.get().trim().to_string();
        let model_id_val = model_id.get();

        if let Some(err) = validate_alias_name(&name_val) {
            submit_error.set(Some(err));
            return;
        }
        if model_id_val == 0 {
            submit_error.set(Some("Please select a model.".to_string()));
            return;
        }

        submit_error.set(None);
        let aliases_c = aliases;
        let save_status_c = save_status;
        let on_close_c = on_close;
        wasm_bindgen_futures::spawn_local(async move {
            match create_alias(&name_val, model_id_val, &desc_val).await {
                Ok(new_alias) => {
                    let mut list = aliases_c.get_untracked();
                    list.push(new_alias);
                    list.sort_by(|a, b| a.name.cmp(&b.name));
                    aliases_c.set(list);
                    on_close_c.run(());
                    save_status_c.set(Some((true, "Alias created successfully.".to_string())));
                }
                Err(e) => {
                    submit_error.set(Some(format!("Failed to create alias: {}", e)));
                    save_status_c.set(Some((false, format!("Failed to create alias: {}", e))));
                }
            }
        });
    };

    view! {
        <form on:submit=handle_submit>
            <div class="form-group">
                <label for="alias-name">"Alias Name"</label>
                <input
                    id="alias-name"
                    type="text"
                    placeholder="e.g. my-fast-model"
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(target_value(&ev))
                    autofocus=true
                />
            </div>

            <div class="form-group">
                <label for="alias-model">"Model"</label>
                <select
                    id="alias-model"
                    prop:value=move || model_id.get().to_string()
                    on:change=move |ev| {
                        if let Ok(val) = target_value(&ev).parse::<i64>() {
                            model_id.set(val);
                        }
                    }
                >
                    <option value="0">"-- Select a model --"</option>
                    {move || {
                        models.get().into_iter().map(|m| {
                            view! {
                                <option value={m.id.to_string()}>{m.label}</option>
                            }
                        }).collect::<Vec<_>>()
                    }}
                </select>
            </div>

            <div class="form-group">
                <label for="alias-desc">"Description (optional)"</label>
                <textarea
                    id="alias-desc"
                    placeholder="What is this alias for?"
                    prop:value=move || description.get()
                    on:input=move |ev| description.set(target_value(&ev))
                    rows=3
                />
            </div>

            {move || submit_error.get().map(|e| view! { <div class="text-error">{e}</div> })}

            <div class="form-actions">
                <button type="button" class="btn btn-secondary" on:click=move |_| on_close.run(())>
                    "Cancel"
                </button>
                <button type="submit" class="btn btn-primary">
                    "Create Alias"
                </button>
            </div>
        </form>
    }
}

/// Form for editing an existing alias.
#[component]
fn EditAliasForm(
    alias: Alias,
    models: RwSignal<Vec<ModelOption>>,
    aliases: RwSignal<Vec<Alias>>,
    save_status: RwSignal<Option<(bool, String)>>,
    on_close: Callback<(), ()>,
) -> impl IntoView {
    let name = RwSignal::new(alias.name.clone());
    let model_id = RwSignal::new(alias.model_id);
    let description = RwSignal::new(alias.description.clone().unwrap_or_default());
    let enabled = RwSignal::new(alias.enabled);
    let submit_error = RwSignal::new(Option::<String>::None);
    let alias_id = alias.id;

    let handle_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let name_val = name.get().trim().to_string();
        let desc_val = description.get().trim().to_string();
        let model_id_val = model_id.get();
        let enabled_val = enabled.get();

        if let Some(err) = validate_alias_name(&name_val) {
            submit_error.set(Some(err));
            return;
        }
        if model_id_val == 0 {
            submit_error.set(Some("Please select a model.".to_string()));
            return;
        }

        submit_error.set(None);
        let aliases_c = aliases;
        let save_status_c = save_status;
        let on_close_c = on_close;
        wasm_bindgen_futures::spawn_local(async move {
            match update_alias(
                alias_id,
                Some(&name_val),
                Some(model_id_val),
                if desc_val.is_empty() {
                    Some("")
                } else {
                    Some(&desc_val)
                },
                Some(enabled_val),
            )
            .await
            {
                Ok(updated) => {
                    let mut list = aliases_c.get_untracked();
                    if let Some(pos) = list.iter().position(|a| a.id == alias_id) {
                        list[pos] = updated;
                        list.sort_by(|a, b| a.name.cmp(&b.name));
                        aliases_c.set(list);
                    }
                    on_close_c.run(());
                    save_status_c.set(Some((true, "Alias updated successfully.".to_string())));
                }
                Err(e) => {
                    submit_error.set(Some(format!("Failed to update alias: {}", e)));
                    save_status_c.set(Some((false, format!("Failed to update alias: {}", e))));
                }
            }
        });
    };

    view! {
        <form on:submit=handle_submit>
            <div class="form-group">
                <label for="edit-alias-name">"Alias Name"</label>
                <input
                    id="edit-alias-name"
                    type="text"
                    placeholder="e.g. my-fast-model"
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(target_value(&ev))
                    autofocus=true
                />
            </div>

            <div class="form-group">
                <label for="edit-alias-model">"Model"</label>
                <select
                    id="edit-alias-model"
                    on:change=move |ev| {
                        if let Ok(val) = target_value(&ev).parse::<i64>() {
                            model_id.set(val);
                        }
                    }
                >
                    <option value="0" selected=move || model_id.get() == 0>"-- Select a model --"</option>
                    {move || {
                        let current_id = model_id.get();
                        models.get().into_iter().map(move |m| {
                            view! {
                                <option
                                    value={m.id.to_string()}
                                    selected=move || current_id == m.id
                                >{m.label}</option>
                            }
                        }).collect::<Vec<_>>()
                    }}
                </select>
            </div>

            <div class="form-group">
                <label for="edit-alias-desc">"Description (optional)"</label>
                <textarea
                    id="edit-alias-desc"
                    placeholder="What is this alias for?"
                    prop:value=move || description.get()
                    on:input=move |ev| description.set(target_value(&ev))
                    rows=3
                />
            </div>

            <div class="form-group form-group--checkbox">
                <label>
                    <input
                        type="checkbox"
                        prop:checked=move || enabled.get()
                        on:change=move |ev| {
                            use wasm_bindgen::JsCast;
                            if let Some(checked) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                enabled.set(checked.checked());
                            }
                        }
                    />
                    "Enabled"
                </label>
            </div>

            {move || submit_error.get().map(|e| view! { <div class="text-error">{e}</div> })}

            <div class="form-actions">
                <button type="button" class="btn btn-secondary" on:click=move |_| on_close.run(())>
                    "Cancel"
                </button>
                <button type="submit" class="btn btn-primary">
                    "Save Changes"
                </button>
            </div>
        </form>
    }
}
