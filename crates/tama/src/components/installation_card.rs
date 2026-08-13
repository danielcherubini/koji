//! Backend card component - displays a single installation with action buttons.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;

// ── DTOs (mirror of tama-web::api::backends DTOs) ────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct UpdateStatusDto {
    #[serde(default)]
    pub checked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_available: Option<bool>,
}

#[allow(dead_code)] // Used only by tests
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GpuVariantDto {
    Cuda { version: String },
    Vulkan,
    Metal,
    Rocm { version: String },
    CpuOnly,
    Custom,
}

impl GpuVariantDto {
    #[allow(dead_code)] // Used only by tests
    pub fn label(&self) -> String {
        match self {
            GpuVariantDto::Cuda { version } => format!("CUDA {version}"),
            GpuVariantDto::Vulkan => "Vulkan".to_string(),
            GpuVariantDto::Metal => "Metal".to_string(),
            GpuVariantDto::Rocm { version } => format!("ROCm {version}"),
            GpuVariantDto::CpuOnly => "CPU".to_string(),
            GpuVariantDto::Custom => "Custom".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InstallationSourceDto {
    Prebuilt {
        version: String,
    },
    SourceCode {
        version: String,
        git_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
}

impl InstallationSourceDto {
    /// Returns true if this installation was built from source code.
    pub fn is_source_code(&self) -> bool {
        matches!(self, InstallationSourceDto::SourceCode { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallationInfoDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_variant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<InstallationSourceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallationVersionDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_variant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<InstallationSourceDto>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallationCardDto {
    pub r#type: String,
    /// Actual DB key for the installation (used in save URLs). For native backends this equals `r#type`;
    /// for docker/custom backends it carries the actual name (e.g. "vllm").
    #[serde(default)]
    pub backend_name: String,
    pub display_name: String,
    pub installed: bool,
    /// GPU variant folder for this card (e.g. "cpu", "cuda_12", "vulkan").
    #[serde(default)]
    pub gpu_variant: String,
    /// Info for the currently selected version (default: active version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<InstallationInfoDto>,
    /// All installed versions of this installation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<InstallationVersionDto>,
    #[serde(default)]
    pub update: UpdateStatusDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_notes_url: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub default_env: Vec<String>,
    /// Whether the active version is currently selected for display.
    #[serde(default)]
    pub is_active: bool,
}

// ── Component ────────────────────────────────────────────────────────────────

/// InstallationCard - displays one installation with action buttons and a version selector.
#[component]
#[allow(dead_code)]
pub fn InstallationCard(
    installation: InstallationCardDto,
    /// Called with the installation type when "Install" is clicked.
    #[prop(optional)]
    on_install: Option<Callback<String>>,
    /// Called with (backend_name, gpu_variant) when "Update" is clicked.
    #[prop(optional)]
    on_update: Option<Callback<(String, String)>>,
    /// Called with (backend_name, gpu_variant) when "Check for updates" is clicked.
    #[prop(optional)]
    on_check_updates: Option<Callback<(String, String)>>,
    /// Called with (backend_name, gpu_variant) when "Uninstall" is clicked.
    #[prop(optional)]
    on_delete: Option<Callback<(String, String)>>,
    /// Called when default_args input changes with (backend_name, gpu_variant, value)
    #[prop(optional)]
    on_default_args_change: Option<Callback<(String, String, String)>>,
    /// Called when default_env input changes with (backend_name, gpu_variant, value)
    #[prop(optional)]
    on_default_env_change: Option<Callback<(String, String, String)>>,
    /// Called with (backend_name, version, gpu_variant) when version dropdown changes.
    #[prop(optional)]
    on_version_change: Option<Callback<(String, String, String)>>,
    /// Called with (backend_name, gpu_variant, build_from_source) when toggle changes.
    #[prop(optional)]
    on_build_method_change: Option<Callback<(String, String, bool)>>,
) -> impl IntoView {
    let type_install = installation.r#type.clone();

    let installed = installation.installed;
    let display_name = installation.display_name.clone();
    let gpu_variant = installation.gpu_variant.clone();
    let release_notes_url = installation.release_notes_url.clone();
    // Actual DB key for this installation (falls back to r#type for native backends)
    let backend_name = if installation.backend_name.is_empty() {
        installation.r#type.clone()
    } else {
        installation.backend_name.clone()
    };
    // Clone for use in multiple event closures below
    let backend_name_for_args = backend_name.clone();
    let backend_name_for_env = backend_name.clone();
    let backend_name_for_update = backend_name.clone();
    let backend_name_for_check = backend_name.clone();
    let backend_name_for_delete = backend_name.clone();
    let backend_name_for_build = backend_name.clone();
    let gpu_variant_for_args = gpu_variant.clone();
    let gpu_variant_for_env = gpu_variant.clone();
    let gpu_variant_for_update = gpu_variant.clone();
    let gpu_variant_for_check = gpu_variant.clone();
    let gpu_variant_for_delete = gpu_variant.clone();
    let gpu_variant_for_build = gpu_variant.clone();

    let update_available = installation.update.update_available.unwrap_or(false);
    let latest_version = installation.update.latest_version.clone();

    let default_args_initial = installation.default_args.join("\n");
    let default_args_signal = RwSignal::new(default_args_initial.clone());
    let default_env_initial = installation
        .default_env
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let default_env_signal = RwSignal::new(default_env_initial.clone());

    // All installed versions (sorted by installed_at DESC)
    let versions = installation.versions.clone();
    let version_count = versions.len();

    // Find the index of the active version to select it by default
    let active_index = versions.iter().position(|v| v.is_active).unwrap_or(0);

    // Track which version is currently selected for display
    let selected_version_idx = RwSignal::new(active_index);

    // Clone for selected info closure
    let versions_for_info = versions.clone();
    let selected_info = move || versions_for_info.get(selected_version_idx.get()).cloned();

    // Clone for active check closure
    let versions_for_active = versions.clone();
    let is_selected_active = move || {
        selected_version_idx.get() < version_count
            && versions_for_active[selected_version_idx.get()].is_active
    };

    // Build method toggle state
    let current_build_from_source = RwSignal::new(
        installation
            .info
            .as_ref()
            .and_then(|i| i.source.as_ref())
            .map(|s| s.is_source_code())
            .unwrap_or(false), // Default to prebuilt if no source recorded
    );

    // Whether toggle should be disabled (forced source)
    let force_source = installation.r#type == "ik_llama";

    // Whether to show the toggle at all (not for tts/custom, only when installed)
    let show_toggle = {
        let bt = installation.r#type.clone();
        installed && bt != "tts_kokoro" && bt != "custom"
    };

    view! {
        <fieldset style="border:1px solid var(--border,#ccc);padding:1rem;border-radius:6px;">
            <legend style="font-weight:600;display:flex;align-items:center;gap:0.5rem;flex-wrap:wrap;">
                <span>{display_name}</span>
                {if !gpu_variant.is_empty() {
                    let variant = gpu_variant.clone();
                    // Format variant for display: "cpu" -> "CPU", "cuda_12" -> "CUDA 12", "vulkan" -> "Vulkan"
                    let display_variant = variant.replace('_', " ").to_ascii_uppercase();
                    view! { <span class="badge" style="background:#7c3aed;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">{display_variant}</span> }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                {if !installed {
                    view! { <span class="badge" style="background:#94a3b8;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Not installed"</span> }.into_any()
                } else if is_selected_active() && version_count == 1 {
                    // Single installed version = it's the active one
                    view! { <span class="badge" style="background:#22c55e;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Active"</span> }.into_any()
                } else if is_selected_active() {
                    view! { <span class="badge" style="background:#22c55e;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Active"</span> }.into_any()
                } else {
                    view! { <span class="badge" style="background:#94a3b8;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Installed"</span> }.into_any()
                }}
                {if update_available {
                    view! { <span class="badge" style="background:#3b82f6;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Update available"</span> }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}

                {/* Version count badge when multiple versions exist */}
                {if version_count > 1 {
                    let count = version_count;
                    view! { <span class="badge" style="background:#64748b;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;">{format!("{} versions", count)}</span> }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
            </legend>

            {/* Version selector dropdown */}
            {if installed && version_count > 1 {
                let version_cb = on_version_change;
                view! {
                    <div style="display:flex;align-items:center;gap:0.5rem;margin-bottom:0.75rem;">
                        <label style="font-size:0.8125rem;font-weight:600;">"Version:"</label>
                        <select
                            class="form-select"
                            style="font-size:0.8125rem;padding:0.25rem 0.5rem;min-width:180px;"
                            prop:value=move || selected_version_idx.get().to_string()
                            on:change=move |ev| {
                                let value = crate::utils::target_value(&ev);
                                if let Ok(idx) = value.parse::<usize>() {
                                    selected_version_idx.set(idx);
                                    // Track version change as a pending edit
                                    if let Some(cb) = &version_cb {
                                        if idx < version_count {
                                            let ver = versions[idx].version.clone();
                                            let gv = versions[idx].gpu_variant.clone();
                                            cb.run((backend_name.clone(), ver, gv));
                                        }
                                    }
                                }
                            }
                        >
                            {versions.clone().iter().enumerate().map(|(i, v)| {
                                let label = if v.is_active {
                                    format!("{} (active)", v.version)
                                } else {
                                    v.version.clone()
                                };
                                view! {
                                    <option value=i.to_string()>{label}</option>
                                }.into_any()
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>
                }.into_any()
            } else {
                view! { <span/> }.into_any()
            }}

            <div style="display:flex;flex-direction:column;gap:0.5rem;">
                {/* Version info — derived from selected version */}
                {move || {
                    let info = selected_info();
                    view! {
                        {if let Some(ref v) = info {
                            let ver = v.version.clone();
                            view! { <div style="font-size:0.875rem;"><strong>"Version: "</strong>{ver}</div> }.into_any()
                        } else { view! { <span/> }.into_any() }}

                        {if let Some(ref v) = info {
                            let gpu_label = if v.gpu_variant.is_empty() {
                                "CPU".to_string()
                            } else {
                                v.gpu_variant.to_lowercase()
                            };
                            view! { <div style="font-size:0.875rem;"><strong>"GPU: "</strong>{gpu_label}</div> }.into_any()
                        } else { view! { <span/> }.into_any() }}

                        {if let Some(ref v) = info {
                            let path = v.path.clone();
                            view! { <div style="font-size:0.875rem;color:var(--muted,#666);"><strong>"Path: "</strong><code>{path}</code></div> }.into_any()
                        } else { view! { <span/> }.into_any() }}
                    }
                }}

                <div style="display:flex;gap:1rem;">
                    <div style="flex:1;min-width:0;">
                        <label style="font-size:0.875rem;font-weight:600;">"Default Args"</label>
                        <textarea
                            rows=4
                            placeholder="One arg per line\n--max-num-seqs 4\n--enable-prefix-caching"
                            style="font-size:0.875rem;padding:0.375rem;border:1px solid var(--border,#ccc);border-radius:4px;font-family:monospace;width:100%;resize:vertical;box-sizing:border-box;"
                            prop:value=move || default_args_signal.get()
                            on:input=move |ev| {
                                let value = crate::utils::target_value(&ev);
                                default_args_signal.set(value.clone());
                                if let Some(cb) = &on_default_args_change {
                                    cb.run((backend_name_for_args.clone(), gpu_variant_for_args.clone(), value));
                                }
                            }
                        ></textarea>
                    </div>
                    <div style="flex:1;min-width:0;">
                        <label style="font-size:0.875rem;font-weight:600;">"Environment Variables"</label>
                        <textarea
                            rows=4
                            placeholder="One variable per line\nKEY=value\nOTHER_VAR=123"
                            style="font-size:0.875rem;padding:0.375rem;border:1px solid var(--border,#ccc);border-radius:4px;font-family:monospace;width:100%;resize:vertical;box-sizing:border-box;"
                            prop:value=move || default_env_signal.get()
                            on:input=move |ev| {
                                let value = crate::utils::target_value(&ev);
                                default_env_signal.set(value.clone());
                                if let Some(cb) = &on_default_env_change {
                                    cb.run((backend_name_for_env.clone(), gpu_variant_for_env.clone(), value));
                                }
                            }
                        ></textarea>
                    </div>
                </div>

                {if update_available {
                    if let Some(lv) = latest_version {
                        view! { <div style="font-size:0.875rem;color:#3b82f6;"><strong>"Latest: "</strong>{lv}</div> }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                } else {
                    view! { <span/> }.into_any()
                }}
            </div>

            {/* Build method toggle */}
            {if show_toggle {
                let bn = backend_name_for_build.clone();
                let gv = gpu_variant_for_build.clone();
                let cb = on_build_method_change;
                let force = force_source;
                view! {
                    <div style="margin-top:0.75rem;">
                        <div class="form-check" style="display:flex;align-items:center;gap:0.5rem;">
                            <input
                                type="checkbox"
                                class="form-check-input"
                                prop:checked=move || current_build_from_source.get()
                                prop:disabled=move || force
                                on:change=move |e| {
                                    let checked = e.target()
                                        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                        .map(|el| el.checked())
                                        .unwrap_or(false);
                                    current_build_from_source.set(checked);
                                    if let Some(c) = &cb {
                                        c.run((bn.clone(), gv.clone(), checked));
                                    }
                                }
                            />
                            <span class="form-check-label" style="font-size:0.875rem;">"Build from source"</span>
                        </div>
                        {move || {
                            if force {
                                view! {
                                    <div style="font-size:0.75rem;color:var(--muted,#666);margin-top:0.125rem;margin-left:1.5rem;">
                                        "Always built from source — no prebuilt binaries"
                                    </div>
                                }.into_any()
                            } else {
                                view! { <span/> }.into_any()
                            }
                        }}
                    </div>
                }.into_any()
            } else {
                view! { <span/> }.into_any()
            }}

            <div style="display:flex;gap:0.5rem;margin-top:1rem;flex-wrap:wrap;">
                {/* Install button */}
                {if !installed {
                    let cb = on_install;
                    let bt = type_install.clone();
                    view! {
                        <button
                            type="button"
                            class="btn btn-primary"
                            on:click=move |_| {
                                if let Some(c) = cb {
                                    c.run(bt.clone());
                                }
                            }
                        >
                            "Install"
                        </button>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}

                {/* Check for updates — always when installed */}
                {if installed {
                    if let Some(cb) = on_check_updates {
                        let bn = backend_name_for_check.clone();
                        let gv = gpu_variant_for_check.clone();
                        view! {
                            <button
                                type="button"
                                class="btn btn-secondary"
                                on:click=move |_| {
                                    cb.run((bn.clone(), gv.clone()));
                                }
                            >
                                "Check for updates"
                            </button>
                        }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                } else {
                    view! { <span/> }.into_any()
                }}

                {/* Update button — only when update available */}
                {if installed && update_available {
                    let cb = on_update;
                    let bn = backend_name_for_update.clone();
                    let gv = gpu_variant_for_update.clone();
                    view! {
                        <button
                            type="button"
                            class="btn btn-primary"
                            on:click=move |_| {
                                if let Some(c) = cb {
                                    c.run((bn.clone(), gv.clone()));
                                }
                            }
                        >
                            "Update"
                        </button>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}

                {/* Uninstall — only when the selected version is active */}
                {move || {
                    if installed && is_selected_active() {
                        let cb = on_delete;
                        let bn = backend_name_for_delete.clone();
                        let gv = gpu_variant_for_delete.clone();
                        view! {
                            <button
                                type="button"
                                class="btn btn-secondary"
                                style="color:#dc2626;"
                                on:click=move |_| {
                                    if let Some(c) = cb {
                                        c.run((bn.clone(), gv.clone()));
                                    }
                                }
                            >
                                "Uninstall"
                            </button>
                        }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                }}

                {/* Release notes */}
                {if let Some(url) = release_notes_url {
                    view! {
                        <a
                            href=url
                            target="_blank"
                            rel="noopener noreferrer"
                            class="btn btn-secondary"
                            style="text-decoration:none;"
                        >
                            "Release notes"
                        </a>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
            </div>
        </fieldset>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_source_dto_is_source_code() {
        let prebuilt = InstallationSourceDto::Prebuilt {
            version: "1.0.0".to_string(),
        };
        assert!(!prebuilt.is_source_code());

        let source = InstallationSourceDto::SourceCode {
            version: "main".to_string(),
            git_url: "https://github.com/example/repo".to_string(),
            commit: Some("abc123".to_string()),
        };
        assert!(source.is_source_code());

        let source_no_commit = InstallationSourceDto::SourceCode {
            version: "main".to_string(),
            git_url: "https://github.com/example/repo".to_string(),
            commit: None,
        };
        assert!(source_no_commit.is_source_code());
    }

    #[test]
    fn test_gpu_variant_label() {
        assert_eq!(
            GpuVariantDto::Cuda {
                version: "12.4".to_string()
            }
            .label(),
            "CUDA 12.4"
        );
        assert_eq!(GpuVariantDto::Vulkan.label(), "Vulkan");
        assert_eq!(GpuVariantDto::CpuOnly.label(), "CPU");
    }

    #[test]
    fn test_backend_card_dto_serialization() {
        let dto = InstallationCardDto {
            r#type: "llama_cpp".to_string(),
            backend_name: "llama_cpp".to_string(),
            display_name: "llama.cpp".to_string(),
            installed: false,
            gpu_variant: String::new(),
            info: None,
            versions: vec![],
            update: UpdateStatusDto::default(),
            release_notes_url: Some("https://example.com".to_string()),
            default_args: vec![],
            default_env: vec![],
            is_active: false,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("llama_cpp"));
        assert!(json.contains("\"installed\":false"));
    }

    #[test]
    fn test_backend_card_dto_is_active_field() {
        let dto_active = InstallationCardDto {
            r#type: "llama_cpp".to_string(),
            backend_name: "llama_cpp".to_string(),
            display_name: "llama.cpp".to_string(),
            installed: true,
            gpu_variant: "cpu".to_string(),
            info: None,
            versions: vec![],
            update: UpdateStatusDto::default(),
            release_notes_url: None,
            default_args: vec![],
            default_env: vec![],
            is_active: true,
        };
        let json = serde_json::to_string(&dto_active).unwrap();
        assert!(json.contains("\"is_active\":true"));

        let dto_inactive = InstallationCardDto {
            r#type: "llama_cpp".to_string(),
            backend_name: "llama_cpp".to_string(),
            display_name: "llama.cpp".to_string(),
            installed: true,
            gpu_variant: "cpu".to_string(),
            info: None,
            versions: vec![],
            update: UpdateStatusDto::default(),
            release_notes_url: None,
            default_args: vec![],
            default_env: vec![],
            is_active: false,
        };
        let json2 = serde_json::to_string(&dto_inactive).unwrap();
        assert!(json2.contains("\"is_active\":false"));
    }

    #[test]
    fn test_backend_card_dto_is_active_default() {
        // Deserializing without is_active should default to false
        let json = r#"{
            "type": "llama_cpp",
            "display_name": "llama.cpp",
            "installed": true,
            "update": {},
            "default_args": []
        }"#;
        let dto: InstallationCardDto = serde_json::from_str(json).unwrap();
        assert!(!dto.is_active);
        assert!(dto.gpu_variant.is_empty());
    }

    #[test]
    fn test_backend_card_dto_with_versions() {
        let dto = InstallationCardDto {
            r#type: "llama_cpp".to_string(),
            backend_name: "llama_cpp".to_string(),
            display_name: "llama.cpp".to_string(),
            installed: true,
            gpu_variant: "cuda_12".to_string(),
            info: Some(InstallationInfoDto {
                name: "llama-cpp".to_string(),
                version: "1.0.0".to_string(),
                path: "/path/to/installation".to_string(),
                installed_at: 1700000000,
                gpu_variant: "cuda_12".to_string(),
                source: Some(InstallationSourceDto::Prebuilt {
                    version: "1.0.0".to_string(),
                }),
            }),
            versions: vec![InstallationVersionDto {
                name: "llama-cpp".to_string(),
                version: "1.0.0".to_string(),
                path: "/path/to/installation".to_string(),
                installed_at: 1700000000,
                gpu_variant: "cuda_12".to_string(),
                source: Some(InstallationSourceDto::Prebuilt {
                    version: "1.0.0".to_string(),
                }),
                is_active: true,
            }],
            update: UpdateStatusDto {
                checked: true,
                latest_version: Some("1.0.0".to_string()),
                update_available: Some(false),
            },
            release_notes_url: None,
            default_args: vec!["--threads".to_string()],
            default_env: vec![],
            is_active: true,
        };

        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("llama_cpp"));
        assert!(json.contains("1.0.0"));
        assert!(json.contains("cuda_12"));
    }

    #[test]
    fn test_backend_card_dto_ik_llama_type() {
        let dto = InstallationCardDto {
            r#type: "ik_llama".to_string(),
            backend_name: "ik_llama".to_string(),
            display_name: "ik_llama".to_string(),
            installed: false,
            gpu_variant: String::new(),
            info: None,
            versions: vec![],
            update: UpdateStatusDto::default(),
            release_notes_url: None,
            default_args: vec![],
            default_env: vec![],
            is_active: false,
        };

        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("ik_llama"));
    }

    #[test]
    fn test_backend_card_dto_custom_type() {
        let dto = InstallationCardDto {
            r#type: "custom".to_string(),
            backend_name: "custom".to_string(),
            display_name: "Custom Backend".to_string(),
            installed: true,
            gpu_variant: String::new(),
            info: Some(InstallationInfoDto {
                name: "custom-installation".to_string(),
                version: "custom-1.0".to_string(),
                path: "/custom/path".to_string(),
                installed_at: 1700000000,
                gpu_variant: String::new(),
                source: None,
            }),
            versions: vec![],
            update: UpdateStatusDto {
                checked: false,
                latest_version: None,
                update_available: None,
            },
            release_notes_url: None,
            default_args: vec![],
            default_env: vec![],
            is_active: false,
        };

        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("custom"));
        assert!(json.contains("Custom Backend"));
    }

    #[test]
    fn test_backend_card_dto_roundtrip() {
        let original = InstallationCardDto {
            r#type: "llama_cpp".to_string(),
            backend_name: "llama_cpp".to_string(),
            display_name: "llama.cpp".to_string(),
            installed: true,
            gpu_variant: "cuda_12".to_string(),
            info: Some(InstallationInfoDto {
                name: "llama-cpp".to_string(),
                version: "b8407".to_string(),
                path: "/home/user/.local/share/tama/backends/llama-cpp/b8407".to_string(),
                installed_at: 1700000000,
                gpu_variant: "cuda_12".to_string(),
                source: Some(InstallationSourceDto::Prebuilt {
                    version: "b8407".to_string(),
                }),
            }),
            versions: vec![InstallationVersionDto {
                name: "llama-cpp".to_string(),
                version: "b8407".to_string(),
                path: "/home/user/.local/share/tama/backends/llama-cpp/b8407".to_string(),
                installed_at: 1700000000,
                gpu_variant: "cuda_12".to_string(),
                source: Some(InstallationSourceDto::Prebuilt {
                    version: "b8407".to_string(),
                }),
                is_active: true,
            }],
            update: UpdateStatusDto {
                checked: true,
                latest_version: Some("b8500".to_string()),
                update_available: Some(true),
            },
            release_notes_url: Some(
                "https://github.com/ggml-org/llama.cpp/releases/tag/b8500".to_string(),
            ),
            default_args: vec!["--threads".to_string(), "4".to_string()],
            default_env: vec![],
            is_active: true,
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: InstallationCardDto = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.r#type, "llama_cpp");
        assert_eq!(deserialized.display_name, "llama.cpp");
        assert!(deserialized.installed);
        assert!(deserialized.is_active);
        assert_eq!(deserialized.gpu_variant, "cuda_12");
        assert_eq!(deserialized.update.update_available, Some(true));
        assert_eq!(
            deserialized.update.latest_version,
            Some("b8500".to_string())
        );
    }
}
