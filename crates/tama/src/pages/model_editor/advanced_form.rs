use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::{is_transformers, ModelForm};
use super::vllm_form::strip_managed_flags;
use crate::utils::{set_checked, set_input_value, target_value};

const SPEC_TYPE_DRAFT_MTP: &str = "draft-mtp";
const SPEC_TYPE_NGRAM_SIMPLE: &str = "ngram-simple";

/// Advanced form section combining Speculative Decoding and Extra Args.
#[component]
pub fn ModelEditorAdvancedForm(form: RwSignal<Option<ModelForm>>) -> impl IntoView {
    // Checkboxes for spec types
    let toggle_spec_type = move |e: web_sys::Event, spec_type: String| {
        let checked = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|el| el.checked())
            .unwrap_or(false);
        form.update(move |f| {
            if let Some(form) = f {
                if checked {
                    if !form.spec_decoding.spec_types.contains(&spec_type) {
                        form.spec_decoding.spec_types.push(spec_type);
                    }
                } else {
                    form.spec_decoding.spec_types.retain(|s| s != &spec_type);
                }
            }
        });
    };

    let has_any_type = Signal::derive(move || {
        form.get()
            .as_ref()
            .map(|f| !f.spec_decoding.spec_types.is_empty())
            .unwrap_or(false)
    });

    let has_draft_mtp = Signal::derive(move || {
        form.get()
            .as_ref()
            .map(|f| {
                f.spec_decoding
                    .spec_types
                    .contains(&SPEC_TYPE_DRAFT_MTP.to_string())
            })
            .unwrap_or(false)
    });

    // Derived signals for vLLM spec decoding UI
    let has_method = Signal::derive(move || {
        form.get()
            .as_ref()
            .map(|f| {
                f.vllm
                    .spec_decoding
                    .method
                    .as_deref()
                    .is_some_and(|m| !m.is_empty())
            })
            .unwrap_or(false)
    });

    let needs_drafter = Signal::derive(move || {
        form.get()
            .as_ref()
            .map(|f| {
                matches!(
                    f.vllm.spec_decoding.method.as_deref(),
                    Some("dflash") | Some("eagle3") | Some("draft_model")
                )
            })
            .unwrap_or(false)
    });

    // Populate input values when the form data loads (or model changes).
    // Only runs when the model ID changes, not on every keystroke.
    let last_init_id = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            if last_init_id.get_value() != Some(f.id.clone()) {
                set_checked(
                    "field-spec-draft-mtp",
                    f.spec_decoding
                        .spec_types
                        .contains(&SPEC_TYPE_DRAFT_MTP.to_string()),
                );
                set_checked(
                    "field-spec-ngram-simple",
                    f.spec_decoding
                        .spec_types
                        .contains(&SPEC_TYPE_NGRAM_SIMPLE.to_string()),
                );
                set_input_value(
                    "field-args",
                    &if is_transformers(f.hf_format.as_deref()) {
                        strip_managed_flags(&f.args)
                    } else {
                        f.args.clone()
                    },
                );
                // Spec decoding selects and input
                set_input_value(
                    "field-spec-n-max",
                    &f.spec_decoding
                        .n_max
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-spec-n-min",
                    &f.spec_decoding
                        .n_min
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-spec-draft-ngl",
                    &f.spec_decoding
                        .draft_ngl
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                // vLLM spec decoding fields
                set_input_value(
                    "field-vllm-spec-method",
                    &f.vllm.spec_decoding.method.clone().unwrap_or_default(),
                );
                set_input_value(
                    "field-vllm-spec-tokens",
                    &f.vllm
                        .spec_decoding
                        .num_speculative_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-vllm-spec-model",
                    &f.vllm.spec_decoding.model.clone().unwrap_or_default(),
                );
                set_input_value(
                    "field-vllm-spec-rejection-method",
                    &f.vllm
                        .spec_decoding
                        .rejection_sample_method
                        .clone()
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-vllm-spec-draft-sample-method",
                    &f.vllm
                        .spec_decoding
                        .draft_sample_method
                        .clone()
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-vllm-spec-draft-tp-size",
                    &f.vllm
                        .spec_decoding
                        .draft_tensor_parallel_size
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                set_checked(
                    "field-vllm-spec-disable-padded",
                    f.vllm
                        .spec_decoding
                        .disable_padded_drafter_batch
                        .unwrap_or(false),
                );
                last_init_id.set_value(Some(f.id.clone()));
            }
        }
    });

    view! {
        // ── Speculative Decoding subsection (llama.cpp-only) ─────────────
        <Show when=move || !is_transformers(form.get().and_then(|f| f.hf_format.clone()).as_deref())>
        <h3 class="form-section-title">"Speculative Decoding"</h3>
        <div class="form-grid">
            // Spec type checkboxes
            <label class="form-label">"Spec Types"</label>
            <div class="form-check-group">
                // draft-mtp checkbox
                <div class="form-check">
                    <input
                        id="field-spec-draft-mtp"
                        type="checkbox"
                        on:change=move |e| {
                            toggle_spec_type(e, SPEC_TYPE_DRAFT_MTP.to_string());
                        }
                    />
                    <label class="form-check-label" for="field-spec-draft-mtp">
                        "draft-mtp"
                        <div class="form-hint">"Multi-Token Prediction — uses a draft model for speculative decoding"</div>
                    </label>
                </div>

                // ngram-simple checkbox
                <div class="form-check">
                    <input
                        id="field-spec-ngram-simple"
                        type="checkbox"
                        on:change=move |e| {
                            toggle_spec_type(e, SPEC_TYPE_NGRAM_SIMPLE.to_string());
                        }
                    />
                    <label class="form-check-label" for="field-spec-ngram-simple">
                        "ngram-simple"
                        <div class="form-hint">"Simple n-gram speculative decoding — lightweight, no extra model needed"</div>
                    </label>
                </div>
            </div>

            // Draft Max (n_max) — shown when any type is checked
            <Show when=move || has_any_type.get()>
                <label class="form-label" for="field-spec-n-max">"Draft Max"</label>
                <select
                    id="field-spec-n-max"
                    class="form-select"
                    on:change=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.spec_decoding.n_max = val.parse::<u32>().ok();
                            }
                        });
                    }
                >
                    <option value="">"(select)"</option>
                    {(1..=8).map(|v| {
                        let selected = form.get_untracked()
                            .as_ref()
                            .map(|f| f.spec_decoding.n_max == Some(v))
                            .unwrap_or(false);
                        let val = v.to_string();
                        view! { <option value=val selected=selected>{v}</option> }
                    }).collect::<Vec<_>>()}
                </select>

                // Draft Min (n_min) — shown when any type is checked
                <label class="form-label" for="field-spec-n-min">"Draft Min"</label>
                <select
                    id="field-spec-n-min"
                    class="form-select"
                    on:change=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.spec_decoding.n_min = val.parse::<u32>().ok();
                            }
                        });
                    }
                >
                    <option value="">"(select)"</option>
                    {(1..=8).map(|v| {
                        let selected = form.get_untracked()
                            .as_ref()
                            .map(|f| f.spec_decoding.n_min == Some(v))
                            .unwrap_or(false);
                        let val = v.to_string();
                        view! { <option value=val selected=selected>{v}</option> }
                    }).collect::<Vec<_>>()}
                </select>
            </Show>

            // Draft GPU Layers (draft_ngl) — shown when draft-mtp is checked
            <Show when=move || has_draft_mtp.get()>
                <label class="form-label" for="field-spec-draft-ngl">
                    "Draft GPU Layers"
                    <div class="form-hint">"99 = all layers"</div>
                </label>
                <input
                    id="field-spec-draft-ngl"
                    class="form-input"
                    type="number"
                    min="0"
                    max="999"
                    placeholder="e.g. 99"
                    on:input=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.spec_decoding.draft_ngl = if val.is_empty() {
                                    None
                                } else {
                                    val.parse::<u32>().ok()
                                };
                            }
                        });
                    }
                />
            </Show>
        </div>
        </Show>

        // ── vLLM subsection (transformers-format only) ────────────────────
        <Show when=move || is_transformers(form.get().and_then(|f| f.hf_format.clone()).as_deref())>
        <h3 class="form-section-title">"vLLM"</h3>
        <div class="form-grid">
            <label class="form-label">"Prefix caching"</label>
            <div class="form-check">
                <input
                    id="field-vllm-prefix-caching"
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().map(|f| f.vllm.enable_prefix_caching).unwrap_or(false)
                    on:change=move |e| {
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.vllm.enable_prefix_caching = checked;
                            }
                        });
                    }
                />
                <label class="form-check-label" for="field-vllm-prefix-caching">
                    "Enable prefix caching"
                    <div class="form-hint">"vLLM --enable-prefix-caching — reuse KV blocks across requests with shared prefixes"</div>
                </label>
            </div>

            <label class="form-label">"Remote code"</label>
            <div class="form-check">
                <input
                    id="field-vllm-trust-remote-code"
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().map(|f| f.vllm.trust_remote_code).unwrap_or(false)
                    on:change=move |e| {
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.vllm.trust_remote_code = checked;
                            }
                        });
                    }
                />
                <label class="form-check-label" for="field-vllm-trust-remote-code">
                    "Trust remote code"
                    <div class="form-hint">"vLLM --trust-remote-code — required for repos with custom modeling code"</div>
                </label>
            </div>

            // ── Speculative Decoding subsection ─────────────────────
            <h3 class="form-section-title mt-2">"Speculative Decoding"</h3>
            <div class="form-grid">
                // Method dropdown
                <label class="form-label">"Method"</label>
                <select
                    id="field-vllm-spec-method"
                    class="form-select"
                    on:change=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.vllm.spec_decoding.method = if val.is_empty() {
                                    None
                                } else {
                                    Some(val)
                                };
                            }
                        });
                    }
                >
                    <option value="">"(disabled)"</option>
                    <option value="mtp">"mtp — Multi-Token Prediction (no drafter needed)"</option>
                    <option value="ngram">"ngram — N-gram matching (no drafter needed)"</option>
                    <option value="dflash">"dflash — Diffusion block prediction (needs drafter)"</option>
                    <option value="eagle3">"eagle3 — EAGLE-3 autoregressive (needs drafter)"</option>
                    <option value="draft_model">"draft_model — Any smaller model (needs drafter)"</option>
                </select>
                <div class="form-hint">"MTP requires model family support (DeepSeek, Qwen3, Gemma 4, etc.)"</div>

                // Fields shown when a method is selected
                <Show when=move || has_method.get()>
                    // num_speculative_tokens
                    <label class="form-label" for="field-vllm-spec-tokens">"Speculative tokens"</label>
                    <input
                        id="field-vllm-spec-tokens"
                        class="form-input"
                        type="number"
                        min="1"
                        placeholder="5"
                        on:input=move |e| {
                            let val = target_value(&e);
                            form.update(|f| {
                                if let Some(form) = f {
                                    form.vllm.spec_decoding.num_speculative_tokens = if val.is_empty() {
                                        None
                                    } else {
                                        val.parse::<u32>().ok()
                                    };
                                }
                            });
                        }
                    />
                    <div class="form-hint">"Tokens to propose per step. Default: 5. Values above 8 may reduce quality."</div>

                    // Drafter model — shown only for dflash, eagle3, draft_model
                    <Show when=move || needs_drafter.get()>
                        <label class="form-label" for="field-vllm-spec-model">"Drafter model"</label>
                        <input
                            id="field-vllm-spec-model"
                            class="form-input"
                            type="text"
                            placeholder="owner/repo or /path/to/model"
                            on:input=move |e| {
                                let val = target_value(&e);
                                form.update(|f| {
                                    if let Some(form) = f {
                                        form.vllm.spec_decoding.model = if val.is_empty() {
                                            None
                                        } else {
                                            Some(val)
                                        };
                                    }
                                });
                            }
                        />
                        <div class="form-hint">"HF repo ID or local path to the drafter/speculator model"</div>
                    </Show>

                    // Advanced — collapsible
                    <details>
                        <summary>"Advanced"</summary>
                        // rejection_sample_method
                        <label class="form-label">"Rejection method"</label>
                        <select
                            id="field-vllm-spec-rejection-method"
                            class="form-select"
                            on:change=move |e| {
                                let val = target_value(&e);
                                form.update(|f| {
                                    if let Some(form) = f {
                                        form.vllm.spec_decoding.rejection_sample_method = if val.is_empty() {
                                            None
                                        } else {
                                            Some(val)
                                        };
                                    }
                                });
                            }
                        >
                            <option value="">"(default)"</option>
                            <option value="standard">"standard"</option>
                            <option value="synthetic">"synthetic"</option>
                            <option value="block">"block"</option>
                        </select>

                        // draft_sample_method
                        <label class="form-label">"Draft sample method"</label>
                        <select
                            id="field-vllm-spec-draft-sample-method"
                            class="form-select"
                            on:change=move |e| {
                                let val = target_value(&e);
                                form.update(|f| {
                                    if let Some(form) = f {
                                        form.vllm.spec_decoding.draft_sample_method = if val.is_empty() {
                                            None
                                        } else {
                                            Some(val)
                                        };
                                    }
                                });
                            }
                        >
                            <option value="">"(default)"</option>
                            <option value="greedy">"greedy"</option>
                            <option value="probabilistic">"probabilistic"</option>
                        </select>

                        // draft_tensor_parallel_size
                        <label class="form-label">"Draft TP size"</label>
                        <input
                            id="field-vllm-spec-draft-tp-size"
                            class="form-input"
                            type="number"
                            min="1"
                            placeholder="1"
                            on:input=move |e| {
                                let val = target_value(&e);
                                form.update(|f| {
                                    if let Some(form) = f {
                                        form.vllm.spec_decoding.draft_tensor_parallel_size = if val.is_empty() {
                                            None
                                        } else {
                                            val.parse::<u32>().ok()
                                        };
                                    }
                                });
                            }
                        />

                        // disable_padded_drafter_batch
                        <div class="form-check">
                            <input
                                id="field-vllm-spec-disable-padded"
                                type="checkbox"
                                prop:checked=move || form.get().as_ref().and_then(|f| f.vllm.spec_decoding.disable_padded_drafter_batch).unwrap_or(false)
                                on:change=move |e| {
                                    let checked = e.target()
                                        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                        .map(|el| el.checked())
                                        .unwrap_or(false);
                                    form.update(|f| {
                                        if let Some(form) = f {
                                            form.vllm.spec_decoding.disable_padded_drafter_batch = Some(checked);
                                        }
                                    });
                                }
                            />
                            <label class="form-check-label" for="field-vllm-spec-disable-padded">
                                "Disable padded drafter batch"
                                <div class="form-hint">"Use unpadded draft batches (EAGLE only)"</div>
                            </label>
                        </div>
                    </details>
                </Show>
            </div>
        </div>
        </Show>

        // ── Extra Args subsection ────────────────────────────────────────
        <h3 class="form-section-title mt-2">"Extra Args"</h3>
        <textarea
            id="field-args"
            class="form-textarea"
            rows="6"
            placeholder="One flag per line, e.g. -fa 1, -b 4096, --mlock"
            on:input=move |e| {
                let val = target_value(&e);
                form.update(|f| {
                    if let Some(form) = f {
                        // For transformers models, strip managed vLLM flags from Extra Args
                        form.args = if is_transformers(form.hf_format.as_deref()) {
                            strip_managed_flags(&val)
                        } else {
                            val
                        };
                    }
                });
            }
        />
        <span class="form-hint">"One flag per line, e.g. -fa 1, --mlock, or -b 4096. Quote values containing spaces"</span>
    }
}
