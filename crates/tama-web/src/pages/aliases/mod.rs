mod api;
mod types;

use leptos::prelude::*;

use crate::components::modal::Modal;
use crate::utils::{rw_signal_to_signal, target_value};

use self::api::*;
use self::types::{Alias, ModelOption};

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
                <p>"Custom model aliases — point a friendly name to any loaded model."</p>
                <div class="page-header-actions">
                    {move || save_status.get().map(|(ok, msg)| {
                        let cls = if ok { "alert alert--success" } else { "alert alert--error" };
                        view! { <div class=cls>{msg}</div> }
                    })}
                    <button
                        class="btn btn-primary"
                        on:click=move |_| show_create.set(true)
                    >
                        "+ New Alias"
                    </button>
                </div>
            </div>

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
            {move || {
                error.get().map(|e| {
                    view! {
                        <div class="alert alert--error">{e}</div>
                    }
                    .into_any()
                })
            }}

            // Empty state (when not loading and no aliases)
            {move || {
                (!loading.get() && aliases.get().is_empty()).then(|| {
                    view! {
                        <div class="card card--centered">
                            <p class="text-muted">"No aliases configured yet."</p>
                            <button class="btn btn-primary mt-2" on:click=move |_| show_create.set(true)>
                                "Create your first alias"
                            </button>
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
    let alias_name = alias.name.clone();
    let alias_id = alias.id;
    let alias_enabled = alias.enabled;
    let show_edit = RwSignal::new(false);

    let alias_name_for_title = alias.name.clone();
    let alias_name_for_delete = alias.name.clone();

    view! {
        <div class="card alias-card">
            <div class="card-header">
                <h3>{alias_name.clone()}</h3>
                <span class=format!("badge {}", if alias.enabled { "badge--enabled" } else { "badge--disabled" })>
                    {if alias.enabled { "Enabled" } else { "Disabled" }}
                </span>
            </div>
            <div class="card-body">
                <p>"→" {alias.model_name.clone()}</p>
                <p class="description">{alias.description.as_deref().unwrap_or("").to_string()}</p>
            </div>
            <div class="card-actions">
                <button
                    class="btn btn-sm btn-secondary"
                    on:click=move |_| show_edit.set(true)
                >
                    "Edit"
                </button>
                <button
                    class="btn btn-sm"
                    class=("btn-secondary", move || !alias_enabled)
                    class=("btn-warning", move || alias_enabled)
                    on:click=move |_| {
                        let aliases_c = aliases;
                        let save_status_c = save_status;
                        wasm_bindgen_futures::spawn_local(async move {
                            match update_alias(alias_id, None, None, None, Some(!alias_enabled)).await {
                                Ok(updated) => {
                                    let mut list = aliases_c.get_untracked();
                                    if let Some(pos) = list.iter().position(|a| a.id == alias_id) {
                                        list[pos] = updated;
                                        list.sort_by(|a, b| a.name.cmp(&b.name));
                                        aliases_c.set(list);
                                    }
                                    save_status_c.set(Some((
                                        true,
                                        format!(
                                            "Alias {}.",
                                            if alias_enabled { "disabled" } else { "enabled" }
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
                    {if alias_enabled { "Disable" } else { "Enable" }}
                </button>
                <button
                    class="btn btn-sm btn-danger"
                    on:click=move |_| {
                        let name_for_confirm = alias_name_for_delete.clone();
                        let confirmed = web_sys::window()
                            .and_then(|w| w.confirm_with_message(&format!("Delete alias \"{}\"? This cannot be undone.", name_for_confirm)).ok())
                            .unwrap_or(false);
                        if confirmed {
                            let name_for_status = alias_name_for_delete.clone();
                            let aliases_c = aliases;
                            let save_status_c = save_status;
                            wasm_bindgen_futures::spawn_local(async move {
                                match delete_alias(alias_id).await {
                                    Ok(()) => {
                                        let mut list = aliases_c.get_untracked();
                                        list.retain(|a| a.id != alias_id);
                                        aliases_c.set(list);
                                        save_status_c.set(Some((true, format!("Alias \"{}\" deleted.", name_for_status))));
                                    }
                                    Err(e) => {
                                        save_status_c.set(Some((false, format!("Failed to delete alias: {}", e))));
                                    }
                                }
                            });
                        }
                    }
                >
                    "Delete"
                </button>
            </div>

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
        </div>
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

        if name_val.is_empty() {
            submit_error.set(Some("Alias name is required.".to_string()));
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

        if name_val.is_empty() {
            submit_error.set(Some("Alias name is required.".to_string()));
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
