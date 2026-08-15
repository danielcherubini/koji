use super::types::*;

use super::types::{ModelDetail, ModelListResponse, RefreshResponse, VerifyResponse};

/// Return the response when its status is one of `ok_statuses`,
/// otherwise Err with the response body text.
async fn expect_status(
    resp: gloo_net::http::Response,
    ok_statuses: &[u16],
) -> Result<gloo_net::http::Response, String> {
    if ok_statuses.contains(&resp.status()) {
        Ok(resp)
    } else {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(text)
    }
}

use crate::utils::{delete_request, get_request, handle_response, post_request, put_request};

pub async fn fetch_model(id: String) -> Option<ModelDetail> {
    if id == "new" {
        let resp = get_request("/tama/v1/models").send().await.ok()?;
        if handle_response(&resp) {
            return None;
        }
        let list: ModelListResponse = resp.json().await.ok()?;
        return Some(ModelDetail {
            id: 0,
            backend: list
                .backends
                .first()
                .map(|b| b.name.clone())
                .unwrap_or_default(),
            gpu_variant: None,
            gpu_device: None,
            model: None,
            quant: None,
            args: vec![],
            sampling: None,
            enabled: true,
            context_length: None,
            num_parallel: Some(0), // 0 = auto
            port: None,
            api_name: None,
            display_name: None,
            kv_unified: true,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            hf_context_length: None,
            quants: std::collections::BTreeMap::new(),
            backends: list.backends,
            mmproj: None,
            mtp_model: None,
            repo_commit_sha: None,
            repo_pulled_at: None,
            modalities: None,
            reasoning_levels: None,
            spec_decoding: None,
            vllm: None,
            n_batch: None,
            n_ubatch: None,
            hf_format: None,
        });
    }
    let encoded_id = urlencoding::encode(&id);
    let resp = get_request(&format!("/tama/v1/models/{}", encoded_id))
        .send()
        .await;
    match resp {
        Ok(r) => {
            if handle_response(&r) {
                return None;
            }
            if r.status() == 200 {
                r.json::<ModelDetail>().await.ok()
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// Sampling form fields and their JSON value kind. `top_k` is an integer;
/// all others are floats.
const SAMPLING_FIELDS: &[(&str, SamplingKind)] = &[
    ("temperature", SamplingKind::Float),
    ("top_k", SamplingKind::Int),
    ("top_p", SamplingKind::Float),
    ("min_p", SamplingKind::Float),
    ("presence_penalty", SamplingKind::Float),
    ("frequency_penalty", SamplingKind::Float),
    ("repeat_penalty", SamplingKind::Float),
];

enum SamplingKind {
    Float,
    Int,
}

pub fn form_to_sampling_json(form: &ModelForm) -> serde_json::Value {
    let mut obj = serde_json::Map::new();

    for (key, kind) in SAMPLING_FIELDS {
        if let Some(field) = form.sampling.get(*key) {
            if !field.enabled {
                continue;
            }
            match kind {
                SamplingKind::Float => {
                    if let Ok(val) = field.value.parse::<f64>() {
                        obj.insert(key.to_string(), serde_json::json!(val));
                    }
                }
                SamplingKind::Int => {
                    if let Ok(val) = field.value.parse::<u64>() {
                        obj.insert(key.to_string(), serde_json::json!(val));
                    }
                }
            }
        }
    }

    if obj.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!(obj)
    }
}

pub async fn save_model(
    args: Vec<String>,
    form: ModelForm,
    is_new: bool,
    reasoning_levels: Vec<String>,
) -> Result<(), String> {
    let sampling = form_to_sampling_json(&form);

    let body = serde_json::json!({
        "id": form.id,
        "backend": form.backend,
        "gpu_variant": form.gpu_variant,
        "gpu_device": form.gpu_device,
        "model": form.model,
        "quant": form.quant,
        "mmproj": form.mmproj,
        "mtp_model": form.mtp_model,
        "args": args,
        // Always an array — `[]` clears levels on the server; `null` would
        // preserve them.
        "reasoningLevels": serde_json::json!(reasoning_levels),
        "sampling": sampling,
        "enabled": form.enabled,
        "context_length": form.context_length,
        "num_parallel": form.num_parallel,
        "port": form.port,
        "api_name": form.api_name,
        "display_name": form.display_name,
        "kv_unified": form.kv_unified,
        "gpu_layers": form.gpu_layers,
        "cache_type_k": form.cache_type_k.clone(),
        "cache_type_v": form.cache_type_v.clone(),
        "quants": form.quants,
        "modalities": form.modalities,
        "spec_decoding": form.spec_decoding,
        "vllm": form.vllm,
        "n_batch": form.n_batch,
        "n_ubatch": form.n_ubatch,
    });

    let encoded_id = urlencoding::encode(&form.id);
    let (url, is_post) = if is_new {
        ("/tama/v1/models".to_string(), true)
    } else {
        (format!("/tama/v1/models/{}", encoded_id), false)
    };

    let req = if is_post {
        post_request(&url)
    } else {
        put_request(&url)
    };

    let resp = req
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    expect_status(resp, &[200, 201]).await?;
    Ok(())
}

pub async fn rename_model(old_id: &str, new_id: &str) -> Result<(), String> {
    let body = serde_json::json!({ "new_id": new_id });
    let encoded_id = urlencoding::encode(old_id);
    let resp = post_request(&format!("/tama/v1/models/{}/rename", encoded_id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    expect_status(resp, &[200]).await?;
    Ok(())
}

pub async fn delete_model_api(id: String) -> Result<(), String> {
    let encoded_id = urlencoding::encode(&id);
    let resp = delete_request(&format!("/tama/v1/models/{}", encoded_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    expect_status(resp, &[200]).await?;
    Ok(())
}

pub async fn delete_quant_api(id: String, quant_key: String) -> Result<(), String> {
    let encoded_id = urlencoding::encode(&id);
    let encoded_key = urlencoding::encode(&quant_key);
    let resp = delete_request(&format!(
        "/tama/v1/models/{}/quants/{}",
        encoded_id, encoded_key
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;
    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    expect_status(resp, &[200]).await?;
    Ok(())
}

pub async fn refresh_model_api(id: String) -> Result<RefreshResponse, String> {
    // Percent-encode the id for safe path interpolation; model ids may
    // contain `/`, spaces, or other reserved characters.
    let encoded_id = urlencoding::encode(&id);
    let resp = post_request(&format!("/tama/v1/models/{}/refresh", encoded_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    let resp = expect_status(resp, &[200]).await?;
    resp.json::<RefreshResponse>()
        .await
        .map_err(|e| format!("Failed to parse refresh response: {}", e))
}

pub async fn verify_model_api(id: String) -> Result<VerifyResponse, String> {
    let encoded_id = urlencoding::encode(&id);
    let resp = post_request(&format!("/tama/v1/models/{}/verify", encoded_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    let resp = expect_status(resp, &[200]).await?;
    resp.json::<VerifyResponse>()
        .await
        .map_err(|e| format!("Failed to parse verify response: {}", e))
}

pub async fn fetch_sampling_templates(
) -> Option<std::collections::HashMap<String, serde_json::Value>> {
    let resp = get_request("/tama/v1/models").send().await.ok()?;
    if handle_response(&resp) {
        return None;
    }
    let list: ModelListResponse = resp.json().await.ok()?;
    let templates = list.sampling_templates?;
    Some(templates)
}

/// Fetch GPU devices available for a backend.
pub async fn fetch_gpu_devices(
    backend: &str,
    gpu_variant: &str,
) -> Vec<super::types::GpuDeviceInfo> {
    let url = format!(
        "/tama/v1/system/gpu-devices?backend={}&gpu_variant={}",
        urlencoding::encode(backend),
        urlencoding::encode(gpu_variant)
    );
    let resp = get_request(&url).send().await;
    match resp {
        Ok(r) => {
            if handle_response(&r) {
                return Vec::new();
            }
            if r.status() == 200 {
                r.json::<Vec<super::types::GpuDeviceInfo>>()
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        Err(_) => Vec::new(),
    }
}

/// Refresh GPU devices for a backend (forces re-discovery).
pub async fn refresh_gpu_devices(
    backend: &str,
    gpu_variant: &str,
) -> Vec<super::types::GpuDeviceInfo> {
    let url = format!(
        "/tama/v1/system/gpu-devices/refresh?backend={}&gpu_variant={}",
        urlencoding::encode(backend),
        urlencoding::encode(gpu_variant)
    );
    let resp = post_request(&url).send().await;
    match resp {
        Ok(r) => {
            if handle_response(&r) {
                return Vec::new();
            }
            if r.status() == 200 {
                r.json::<Vec<super::types::GpuDeviceInfo>>()
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        Err(_) => Vec::new(),
    }
}

/// Save a sampling preset template via the existing structured config endpoint.
///
/// Flow: GET `/tama/v1/config/structured` → insert/update `sampling_templates` → POST back.
pub async fn save_sampling_template(name: &str, params: &serde_json::Value) -> Result<(), String> {
    // GET the current structured config
    let get_resp = get_request("/tama/v1/config/structured")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if handle_response(&get_resp) {
        return Err("unauthorized".into());
    }
    if get_resp.status() != 200 {
        let text = get_resp
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".into());
        return Err(format!("Failed to fetch config: {}", text));
    }

    // Parse the config as a JSON Value so we can modify it
    let mut config: serde_json::Value = get_resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse config: {}", e))?;

    // Ensure sampling_templates exists and insert/update the preset
    if let Some(obj) = config.as_object_mut() {
        let templates = obj
            .entry("sampling_templates")
            .or_insert(serde_json::Value::Object(serde_json::Map::new()));

        if let Some(templates_obj) = templates.as_object_mut() {
            templates_obj.insert(name.to_string(), params.clone());
        } else {
            // Replace with a proper object
            let mut map = serde_json::Map::new();
            map.insert(name.to_string(), params.clone());
            obj.insert(
                "sampling_templates".to_string(),
                serde_json::Value::Object(map),
            );
        }
    }

    // POST the full config back
    let post_resp = post_request("/tama/v1/config/structured")
        .json(&config)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if handle_response(&post_resp) {
        return Err("unauthorized".into());
    }
    if post_resp.status() == 200 {
        Ok(())
    } else {
        let text = post_resp
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".into());
        Err(format!("Failed to save template: {}", text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::model_editor::types::{ModelForm, SamplingField};
    use std::collections::HashMap;

    fn make_form(sampling: HashMap<String, SamplingField>) -> ModelForm {
        ModelForm {
            sampling,
            ..Default::default()
        }
    }

    /// Enabled float field is inserted as json!(0.7) (f64).
    #[test]
    fn test_form_to_sampling_json_enabled_float() {
        let mut sampling = HashMap::new();
        sampling.insert(
            "temperature".to_string(),
            SamplingField {
                enabled: true,
                value: "0.7".to_string(),
            },
        );
        let result = form_to_sampling_json(&make_form(sampling));
        assert_eq!(result, serde_json::json!({"temperature": 0.7}));
    }

    /// top_k (integer) is inserted as json!(40u64), not 40.0.
    #[test]
    fn test_form_to_sampling_json_top_k_int() {
        let mut sampling = HashMap::new();
        sampling.insert(
            "top_k".to_string(),
            SamplingField {
                enabled: true,
                value: "40".to_string(),
            },
        );
        let result = form_to_sampling_json(&make_form(sampling));
        assert_eq!(result, serde_json::json!({"top_k": 40}));
    }

    /// Disabled field is skipped.
    #[test]
    fn test_form_to_sampling_json_disabled_skipped() {
        let mut sampling = HashMap::new();
        sampling.insert(
            "temperature".to_string(),
            SamplingField {
                enabled: false,
                value: "0.7".to_string(),
            },
        );
        let result = form_to_sampling_json(&make_form(sampling));
        assert_eq!(result, serde_json::Value::Null);
    }

    /// Unparseable value is skipped.
    #[test]
    fn test_form_to_sampling_json_unparseable_skipped() {
        let mut sampling = HashMap::new();
        sampling.insert(
            "temperature".to_string(),
            SamplingField {
                enabled: true,
                value: "abc".to_string(),
            },
        );
        let result = form_to_sampling_json(&make_form(sampling));
        assert_eq!(result, serde_json::Value::Null);
    }

    /// All-empty form → serde_json::Value::Null.
    #[test]
    fn test_form_to_sampling_json_all_empty() {
        let result = form_to_sampling_json(&make_form(HashMap::new()));
        assert_eq!(result, serde_json::Value::Null);
    }

    // Note: expect_status gets no unit test (constructing a
    // gloo_net::http::Response requires a browser runtime).
    // Compile-checked only.
}
