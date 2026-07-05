mod advanced_form;
mod api;
mod files_form;
mod hardware_form;
mod sampling_form;
mod sections;
mod settings_form;
mod types;

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::components::modal::Modal;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
use crate::utils::rw_signal_to_signal;

use self::advanced_form::ModelEditorAdvancedForm;
use self::api::*;
use self::files_form::ModelEditorFilesForm;
use self::hardware_form::ModelEditorHardwareForm;
use self::sampling_form::ModelEditorSamplingForm;
use self::settings_form::ModelEditorSettingsForm;
use self::types::*;

// ── Component ─────────────────────────────────────────────────────────────────

use self::sections::Section;

#[component]
pub fn ModelEditor() -> impl IntoView {
    let params = use_params_map();
    let model_id = move || params.get().get("id").unwrap_or_default();
    let is_new = move || model_id() == "new";

    let detail: LocalResource<Option<ModelDetail>> = LocalResource::new(move || {
        let id = model_id();
        async move { fetch_model(id).await }
    });

    // Refresh trigger for templates LocalResource
    let templates_refresh = RwSignal::new(0u32);

    // Use LocalResource for templates
    let templates: LocalResource<Option<std::collections::HashMap<String, serde_json::Value>>> =
        LocalResource::new(move || {
            let _ = templates_refresh.get();
            async move { fetch_sampling_templates().await }
        });

    // Consolidated form signal
    let form = RwSignal::new(Option::<ModelForm>::None);

    // UI-only signals (not part of form)
    let backends = RwSignal::new(Vec::<BackendOption>::new());
    let original_id = RwSignal::new(String::new());
    let pull_modal_open_signal = RwSignal::new(false);

    // Status
    let save_status = RwSignal::new(Option::<(bool, String)>::None);
    let deleted = RwSignal::new(false);

    // Repo-level DB metadata (from Phase 3 API enrichment)
    let repo_commit_sha = RwSignal::new(Option::<String>::None);
    let repo_pulled_at = RwSignal::new(Option::<String>::None);

    // Status for refresh / verify actions (busy flag + last message)
    let refresh_busy = RwSignal::new(false);
    let verify_busy = RwSignal::new(false);
    let refresh_status = RwSignal::new(Option::<(bool, String)>::None);
    let verify_status = RwSignal::new(Option::<(bool, String)>::None);

    // Active navigation section
    let active_section = RwSignal::new(Section::Settings);

    // Active preset name (tracks which preset was last loaded)
    let active_preset = RwSignal::new(String::new());

    // Dirty tracking via snapshot comparison
    let last_saved_form = RwSignal::new(Option::<String>::None);
    let is_dirty = Signal::derive(move || {
        let current = serde_json::to_string(&form.get()).ok();
        current != last_saved_form.get()
    });

    // Tracks whether the form has been populated from the loaded model detail.
    // Used to gate the layout render without depending on form.get() (which changes on every keystroke).
    let form_ready = RwSignal::new(false);

    // Populate signals when resource loads
    Effect::new(move |_| {
        if let Some(guard) = detail.get() {
            if let Some(d) = guard.take() {
                backends.set(d.backends.clone());
                original_id.set(d.id.to_string());

                // Build consolidated form
                let mut sampling_fields = std::collections::HashMap::new();
                if let Some(sampling_json) = &d.sampling {
                    if let Some(obj) = sampling_json.as_object() {
                        if let Some(temp) = obj.get("temperature") {
                            if let Some(val) = temp.as_f64() {
                                sampling_fields.insert(
                                    "temperature".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(top_k) = obj.get("top_k") {
                            if let Some(val) = top_k.as_u64() {
                                sampling_fields.insert(
                                    "top_k".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(top_p) = obj.get("top_p") {
                            if let Some(val) = top_p.as_f64() {
                                sampling_fields.insert(
                                    "top_p".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(min_p) = obj.get("min_p") {
                            if let Some(val) = min_p.as_f64() {
                                sampling_fields.insert(
                                    "min_p".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(presence) = obj.get("presence_penalty") {
                            if let Some(val) = presence.as_f64() {
                                sampling_fields.insert(
                                    "presence_penalty".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(frequency) = obj.get("frequency_penalty") {
                            if let Some(val) = frequency.as_f64() {
                                sampling_fields.insert(
                                    "frequency_penalty".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(repeat_pen) = obj.get("repeat_penalty") {
                            if let Some(val) = repeat_pen.as_f64() {
                                sampling_fields.insert(
                                    "repeat_penalty".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                    }
                }

                // Initialize modalities if absent so checkboxes have stable structure
                let mut modalities = d.modalities.clone();
                if modalities.is_none() {
                    modalities = Some(ModelModalities {
                        input: Vec::new(),
                        output: Vec::new(),
                    });
                }

                // Parse spec_decoding from ModelDetail
                let spec_decoding = if let Some(sd_json) = &d.spec_decoding {
                    serde_json::from_value(sd_json.clone()).unwrap_or_default()
                } else {
                    SpecDecodingForm::default()
                };

                form.set(Some(ModelForm {
                    id: d.id.to_string(),
                    backend: d.backend.clone(),
                    gpu_variant: d.gpu_variant.clone(),
                    gpu_device: d.gpu_device.clone(),
                    model: d.model,
                    quant: d.quant,
                    mmproj: d.mmproj,
                    mtp_model: d.mtp_model,
                    args: d.args.join("\n"),
                    sampling: sampling_fields,
                    enabled: d.enabled,
                    context_length: d.context_length,
                    num_parallel: d.num_parallel,
                    kv_unified: d.kv_unified,
                    port: d.port,
                    api_name: d.api_name.clone(),
                    display_name: d.display_name.clone(),
                    gpu_layers: d.gpu_layers,
                    cache_type_k: d.cache_type_k,
                    cache_type_v: d.cache_type_v,
                    hf_context_length: d.hf_context_length,
                    quants: d.quants.clone(),
                    modalities,
                    spec_decoding,
                }));

                repo_commit_sha.set(d.repo_commit_sha.clone());
                repo_pulled_at.set(d.repo_pulled_at.clone());
                // Seed last_saved_form so is_dirty starts as false (not "unsaved" on load)
                last_saved_form.set(serde_json::to_string(&form.get()).ok());
                form_ready.set(true);
            }
        }
    });

    let load_preset_action: Action<String, (), LocalStorage> =
        Action::new_unsync(move |preset_name: &String| {
            let preset_name_clone = preset_name.clone();
            async move {
                let templates_map: Option<std::collections::HashMap<String, serde_json::Value>> =
                    templates.get().and_then(|g| (*g).clone());
                if let Some(templates_map) = templates_map {
                    if let Some(preset) = templates_map.get(&preset_name_clone) {
                        if let Some(obj) = preset.as_object() {
                            form.update(|f| {
                                if let Some(form) = f {
                                    if let Some(temp) = obj.get("temperature") {
                                        if let Some(val) = temp.as_f64() {
                                            form.sampling
                                                .entry("temperature".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(top_k) = obj.get("top_k") {
                                        if let Some(val) = top_k.as_u64() {
                                            form.sampling
                                                .entry("top_k".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(top_p) = obj.get("top_p") {
                                        if let Some(val) = top_p.as_f64() {
                                            form.sampling
                                                .entry("top_p".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(min_p) = obj.get("min_p") {
                                        if let Some(val) = min_p.as_f64() {
                                            form.sampling
                                                .entry("min_p".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(presence) = obj.get("presence_penalty") {
                                        if let Some(val) = presence.as_f64() {
                                            form.sampling
                                                .entry("presence_penalty".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(frequency) = obj.get("frequency_penalty") {
                                        if let Some(val) = frequency.as_f64() {
                                            form.sampling
                                                .entry("frequency_penalty".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(repeat_pen) = obj.get("repeat_penalty") {
                                        if let Some(val) = repeat_pen.as_f64() {
                                            form.sampling
                                                .entry("repeat_penalty".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
                active_preset.set(preset_name_clone);
            }
        });

    // Save preset action — collects enabled sampling values and persists via config API
    let save_preset_action: Action<String, (), LocalStorage> =
        Action::new_unsync(move |name: &String| {
            let name_clone = name.clone();
            async move {
                let form_val = form.get();
                let Some(initial_form) = form_val else {
                    save_status.set(Some((false, "❌ Form not loaded.".into())));
                    return;
                };

                // Collect enabled sampling values into a JSON object
                let mut map = serde_json::Map::new();
                for (key, field) in &initial_form.sampling {
                    if field.enabled {
                        let val: serde_json::Value = if let Ok(v) = field.value.parse::<f64>() {
                            serde_json::json!(v)
                        } else if let Ok(v) = field.value.parse::<u64>() {
                            serde_json::json!(v)
                        } else {
                            serde_json::json!(field.value)
                        };
                        map.insert(key.clone(), val);
                    }
                }

                match save_sampling_template(&name_clone, &serde_json::Value::Object(map)).await {
                    Ok(()) => {
                        save_status.set(Some((true, "✅ Preset saved".into())));
                        active_preset.set(name_clone.clone());
                        templates_refresh.update(|n| *n += 1);
                        // Clear success message after 2s so dirty indicator can reappear
                        let status = save_status;
                        wasm_bindgen_futures::spawn_local(async move {
                            gloo_timers::future::sleep(std::time::Duration::from_secs(2)).await;
                            status.set(None);
                        });
                    }
                    Err(e) => {
                        save_status.set(Some((false, format!("❌ Preset save failed: {}", e))));
                    }
                }
            }
        });
    // Actions
    let save_action: Action<(), (), LocalStorage> = Action::new_unsync(move |_: &()| {
        let form_val = form.get();
        let original_id_val = original_id.get();
        let is_new_val = is_new();

        async move {
            let Some(initial_form) = form_val else {
                save_status.set(Some((false, "❌ Form not loaded.".into())));
                return;
            };

            // Ensure form_id is set to original_id if empty (prevents creating new models)
            let save_id = if initial_form.id.trim().is_empty() {
                original_id_val.clone()
            } else {
                initial_form.id.clone()
            };

            let args: Vec<String> = initial_form
                .args
                .lines()
                .map(|l: &str| l.trim().to_string())
                .filter(|l: &String| !l.is_empty())
                .collect();

            let form_data = ModelForm {
                id: save_id,
                backend: initial_form.backend.clone(),
                gpu_variant: initial_form.gpu_variant.clone(),
                gpu_device: initial_form.gpu_device.clone(),
                model: initial_form.model.clone(),
                quant: initial_form.quant.clone(),
                mmproj: initial_form.mmproj.clone(),
                mtp_model: initial_form.mtp_model.clone(),
                args: initial_form.args.clone(),
                sampling: initial_form.sampling.clone(),
                enabled: initial_form.enabled,
                context_length: initial_form.context_length,
                num_parallel: initial_form.num_parallel,
                kv_unified: initial_form.kv_unified,
                port: initial_form.port,
                api_name: initial_form.api_name.clone(),
                display_name: initial_form.display_name.clone(),
                gpu_layers: initial_form.gpu_layers,
                cache_type_k: initial_form.cache_type_k,
                cache_type_v: initial_form.cache_type_v,
                hf_context_length: initial_form.hf_context_length,
                quants: initial_form.quants.clone(),
                modalities: initial_form.modalities.clone(),
                spec_decoding: initial_form.spec_decoding.clone(),
            };

            let new_id = form_data.id.clone();
            let old_id = original_id_val;

            if old_id != new_id && !old_id.is_empty() {
                match rename_model(&old_id, &new_id).await {
                    Ok(()) => (),
                    Err(e) => {
                        save_status.set(Some((false, format!("❌ Rename failed: {}", e))));
                        return;
                    }
                }
            }

            let form_id = form_data.id.clone();
            match save_model(args, form_data, is_new_val).await {
                Ok(()) => {
                    original_id.set(form_id);
                    save_status.set(Some((true, "✅ Saved".into())));
                    last_saved_form.set(serde_json::to_string(&form.get()).ok());
                    // Clear success message after 2s so dirty indicator can reappear
                    let status = save_status;
                    wasm_bindgen_futures::spawn_local(async move {
                        gloo_timers::future::sleep(std::time::Duration::from_secs(2)).await;
                        status.set(None);
                    });
                }
                Err(e) => {
                    if old_id != new_id && !old_id.is_empty() {
                        match rename_model(&new_id, &old_id).await {
                            Ok(()) => {
                                original_id.set(old_id.clone());
                                save_status.set(Some((
                                    false,
                                    format!("❌ Save failed, rolled back: {}", e),
                                )));
                            }
                            Err(rename_err) => {
                                save_status.set(Some((
                                    false,
                                    format!(
                                        "❌ Save failed ({}), and rollback also failed ({})",
                                        e, rename_err
                                    ),
                                )));
                            }
                        }
                    } else {
                        save_status.set(Some((false, format!("❌ Error: {}", e))));
                    }
                }
            }
        }
    });

    let delete_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            let form_opt = form.get();
            let Some(form) = form_opt else {
                save_status.set(Some((false, "❌ Form not loaded.".into())));
                return;
            };
            match delete_model_api(form.id.clone()).await {
                Ok(()) => deleted.set(true),
                Err(e) => save_status.set(Some((false, format!("❌ Delete failed: {}", e)))),
            }
        });

    let delete_quant_action: Action<(String, String), (), LocalStorage> =
        Action::new_unsync(move |(id, key): &(String, String)| {
            let id = id.clone();
            let key = key.clone();
            async move {
                match delete_quant_api(id.clone(), key.clone()).await {
                    Ok(()) => {
                        // Remove from local state on success
                        form.update(|f| {
                            if let Some(form) = f {
                                form.quants.retain(|k, _| k != &key);
                                // Clear form.quant if matching
                                if form.quant.as_deref() == Some(key.as_str()) {
                                    form.quant = None;
                                }
                                // Clear mmproj if matching
                                if form.mmproj.as_deref() == Some(key.as_str()) {
                                    form.mmproj = None;
                                }
                                // Clear mtp_model if matching
                                if form.mtp_model.as_deref() == Some(key.as_str()) {
                                    form.mtp_model = None;
                                }
                            }
                        });
                        save_status.set(Some((true, "✅ Quant deleted from disk.".into())));
                    }
                    Err(e) => {
                        save_status.set(Some((false, format!("❌ Delete failed: {}", e))));
                    }
                }
            }
        });

    // Merge a list of DB file records back into the `quants` signal, matching
    // on `QuantInfo.file`. Only updates DB-enrichment fields; TOML fields
    // (name, kind, context_length) are left untouched.
    let merge_file_records = move |files: Vec<FileRecordJson>| {
        form.update(|f| {
            if let Some(form) = f {
                for rec in files {
                    for (_name, q) in form.quants.iter_mut() {
                        if q.file == rec.filename {
                            q.lfs_oid = rec.lfs_oid.clone();
                            q.db_size_bytes = rec.size_bytes;
                            // Authoritative size from HF blob metadata — update
                            // the visible size_bytes too, since the editable input
                            // is now read-only.
                            if rec.size_bytes.is_some() {
                                q.size_bytes = rec.size_bytes;
                            }
                            q.last_verified_at = rec.last_verified_at.clone();
                            q.verified_ok = rec.verified_ok;
                            q.verify_error = rec.verify_error.clone();
                            break;
                        }
                    }
                }
            }
        });
    };

    let refresh_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            refresh_busy.set(true);
            refresh_status.set(None);
            // Use the persisted id, not the editable form_id — otherwise
            // mid-rename edits would cause the backend to look up a model
            // that isn't saved yet.
            let persisted = original_id.get_untracked();
            let id = if persisted.is_empty() {
                form.get_untracked().map(|f| f.id).unwrap_or_default()
            } else {
                persisted
            };
            match refresh_model_api(id).await {
                Ok(resp) => {
                    repo_commit_sha.set(resp.repo_commit_sha.clone());
                    repo_pulled_at.set(resp.repo_pulled_at.clone());
                    let n = resp.files.len();
                    merge_file_records(resp.files);
                    refresh_status.set(Some((
                        true,
                        format!("Refreshed metadata for {} file(s).", n),
                    )));
                }
                Err(e) => {
                    refresh_status.set(Some((false, format!("Refresh failed: {}", e))));
                }
            }
            refresh_busy.set(false);
        });

    let verify_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            verify_busy.set(true);
            verify_status.set(None);
            // Same reasoning as refresh_action: target the saved id.
            let persisted = original_id.get_untracked();
            let id = if persisted.is_empty() {
                form.get_untracked().map(|f| f.id).unwrap_or_default()
            } else {
                persisted
            };
            match verify_model_api(id).await {
                Ok(resp) => {
                    let n = resp.files.len();
                    merge_file_records(resp.files);
                    let msg = if resp.ok && !resp.any_unknown {
                        format!("All {} file(s) verified successfully.", n)
                    } else if resp.ok {
                        format!("Verified {} file(s) (some without an upstream hash).", n)
                    } else {
                        "Verification failed for one or more files.".to_string()
                    };
                    verify_status.set(Some((resp.ok, msg)));
                }
                Err(e) => {
                    verify_status.set(Some((false, format!("Verify failed: {}", e))));
                }
            }
            verify_busy.set(false);
        });

    // View
    view! {
        <div class="page-header">
            <h1>
                {move || {
                    if is_new() {
                        "New Model".to_string()
                    } else {
                        // Prefer display_name from form, fall back to model_id (which may be integer or config_key)
                        form.get()
                            .and_then(|f| f.display_name.clone())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(model_id)
                    }
                }}
            </h1>
        </div>

        {move || deleted.get().then(|| view! {
            <div class="alert alert--success mb-3">
                <span class="alert__icon">"✓"</span>
                <span>"Model deleted. " <A href="/tama/models">"← Back to Models"</A></span>
            </div>
        })}

        <Suspense fallback=|| view! {
            <div class="spinner-container">
                <span class="spinner"></span>
                <span class="text-muted">"Loading model..."</span>
            </div>
        }>
            {move || {
                // Use form_ready as the stability gate, NOT form.get().
                // form.get() changes on every keystroke, which would cause
                // the entire layout to unmount/remount and lose input focus.
                form_ready.get().then(|| {
                    view! {
                        <div class="model-editor-layout">
                            // Pill-style tab navigation
                            <div class="model-editor-pills">
                                <button
                                    class="model-editor-pill"
                                    class:model-editor-pill--active=move || active_section.get() == Section::Settings
                                    on:click=move |_| { active_section.set(Section::Settings); }
                                >
                                    <span>{Section::Settings.icon()}</span>
                                    <span>{Section::Settings.name()}</span>
                                </button>
                                <button
                                    class="model-editor-pill"
                                    class:model-editor-pill--active=move || active_section.get() == Section::Hardware
                                    on:click=move |_| { active_section.set(Section::Hardware); }
                                >
                                    <span>{Section::Hardware.icon()}</span>
                                    <span>{Section::Hardware.name()}</span>
                                </button>
                                <button
                                    class="model-editor-pill"
                                    class:model-editor-pill--active=move || active_section.get() == Section::Sampling
                                    on:click=move |_| { active_section.set(Section::Sampling); }
                                >
                                    <span>{Section::Sampling.icon()}</span>
                                    <span>{Section::Sampling.name()}</span>
                                </button>
                                <button
                                    class="model-editor-pill"
                                    class:model-editor-pill--active=move || active_section.get() == Section::Files
                                    on:click=move |_| { active_section.set(Section::Files); }
                                >
                                    <span>{Section::Files.icon()}</span>
                                    <span>{Section::Files.name()}</span>
                                </button>
                                <button
                                    class="model-editor-pill"
                                    class:model-editor-pill--active=move || active_section.get() == Section::Advanced
                                    on:click=move |_| { active_section.set(Section::Advanced); }
                                >
                                    <span>{Section::Advanced.icon()}</span>
                                    <span>{Section::Advanced.name()}</span>
                                </button>
                            </div>

                            // Tab content — only active tab renders
                            <div class="model-editor-main">
                                {match active_section.get() {
                                    Section::Settings => view! {
                                        <div class="card">
                                            <h2 class="card__title">"Settings"</h2>
                                            <ModelEditorSettingsForm
                                                form=form
                                                backends=backends
                                            />
                                        </div>
                                    }.into_any(),
                                    Section::Hardware => view! {
                                        <div class="card">
                                            <h2 class="card__title">"Hardware"</h2>
                                            <ModelEditorHardwareForm
                                                form=form
                                            />
                                        </div>
                                    }.into_any(),
                                    Section::Sampling => view! {
                                        <div class="card mt-2">
                                            <h2 class="card__title">"Sampling"</h2>
                                            <ModelEditorSamplingForm
                                                form=form
                                                templates=templates
                                                load_preset_action=load_preset_action
                                                active_preset=active_preset
                                                save_preset_action=save_preset_action
                                            />
                                        </div>
                                    }.into_any(),
                                    Section::Files => view! {
                                        <div class="card mt-2">
                                            <h2 class="card__title">"Files"</h2>
                                            <ModelEditorFilesForm
                                                form=form
                                                repo_commit_sha=repo_commit_sha
                                                repo_pulled_at=repo_pulled_at
                                                refresh_busy=refresh_busy
                                                verify_busy=verify_busy
                                                refresh_status=refresh_status
                                                verify_status=verify_status
                                                pull_modal_open_signal=pull_modal_open_signal
                                                delete_quant_action=delete_quant_action
                                                original_id=original_id
                                                refresh_action=refresh_action
                                                verify_action=verify_action
                                            />
                                        </div>
                                    }.into_any(),
                                    Section::Advanced => view! {
                                        <div class="card mt-2">
                                            <h2 class="card__title">"Advanced"</h2>
                                            <ModelEditorAdvancedForm form=form />
                                        </div>
                                    }.into_any(),
                                }}
                            </div>

                            // Sticky save bar
                            <div class="model-editor-save-bar">
                                <div class="model-editor-save-bar__left">
                                    <A href="/tama/models" attr:class="btn btn-secondary btn-sm">"← Back to Models"</A>
                                </div>
                                <div class="model-editor-save-bar__center">
                                    {move || {
                                        if let Some((ok, msg)) = save_status.get() {
                        let cls = if ok {
                            "model-editor-save-bar__status model-editor-save-bar__status--saved"
                        } else {
                            "model-editor-save-bar__status"
                        };
                                            Some(view! { <span class=cls>{msg}</span> }.into_any())
                                        } else if is_dirty.get() {
                                            Some(view! { <span class="model-editor-save-bar__status model-editor-save-bar__status--dirty">"● Unsaved changes"</span> }.into_any())
                                        } else {
                                            None
                                        }
                                    }}
                                </div>
                                <div class="model-editor-save-bar__right">
                                    <button
                                        type="button"
                                        class="btn btn-primary"
                                        on:click=move |_| { save_action.dispatch(()); }
                                    >
                                        "Save Model"
                                    </button>
                                    {move || (!is_new()).then(|| view! {
                                        <button
                                            type="button"
                                            class="btn btn-danger ml-2"
                                            on:click=move |_| {
                                                let confirmed = web_sys::window()
                                                    .and_then(|w| w.confirm_with_message("Delete this model and all its files from disk? This cannot be undone.").ok())
                                                    .unwrap_or(false);
                                                if confirmed { delete_action.dispatch(()); }
                                            }
                                        >"Delete Model"</button>
                                    })}
                                </div>
                            </div>
                        </div>
                    }.into_any()
                })
            }}
        </Suspense>

        <Modal
            open=rw_signal_to_signal(pull_modal_open_signal)
            on_close=Callback::new(move |_| pull_modal_open_signal.set(false))
            title="Pull Quant from HuggingFace".to_string()
        >
            <PullQuantWizard
                initial_repo=Signal::derive(move || form.get().map(|f| f.model.unwrap_or_default()).unwrap_or_default())
                is_open=rw_signal_to_signal(pull_modal_open_signal)
                on_complete=Callback::new(move |completed: Vec<CompletedQuant>| {
                    // Visibility for the silent-failure caveat in spec §8.7: if all
                    // quants in this session failed, log to console so the user has
                    // *some* trace after the modal auto-closes.
                    if completed.is_empty() {
                        web_sys::console::warn_1(
                            &"All pulled quants failed; nothing merged into the editor.".into(),
                        );
                    }
                    form.update(|f| {
                        if let Some(form) = f {
                            for cq in completed {
                                // Detect mmproj / mtp files by filename pattern (matches
                                // the backend's QuantKind::from_filename logic).
                                let lower = cq.filename.to_lowercase();
                                let kind = if lower.starts_with("mmproj") && lower.ends_with(".gguf") {
                                    QuantKind::Mmproj
                                } else if lower.starts_with("mtp") && lower.ends_with(".gguf") {
                                    QuantKind::Mtp
                                } else {
                                    QuantKind::Model
                                };
                                let key = cq.quant.clone().unwrap_or_else(|| {
                                    // Infer quant from filename: try standard patterns first,
                                    // otherwise use last component after splitting by `-` or `_`
                                    let stem = cq.filename.trim_end_matches(".gguf");
                                    let quant_patterns = [
                                        "IQ2_XXS", "IQ3_XXS", "IQ1_S", "IQ1_M", "IQ2_XS", "IQ2_S",
                                        "IQ2_M", "IQ3_XS", "IQ3_S", "IQ3_M", "IQ4_XS", "IQ4_NL",
                                        "Q2_K_S", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q4_K_S", "Q4_K_M",
                                        "Q4_K_L", "Q5_K_S", "Q5_K_M", "Q5_K_L", "Q2_K_XL", "Q3_K_XL",
                                        "Q4_K_XL", "Q5_K_XL", "Q6_K_XL", "Q8_K_XL", "Q2_K", "Q3_K",
                                        "Q4_K", "Q5_K", "Q6_K", "Q4_0", "Q4_1", "Q5_0", "Q5_1",
                                        "Q6_0", "Q8_0", "Q8_1", "F16", "F32", "BF16",
                                    ];
                                    let stem_upper = stem.to_uppercase();
                                    let quant = quant_patterns.iter().find(|pattern| {
                                        stem_upper.ends_with(*pattern)
                                            || stem_upper.contains(&format!("-{}", pattern))
                                            || stem_upper.contains(&format!(".{}", pattern))
                                            || stem_upper.contains(&format!("_{}", pattern))
                                    }).map(|s| s.to_string());
                                    quant.unwrap_or_else(|| {
                                        stem.split(|c: char| ['-', '_'].contains(&c))
                                            .next_back()
                                            .unwrap_or("unknown")
                                            .to_string()
                                    })
                                });
                                if let Some(pos) = form.quants.iter().position(|(k, _)| k == &key) {
                                    // Re-pull: overwrite filename.
                                    // Context length is model-level, populated from GGUF parsing.
                                    // Only overwrite size_bytes when we have a value —
                                    // never clobber a known size with None.
                                    let row = &mut form.quants.values_mut().nth(pos).unwrap();
                                    row.file = cq.filename;
                                    row.kind = kind;
                                    if cq.size_bytes.is_some() {
                                        row.size_bytes = cq.size_bytes;
                                    }
                                } else {
                                    // New row.
                                    // context_length will be populated from GGUF parsing during download.
                                    form.quants.insert(key, QuantInfo {
                                        file: cq.filename,
                                        kind,
                                        size_bytes: cq.size_bytes,
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                    });
                    pull_modal_open_signal.set(false);
                })
                on_close=Callback::new(move |_| pull_modal_open_signal.set(false))
            />
        </Modal>
    }
}
