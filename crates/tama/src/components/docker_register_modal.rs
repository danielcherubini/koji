//! Docker backend registration modal - collect docker config and register the
//! backend via POST /tama/v1/backends.

use leptos::prelude::*;

use crate::utils::{handle_response, post_request, target_value};

/// Modal to register a docker-based backend (e.g. vLLM).
#[component]
pub fn DockerRegisterModal(
    /// Called when the modal is closed (after a successful register or cancel).
    on_close: Callback<()>,
) -> impl IntoView {
    let name = RwSignal::new(String::new());
    let image = RwSignal::new(String::new());
    let container_port = RwSignal::new("8000".to_string());
    let model_host_path = RwSignal::new(String::new());
    let model_container_path = RwSignal::new("/models".to_string());
    let shm_size = RwSignal::new("2G".to_string());
    let gpus = RwSignal::new(String::new());
    let error = RwSignal::new(Option::<String>::None);
    let submitting = RwSignal::new(false);

    let do_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        error.set(None);

        let name_val = name.get().trim().to_string();
        let image_val = image.get().trim().to_string();
        let model_host = model_host_path.get().trim().to_string();
        let model_container = model_container_path.get().trim().to_string();
        let container_port_val: u16 = container_port.get().trim().parse().unwrap_or(8000);
        let shm_val = shm_size.get().trim().to_string();
        let gpus_val = gpus.get().trim().to_string();

        if name_val.is_empty() {
            error.set(Some("Backend name is required".to_string()));
            return;
        }
        if image_val.is_empty() {
            error.set(Some("Docker image is required".to_string()));
            return;
        }
        if model_host.is_empty() || model_container.is_empty() {
            error.set(Some(
                "Both model mount host and container paths are required".to_string(),
            ));
            return;
        }

        let body = serde_json::json!({
            "name": name_val,
            "backend_type": "docker",
            "version": "1.0.0",
            "gpu_variant": "cpu",
            "docker_config": {
                "image": image_val,
                "container_port": container_port_val,
                "model_mount": {
                    "host_path": model_host,
                    "container_path": model_container,
                    "read_only": false
                },
                "shm_size": if shm_val.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(shm_val) },
                "gpus": if gpus_val.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(gpus_val) }
            }
        });

        submitting.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let result = match post_request("/tama/v1/backends").json(&body) {
                Ok(req) => req.send().await,
                Err(e) => {
                    error.set(Some(format!("Failed to encode request: {e}")));
                    submitting.set(false);
                    return;
                }
            };
            match result {
                Ok(resp) => {
                    if handle_response(&resp) {
                        return;
                    }
                    if resp.ok() {
                        submitting.set(false);
                        on_close.run(());
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        error.set(Some(format!("Register failed: {text}")));
                        submitting.set(false);
                    }
                }
                Err(e) => {
                    error.set(Some(format!("Register request failed: {e}")));
                    submitting.set(false);
                }
            }
        });
    };

    view! {
        <div class="modal-backdrop modal-backdrop--open">
            <div class="modal" on:click=|e: leptos::ev::MouseEvent| e.stop_propagation()>
                <div class="modal-header">
                    <h2 class="modal-title">"Register Docker Backend"</h2>
                    <button
                        class="modal-close"
                        on:click=move |_| on_close.run(())
                        aria-label="Close"
                    >
                        "×"
                    </button>
                </div>
                <div class="modal-body">
                    <p class="form-hint">
                        "Register a docker-based backend (e.g. vLLM). The container will be managed by TAMA."
                    </p>

                    {move || error.get().map(|msg| view! {
                        <div class="alert alert--error">
                            <span class="alert__icon">"⚠"</span>
                            {msg}
                        </div>
                    }.into_any())}

                    <form on:submit=do_submit>
                        <div class="form-group">
                            <label class="form-label">"Backend Name"</label>
                            <input
                                class="form-input"
                                type="text"
                                prop:value=move || name.get()
                                on:input=move |ev| {
                                    name.set(target_value(&ev));
                                }
                                placeholder="e.g. vllm"
                            />
                        </div>

                        <div class="form-group">
                            <label class="form-label">"Docker Image"</label>
                            <input
                                class="form-input"
                                type="text"
                                prop:value=move || image.get()
                                on:input=move |ev| {
                                    image.set(target_value(&ev));
                                }
                                placeholder="e.g. stilldeadcode/vllm-radiance:0.5.8"
                            />
                        </div>

                        <div class="form-group">
                            <label class="form-label">"Container Port"</label>
                            <input
                                class="form-input"
                                type="number"
                                min="1"
                                max="65535"
                                prop:value=move || container_port.get()
                                on:input=move |ev| {
                                    container_port.set(target_value(&ev));
                                }
                            />
                            <p class="form-hint">"Port the backend listens on inside the container (default 8000)."</p>
                        </div>

                        <div class="form-group">
                            <label class="form-label">"Model Mount Host Path"</label>
                            <input
                                class="form-input"
                                type="text"
                                prop:value=move || model_host_path.get()
                                on:input=move |ev| {
                                    model_host_path.set(target_value(&ev));
                                }
                                placeholder="e.g. /home/user/models"
                            />
                        </div>

                        <div class="form-group">
                            <label class="form-label">"Model Mount Container Path"</label>
                            <input
                                class="form-input"
                                type="text"
                                prop:value=move || model_container_path.get()
                                on:input=move |ev| {
                                    model_container_path.set(target_value(&ev));
                                }
                                placeholder="e.g. /models"
                            />
                        </div>

                        <div class="form-group">
                            <label class="form-label">"Shared Memory Size"</label>
                            <input
                                class="form-input"
                                type="text"
                                prop:value=move || shm_size.get()
                                on:input=move |ev| {
                                    shm_size.set(target_value(&ev));
                                }
                                placeholder="e.g. 2G"
                            />
                        </div>

                        <div class="form-group">
                            <label class="form-label">"GPUs"</label>
                            <input
                                class="form-input"
                                type="text"
                                prop:value=move || gpus.get()
                                on:input=move |ev| {
                                    gpus.set(target_value(&ev));
                                }
                                placeholder="e.g. all (leave empty to disable)"
                            />
                        </div>

                        <div class="form-actions">
                            <button
                                type="button"
                                class="btn btn-secondary"
                                on:click=move |_| on_close.run(())
                            >
                                "Cancel"
                            </button>
                            <button
                                type="submit"
                                class="btn btn-primary"
                                disabled=move || submitting.get()
                            >
                                "Register"
                            </button>
                        </div>
                    </form>
                </div>
            </div>
        </div>
    }
}
