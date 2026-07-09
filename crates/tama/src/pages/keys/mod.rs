mod api;
mod types;

use leptos::prelude::*;

use crate::components::alert_banner::{AlertBanner, AlertVariant};
use crate::components::list_card::ListCard;
use crate::components::modal::Modal;
use crate::utils::{rw_signal_to_signal, target_value};

use self::api::*;
use self::types::{ApiKey, CreateKeyResponse, AVAILABLE_SCOPES};

/// Returns true if the key is currently active (not revoked and not expired).
pub(crate) fn is_active(key: &ApiKey) -> bool {
    if key.revoked_at.is_some() {
        return false;
    }
    if let Some(ref ts) = key.expires_at {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
            if chrono::Utc::now() >= dt {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(id: i64, name: &str, revoked_at: Option<&str>, expires_at: Option<&str>) -> ApiKey {
        ApiKey {
            id,
            name: name.to_string(),
            key_prefix: "tama_test".to_string(),
            scopes: vec!["inference".to_string()],
            created_by: "admin".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            last_used_at: None,
            revoked_at: revoked_at.map(String::from),
            expires_at: expires_at.map(String::from),
        }
    }

    /// Test 1: Active key (no revoked, no expiry) → true
    #[test]
    fn test_is_active_no_revoked_no_expiry() {
        let key = make_key(1, "active-key", None, None);
        assert!(is_active(&key));
    }

    /// Test 2: Revoked key → false
    #[test]
    fn test_is_active_revoked() {
        let key = make_key(2, "revoked-key", Some("2025-01-01T00:00:00Z"), None);
        assert!(!is_active(&key));
    }

    /// Test 3: Expired key (past expires_at) → false
    #[test]
    fn test_is_active_expired() {
        let key = make_key(3, "expired-key", None, Some("2020-01-01T00:00:00Z"));
        assert!(!is_active(&key));
    }

    /// Test 4: Future expiry → true
    #[test]
    fn test_is_active_future_expiry() {
        let key = make_key(4, "future-expiry-key", None, Some("2099-12-31T23:59:59Z"));
        assert!(is_active(&key));
    }

    /// Test 5: Malformed expires_at → true (treat as active)
    #[test]
    fn test_is_active_malformed_expiry() {
        let key = make_key(5, "malformed-key", None, Some("not-a-date"));
        assert!(is_active(&key));
    }
}

/// Main Keys page component.
/// Displays a card-based list of API keys with create/edit/revoke functionality.
#[component]
pub fn KeysPage() -> impl IntoView {
    let keys = RwSignal::new(Vec::<ApiKey>::new());
    let show_revoked = RwSignal::new(false);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let save_status = RwSignal::new(Option::<(bool, String)>::None);
    let show_create = RwSignal::new(false);
    let show_key_created = RwSignal::new(Option::<CreateKeyResponse>::None);

    // Load keys on mount
    let keys_c = keys;
    let loading_c = loading;
    let error_c = error;
    wasm_bindgen_futures::spawn_local(async move {
        loading_c.set(true);
        error_c.set(None);

        match fetch_keys().await {
            Ok(list) => keys_c.set(list),
            Err(e) => error_c.set(Some(format!("Failed to load API keys: {}", e))),
        }

        loading_c.set(false);
    });

    // Callback to close the key-created modal and refresh the list
    let close_and_refresh = {
        let keys_c = keys;
        let save_status_c = save_status;
        Callback::new(move |_| {
            show_key_created.set(None);
            let keys_c = keys_c;
            let save_status_c = save_status_c;
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_keys().await {
                    Ok(list) => keys_c.set(list),
                    Err(e) => {
                        save_status_c
                            .set(Some((false, format!("Failed to refresh key list: {}", e))));
                    }
                }
            });
        })
    };

    view! {
        <div class="page">
            <div class="page-header">
                <h1>"🔑 API Keys"</h1>
                <button
                    class="btn btn-primary"
                    on:click=move |_| show_create.set(true)
                >
                    "+ New Key"
                </button>
            </div>
            <p class="page-header__subtitle">"Manage API keys for programmatic access."</p>

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
                            <span class="spinner">"Loading API keys..."</span>
                        </div>
                    }
                    .into_any()
                })
            }}

            // Error state
            {move || error.get().map(|e| view! {
                <AlertBanner variant=AlertVariant::Error>{e}</AlertBanner>
            }.into_any())}

            // Filter toggle row
            <div class="keys-filter-row">
                <label>
                    <input
                        type="checkbox"
                        prop:checked=move || !show_revoked.get()
                        on:change=move |ev| {
                            use wasm_bindgen::JsCast;
                            if let Some(checked) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                show_revoked.set(!checked.checked());
                            }
                        }
                    />
                    "Active only"
                </label>
            </div>

            // Empty state
            {move || {
                (!loading.get() && keys.get().is_empty()).then(|| {
                    view! {
                        <div class="card card--centered">
                            <p class="text-muted">"No API keys yet. Click + New Key to create one."</p>
                        </div>
                    }
                    .into_any()
                })
            }}

            // Key card list
            {move || {
                let items = keys.get();
                let show_revoked_val = show_revoked.get();
                let filtered: Vec<_> = items
                    .into_iter()
                    .filter(|k| show_revoked_val || is_active(k))
                    .collect();

                if !filtered.is_empty() {
                    view! {
                        <div class="keys-list">
                            {filtered.into_iter().map(|key| {
                                view! {
                                    <KeyCard
                                        key=key
                                        keys=keys
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
                title="Create API Key".to_string()
            >
                <CreateKeyForm
                    keys=keys
                    save_status=save_status
                    on_key_created=Callback::new(move |resp: CreateKeyResponse| {
                        show_key_created.set(Some(resp));
                    })
                    on_close=Callback::new(move |_| show_create.set(false))
                />
            </Modal>

            // Key created modal
            <KeyCreatedModal
                show=show_key_created
                on_close=close_and_refresh
            />
        </div>
    }
}

/// Component for the key-created modal.
/// Uses a signal to hold the response, avoiding nested closure capture issues.
#[component]
fn KeyCreatedModal(
    show: RwSignal<Option<CreateKeyResponse>>,
    on_close: Callback<()>,
) -> impl IntoView {
    // Derive open state directly from show — no extra RwSignal needed
    let modal_open = Signal::derive(move || show.get().is_some());

    let on_close_wrapped = Callback::new(move |_| {
        show.set(None);
        on_close.run(());
    });

    view! {
        <Modal
            open=modal_open
            on_close=on_close_wrapped
            title="🔑 Key Created".to_string()
        >
            {move || show.get().map(|resp| {
                key_created_modal_content(resp)
            })}
        </Modal>
    }
}

/// Renders the content of the key-created modal.
/// This is a free function (not a component) to avoid closure capture issues.
fn key_created_modal_content(resp: CreateKeyResponse) -> impl IntoView {
    let key_text = resp.key.clone();
    let created_name = resp.name.clone();
    let created_scopes = resp.scopes.clone();
    let created_expires = resp.expires_at.clone();

    let copied = RwSignal::new(false);

    let key_text_for_span = key_text.clone();
    let handle_copy = move |_| {
        let key = key_text.clone();
        let copied_c = copied;
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(navigator) = web_sys::window().map(|w| w.navigator()) {
                let _ = navigator.clipboard().write_text(&key).await;
            }
            copied_c.set(true);
            gloo_timers::future::TimeoutFuture::new(2_000).await;
            copied_c.set(false);
        });
    };

    view! {
        <div class="key-created-modal">
            <p class="text-warning">"Copy this key now — it will never be shown again!"</p>

            <div class="key-created-modal__key-box">
                <span class="key-created-modal__key-text">{key_text_for_span}</span>
                <button class="btn btn-sm btn-secondary" on:click=handle_copy>
                    {move || if copied.get() { "Copied!" } else { "Copy" }}
                </button>
            </div>

            <div class="key-created-modal__summary">
                <div class="form-group">
                    <label>"Name"</label>
                    <span>{created_name}</span>
                </div>
                <div class="form-group">
                    <label>"Scopes"</label>
                    <div>
                        {created_scopes.iter().map(|s| {
                            view! { <span class="badge-pill">{s.clone()}</span> }.into_any()
                        }).collect::<Vec<_>>()}
                    </div>
                </div>
                {move || created_expires.as_ref().map(|exp| {
                    let formatted = chrono::DateTime::parse_from_rfc3339(exp)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                        .unwrap_or_else(|_| exp.clone());
                    view! {
                        <div class="form-group">
                            <label>"Expires"</label>
                            <span>{formatted}</span>
                        </div>
                    }.into_any()
                })}
            </div>
        </div>
    }
}

/// Individual key card component.
#[component]
fn KeyCard(
    key: ApiKey,
    keys: RwSignal<Vec<ApiKey>>,
    save_status: RwSignal<Option<(bool, String)>>,
) -> impl IntoView {
    let key_id = key.id;
    let key_name = key.name.clone();
    let key_prefix = key.key_prefix.clone();
    let key_scopes = key.scopes.clone();
    let show_edit = RwSignal::new(false);
    let key_active = is_active(&key);

    let key_name_for_title = key_name.clone();
    let key_name_for_delete = key_name.clone();
    let key_prefix_actions = key_prefix.clone();
    let key_scopes_actions = key_scopes.clone();
    let key_active_actions = key_active;

    view! {
        <div class=if key_active { "".to_string() } else { "key-card--dimmed".to_string() }>
            <ListCard
                state=Some(RwSignal::new(if key_active { None } else { Some("revoked".to_string()) }).read_only())
                icon=Some(Box::new(move || view! {
                    <span class="key-card__icon">"🔑"</span>
                }.into_any()))
                actions=Some(Box::new(move || view! {
                    // Edit button
                    <button class="btn-icon" title="Edit scopes" on:click=move |_| show_edit.set(true)>
                        "✏️"
                    </button>
                    // Revoke button
                    <button class="btn-icon btn-icon--danger" title="Revoke" on:click=move |_| {
                        let name_for_confirm = key_name_for_delete.clone();
                        let confirmed = web_sys::window()
                            .and_then(|w| w.confirm_with_message(&format!("Revoke key \"{}\"? This cannot be undone.", name_for_confirm)).ok())
                            .unwrap_or(false);
                        if confirmed {
                            let key_id_c = key_id;
                            let keys_c = keys;
                            let save_status_c = save_status;
                            wasm_bindgen_futures::spawn_local(async move {
                                match revoke_key(key_id_c).await {
                                    Ok(()) => {
                                        let mut list = keys_c.get_untracked();
                                        // Mark revoked in-place (soft delete) so it stays visible when "show all" is on
                                        if let Some(k) = list.iter_mut().find(|k| k.id == key_id_c) {
                                            k.revoked_at = Some(chrono::Utc::now().to_rfc3339());
                                        }
                                        keys_c.set(list);
                                        save_status_c.set(Some((true, format!("Key \"{}\" revoked.", name_for_confirm))));
                                    }
                                    Err(e) => {
                                        save_status_c.set(Some((false, format!("Failed to revoke key: {}", e))));
                                    }
                                }
                            });
                        }
                    }>
                        "🗑️"
                    </button>
                }.into_any()))
                line2=Some(Box::new(move || {
                    view! {
                        <span class="key-card__prefix">{key_prefix_actions.clone()}</span>
                        {key_scopes_actions.iter().map(|scope| {
                            view! { <span class="badge-pill">{scope.clone()}</span> }.into_any()
                        }).collect::<Vec<_>>()}
                        // Status badge
                        {if !key_active_actions {
                            view! {
                                <span class="badge-pill badge-pill--danger">"Revoked"</span>
                            }.into_any()
                        } else {
                            view! { <span/> }.into_any()
                        }}
                    }
                }.into_any()))
            >
                <span class="key-card__name">{key_name.clone()}</span>
            </ListCard>

            // Edit modal
            <Modal
                open=rw_signal_to_signal(show_edit)
                on_close=Callback::new(move |_| show_edit.set(false))
                title=move || format!("Edit: {}", key_name_for_title)
            >
                <EditKeyForm
                    key=key.clone()
                    keys=keys
                    save_status=save_status
                    on_close=Callback::new(move |_| show_edit.set(false))
                />
            </Modal>
        </div>
    }
}

/// Form for creating a new API key.
#[component]
fn CreateKeyForm(
    keys: RwSignal<Vec<ApiKey>>,
    save_status: RwSignal<Option<(bool, String)>>,
    on_key_created: Callback<CreateKeyResponse>,
    on_close: Callback<()>,
) -> impl IntoView {
    let name = RwSignal::new(String::new());
    let scopes = RwSignal::new(vec![AVAILABLE_SCOPES[0].0.to_string()]); // Default: first scope checked
    let expires_at = RwSignal::new(String::new());
    let submit_error = RwSignal::new(Option::<String>::None);

    let handle_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let name_val = name.get().trim().to_string();
        let scopes_val = scopes.get();
        let expires_val = expires_at.get();

        // Validate name
        if name_val.is_empty() {
            submit_error.set(Some("Key name is required.".to_string()));
            return;
        }

        // Validate at least one scope
        if scopes_val.is_empty() {
            submit_error.set(Some("Select at least one scope.".to_string()));
            return;
        }

        // Format expires_at: append ":00Z" if non-empty
        let expires_formatted = if expires_val.is_empty() {
            None
        } else {
            Some(format!("{}:00Z", expires_val))
        };

        submit_error.set(None);
        let keys_c = keys;
        let save_status_c = save_status;
        let on_key_created_c = on_key_created;
        let on_close_c = on_close;
        wasm_bindgen_futures::spawn_local(async move {
            match create_key(&name_val, &scopes_val, expires_formatted).await {
                Ok(resp) => {
                    // Extract key_prefix from the plaintext key (tama_ + first 8 chars of random)
                    let key_prefix = if let Some(random) = resp.key.strip_prefix("tama_") {
                        format!("tama_{}", &random[..8.min(random.len())])
                    } else {
                        resp.key.clone()
                    };

                    // Add to list — close_and_refresh will refetch for complete metadata
                    let mut list = keys_c.get_untracked();
                    list.push(ApiKey {
                        id: resp.id,
                        name: resp.name.clone(),
                        key_prefix,
                        scopes: resp.scopes.clone(),
                        created_by: "—".to_string(), // placeholder — refetch fills real value
                        created_at: resp.created_at.clone(),
                        last_used_at: None,
                        revoked_at: None,
                        expires_at: resp.expires_at.clone(),
                    });
                    keys_c.set(list);
                    on_key_created_c.run(resp);
                    on_close_c.run(());
                }
                Err(e) => {
                    submit_error.set(Some(format!("Failed to create key: {}", e)));
                    save_status_c.set(Some((false, format!("Failed to create key: {}", e))));
                }
            }
        });
    };

    view! {
        <form on:submit=handle_submit>
            <div class="form-group">
                <label for="key-name">"Key Name"</label>
                <input
                    id="key-name"
                    type="text"
                    placeholder="e.g. ci-deploy-key"
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(target_value(&ev))
                    autofocus=true
                />
            </div>

            <div class="form-group">
                <label>"Scopes"</label>
                <div class="form-scope-checks">
                    {AVAILABLE_SCOPES.iter().map(|(scope, label)| {
                        let scope_str = scope.to_string();
                        let label_str = label.to_string();
                        let scope_for_change = scope_str.clone();
                        view! {
                            <label class="form-scope-check">
                                <input
                                    type="checkbox"
                                    prop:checked=move || scopes.get().contains(&scope_str)
                                    on:change=move |ev| {
                                        use wasm_bindgen::JsCast;
                                        if let Some(input) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                            let mut current = scopes.get_untracked();
                                            if input.checked() {
                                                if !current.contains(&scope_for_change) {
                                                    current.push(scope_for_change.clone());
                                                }
                                            } else {
                                                current.retain(|s| s != &scope_for_change);
                                            }
                                            scopes.set(current);
                                        }
                                    }
                                />
                                <span>{label_str}</span>
                            </label>
                        }
                    }).collect::<Vec<_>>()}
                </div>
            </div>

            <div class="form-group">
                <label for="key-expires">"Expires at (optional)"</label>
                <input
                    id="key-expires"
                    type="datetime-local"
                    prop:value=move || expires_at.get()
                    on:input=move |ev| expires_at.set(target_value(&ev))
                />
            </div>

            {move || submit_error.get().map(|e| view! { <div class="text-error">{e}</div> })}

            <div class="form-actions">
                <button type="button" class="btn btn-secondary" on:click=move |_| on_close.run(())>
                    "Cancel"
                </button>
                <button type="submit" class="btn btn-primary">
                    "Create Key"
                </button>
            </div>
        </form>
    }
}

/// Form for editing an existing key's scopes.
#[component]
fn EditKeyForm(
    key: ApiKey,
    keys: RwSignal<Vec<ApiKey>>,
    save_status: RwSignal<Option<(bool, String)>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let key_id = key.id;
    let key_name = key.name.clone();
    let key_name_status = key.name.clone();
    let key_prefix = key.key_prefix.clone();
    let created_by = key.created_by.clone();
    let created_at = key.created_at.clone();
    let last_used_at = key.last_used_at.clone();
    let last_used_label: String = last_used_at.as_deref().unwrap_or("Never").to_string();

    let scopes = RwSignal::new(key.scopes.clone());
    let submit_error = RwSignal::new(Option::<String>::None);

    let handle_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let scopes_val = scopes.get();

        if scopes_val.is_empty() {
            submit_error.set(Some("Select at least one scope.".to_string()));
            return;
        }

        submit_error.set(None);
        let key_id_c = key_id;
        let keys_c = keys;
        let save_status_c = save_status;
        let on_close_c = on_close;
        let key_name_status_c = key_name_status.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match update_key_scopes(key_id_c, &scopes_val).await {
                Ok(updated) => {
                    let mut list = keys_c.get_untracked();
                    if let Some(pos) = list.iter().position(|k| k.id == key_id_c) {
                        list[pos] = updated;
                    }
                    keys_c.set(list);
                    on_close_c.run(());
                    save_status_c.set(Some((
                        true,
                        format!("Key \"{}\" updated.", key_name_status_c),
                    )));
                }
                Err(e) => {
                    submit_error.set(Some(format!("Failed to update key: {}", e)));
                    save_status_c.set(Some((false, format!("Failed to update key: {}", e))));
                }
            }
        });
    };

    view! {
        <form on:submit=handle_submit>
            <div class="form-group">
                <label>"Key Name"</label>
                <span>{key_name.clone()}</span>
            </div>

            <div class="form-group">
                <label>"Key Prefix"</label>
                <span class="key-card__prefix">{key_prefix.clone()}</span>
            </div>

            <div class="form-group">
                <label>"Scopes"</label>
                <div class="form-scope-checks">
                    {AVAILABLE_SCOPES.iter().map(|(scope, label)| {
                        let scope_str = scope.to_string();
                        let label_str = label.to_string();
                        let scope_for_change = scope_str.clone();
                        view! {
                            <label class="form-scope-check">
                                <input
                                    type="checkbox"
                                    prop:checked=move || scopes.get().contains(&scope_str)
                                    on:change=move |ev| {
                                        use wasm_bindgen::JsCast;
                                        if let Some(input) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                            let mut current = scopes.get_untracked();
                                            if input.checked() {
                                                if !current.contains(&scope_for_change) {
                                                    current.push(scope_for_change.clone());
                                                }
                                            } else {
                                                current.retain(|s| s != &scope_for_change);
                                            }
                                            scopes.set(current);
                                        }
                                    }
                                />
                                <span>{label_str}</span>
                            </label>
                        }
                    }).collect::<Vec<_>>()}
                </div>
            </div>

            <div class="form-group">
                <label>"Created by"</label>
                <span>{created_by.clone()}</span>
            </div>

            <div class="form-group">
                <label>"Created at"</label>
                <span>{created_at.clone()}</span>
            </div>

            <div class="form-group">
                <label>"Last used"</label>
                <span>{last_used_label}</span>
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
