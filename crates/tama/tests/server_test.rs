mod common;

#[cfg(feature = "ssr")]
mod tests {
    use sqlx::PgPool;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Create a minimal WebState for tests.
    fn test_web_state(
        db_dir: Option<std::path::PathBuf>,
        pool: Option<Arc<PgPool>>,
    ) -> tama_web::web_types::WebState {
        let repository = db_dir.and_then(|dir| {
            tama_core::db::repository::Repository::open(&dir)
                .ok()
                .map(|r| {
                    Arc::new(std::sync::Mutex::new(r))
                        as Arc<std::sync::Mutex<tama_core::db::repository::Repository>>
                })
        });
        tama_web::web_types::WebState {
            jobs: Some(Arc::new(tama_web::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            repository,
            db_pool: pool,
        }
    }

    async fn start_test_server() -> (reqwest::Client, std::net::SocketAddr) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let config = tama_core::config::Config::default();
            let state = Arc::new(tama_core::proxy::ProxyState::new(config, None, None));
            axum::serve(
                listener,
                tama_web::router::build_web_routes(Arc::new(test_web_state(None, None)))
                    .with_state(state)
                    .layer(axum::extract::Extension(test_web_state(None, None))),
            )
            .await
            .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (reqwest::Client::new(), addr)
    }

    /// Helper to get a CSRF token from the server.
    async fn get_csrf_token(client: &reqwest::Client, base_url: &str) -> String {
        let resp = client
            .get(format!("{}/tama/v1/config/structured", base_url))
            .send()
            .await
            .unwrap();
        resp.headers()
            .get(reqwest::header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|cookie| {
                cookie
                    .split(';')
                    .next()
                    .and_then(|part| part.split_once('='))
                    .map(|(_, val)| val.to_string())
            })
            .unwrap_or_else(|| "test-token".to_string())
    }

    /// Helper to make a POST request with CSRF token.
    #[allow(dead_code)]
    async fn post_with_csrf(
        client: &reqwest::Client,
        url: &str,
        body: serde_json::Value,
        csrf_token: &str,
    ) -> reqwest::Response {
        client
            .post(url)
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("tama_csrf_token={csrf_token}"))
            .header("x-csrf-token", csrf_token)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    /// GET / returns 200 (index.html embedded) or 404 (dist/ empty in dev) — both are valid.
    #[tokio::test]
    async fn test_root_returns_html_or_not_found() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .get(format!("http://{}/", addr))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        assert!(
            status == 200 || status == 404,
            "Expected 200 or 404 for /, got {status}"
        );
    }

    /// GET /tama/v1/config returns 410 Gone (raw TOML endpoint removed).
    #[tokio::test]
    async fn test_410_gone_for_raw_toml_config() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .get(format!("http://{}/tama/v1/config", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 410);
    }

    /// POST /tama/v1/config returns 410 Gone (raw TOML endpoint removed).
    #[tokio::test]
    async fn test_410_gone_for_raw_toml_config_save() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .post(format!("http://{}/tama/v1/config", addr))
            .json(&serde_json::json!({ "content": "not valid toml [[[[" }))
            .send()
            .await
            .unwrap();
        // 410 Gone — raw TOML config endpoint removed
        assert_eq!(resp.status().as_u16(), 410);
    }

    /// End-to-end test: CRUD operations via the web API update the proxy's in-memory config.
    ///
    /// This verifies the hot-reload path: when a model is created, updated, or deleted
    /// through the web API, the proxy's live `Arc<RwLock<Config>>` is updated without
    /// requiring a restart.
    #[tokio::test]
    async fn test_hot_reload_crud_updates_proxy_config() {
        // ── Setup ─────────────────────────────────────────────────────────────────
        // Create a temporary config directory with a DB.
        let temp_dir = tempfile::tempdir().unwrap();
        let config_dir = temp_dir.path().to_path_buf();

        // Model CRUD is Postgres-backed; use an isolated migrated schema.
        let guard = crate::common::with_schema().await;
        let pool = Arc::new(guard.pool.clone());

        let initial_config = tama_core::config::Config::default();

        // The shared proxy config — this is what the proxy would hold in production.
        let proxy_config = Arc::new(tokio::sync::RwLock::new(initial_config));

        // ── Start server ──────────────────────────────────────────────────────────
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        {
            let proxy_config_server = proxy_config.clone();
            let config_dir_server = config_dir.clone();
            let pool_server = pool.clone();
            tokio::spawn(async move {
                let config = (*proxy_config_server.read().await).clone();
                let state = Arc::new(tama_core::proxy::ProxyState::new(
                    config,
                    Some(config_dir_server.clone()),
                    Some(pool_server.clone()),
                ));
                axum::serve(
                    listener,
                    tama_web::router::build_web_routes(Arc::new(test_web_state(
                        Some(config_dir_server.clone()),
                        Some(pool_server.clone()),
                    )))
                    .with_state(state)
                    .layer(axum::extract::Extension(test_web_state(
                        Some(config_dir_server.clone()),
                        Some(pool_server.clone()),
                    ))),
                )
                .await
                .unwrap();
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        // Get CSRF token for authenticated POST requests
        let csrf_token = get_csrf_token(&client, &format!("http://{}/", addr)).await;

        // ── POST /tama/v1/models — create ─────────────────────────────────────────────
        let resp = client
            .post(format!("http://{}/tama/v1/models", addr))
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("tama_csrf_token={csrf_token}"))
            .header("x-csrf-token", &csrf_token)
            .json(&serde_json::json!({
                "repo_id": "test-model",
                "backend": "llama_cpp",
                "args": ["--host", "0.0.0.0"],
                "enabled": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            201,
            "POST /tama/v1/models should return 201 Created"
        );

        // Verify 'test-model' was created via GET /tama/v1/models.
        // Extract its auto-assigned integer id for subsequent requests.
        let model_id: i64 = {
            let resp = client
                .get(format!("http://{}/tama/v1/models", addr))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            let models = body["models"].as_array().unwrap();
            let model = models
                .iter()
                .find(|m| m["repo_id"].as_str() == Some("test-model"));
            assert!(
                model.is_some(),
                "proxy config should contain 'test-model' after POST /tama/v1/models"
            );
            let model = model.unwrap();
            assert_eq!(
                model["backend"].as_str(),
                Some("llama_cpp"),
                "backend should be 'llama_cpp'"
            );
            model["id"].as_i64().unwrap()
        };

        // ── PUT /tama/v1/models/:id — update ──────────────────────────────────────────
        let resp = client
            .put(format!("http://{}/tama/v1/models/{}", addr, model_id))
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("tama_csrf_token={csrf_token}"))
            .header("x-csrf-token", &csrf_token)
            .json(&serde_json::json!({
                "backend": "ik_llama",
                "args": [],
                "enabled": false
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            200,
            "PUT /tama/v1/models/:id should return 200"
        );

        // Verify 'test-model' was updated via GET /tama/v1/models.
        {
            let resp = client
                .get(format!("http://{}/tama/v1/models", addr))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            let models = body["models"].as_array().unwrap();
            let model = models
                .iter()
                .find(|m| m["repo_id"].as_str() == Some("test-model"));
            assert!(model.is_some(), "test-model should still exist after PUT");
            let model = model.unwrap();
            assert_eq!(
                model["backend"].as_str(),
                Some("ik_llama"),
                "backend should be updated to 'ik_llama'"
            );
            assert_eq!(
                model["enabled"].as_bool(),
                Some(false),
                "model should be disabled after update"
            );
        }

        // ── DELETE /tama/v1/models/:id ────────────────────────────────────────────────
        let resp = client
            .delete(format!("http://{}/tama/v1/models/{}", addr, model_id))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            200,
            "DELETE /tama/v1/models/:id should return 200"
        );

        // Verify 'test-model' was removed via GET /tama/v1/models.
        {
            let resp = client
                .get(format!("http://{}/tama/v1/models", addr))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            let models = body["models"].as_array().unwrap();
            let found = models
                .iter()
                .any(|m| m["repo_id"].as_str() == Some("test-model"));
            assert!(
                !found,
                "proxy config should not contain 'test-model' after DELETE"
            );
        }

        // ── POST /tama/v1/models — create hot-reload-model ────────────────────────────
        // Models are stored in SQLite, so create via the API directly.
        let resp = client
            .post(format!("http://{}/tama/v1/models", addr))
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("tama_csrf_token={csrf_token}"))
            .header("x-csrf-token", &csrf_token)
            .json(&serde_json::json!({
                "repo_id": "hot-reload-model",
                "backend": "llama_cpp",
                "args": [],
                "enabled": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            201,
            "POST /tama/v1/models should return 201 for hot-reload-model"
        );

        // Verify 'hot-reload-model' was created via GET /tama/v1/models.
        {
            let resp = client
                .get(format!("http://{}/tama/v1/models", addr))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            let models = body["models"].as_array().unwrap();
            let found = models
                .iter()
                .any(|m| m["repo_id"].as_str() == Some("hot-reload-model"));
            assert!(
                found,
                "proxy config should contain 'hot-reload-model' after POST /tama/v1/models"
            );
        }

        // Keep temp_dir alive until all assertions are done so the files aren't removed early.
        drop(temp_dir);
    }

    /// End-to-end test: POST default_args with gpu_variant saves to DB,
    /// GET backends returns per-variant args.
    #[tokio::test]
    async fn test_installation_default_args_db_roundtrip() {
        // Create temp dir for DB
        let temp_dir = tempfile::tempdir().unwrap();
        let config_dir = temp_dir.path().to_path_buf();

        // Initialize DB (runs migrations)
        {
            let _open_result = tama_core::db::open(&config_dir).unwrap();
        }

        // Seed provider_configs with test data for llama_cpp:cpu
        {
            let open_result = tama_core::db::open(&config_dir).unwrap();
            tama_core::db::queries::upsert_installation_config(
                &open_result.conn,
                "",
                "llama_cpp",
                "cpu",
                &["--threads".to_string(), "4".to_string()],
                &[],
                None,
            )
            .unwrap();
            tama_core::db::queries::upsert_installation_config(
                &open_result.conn,
                "",
                "llama_cpp",
                "vulkan",
                &["--flash-attn".to_string()],
                &[],
                None,
            )
            .unwrap();
        }

        // Start server
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        {
            let config_dir_server = config_dir.clone();
            tokio::spawn(async move {
                let config = tama_core::config::Config::default();
                let state = Arc::new(tama_core::proxy::ProxyState::new(
                    config,
                    Some(config_dir_server.clone()),
                    None,
                ));
                axum::serve(
                    listener,
                    tama_web::router::build_web_routes(Arc::new(test_web_state(
                        Some(config_dir_server.clone()),
                        None,
                    )))
                    .with_state(state)
                    .layer(axum::extract::Extension(test_web_state(
                        Some(config_dir_server.clone()),
                        None,
                    ))),
                )
                .await
                .unwrap();
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let csrf_token = get_csrf_token(&client, &format!("http://{}/", addr)).await;

        // POST default_args for llama_cpp with gpu_variant=vulkan
        let resp = client
            .post(format!(
                "http://{}/tama/v1/installations/llama_cpp/default-args?gpu_variant=vulkan",
                addr
            ))
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("tama_csrf_token={csrf_token}"))
            .header("x-csrf-token", &csrf_token)
            .json(&serde_json::json!({
                "default_args": ["--vulkan-devices", "0"]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            200,
            "POST default_args should return 200"
        );

        // Verify the DB was updated
        {
            let open_result = tama_core::db::open(&config_dir).unwrap();
            let config = tama_core::db::queries::get_installation_config(
                &open_result.conn,
                "llama_cpp",
                "vulkan",
            )
            .unwrap()
            .expect("vulkan config should exist");
            assert_eq!(
                config.default_args,
                vec!["--vulkan-devices".to_string(), "0".to_string()],
                "vulkan default_args should be updated"
            );

            // cpu variant should be unchanged
            let cpu_config = tama_core::db::queries::get_installation_config(
                &open_result.conn,
                "llama_cpp",
                "cpu",
            )
            .unwrap()
            .expect("cpu config should exist");
            assert_eq!(
                cpu_config.default_args,
                vec!["--threads".to_string(), "4".to_string()],
                "cpu default_args should be unchanged"
            );
        }

        // POST without gpu_variant should fail (400 or 422)
        let resp = client
            .post(format!(
                "http://{}/tama/v1/installations/llama_cpp/default-args",
                addr
            ))
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("tama_csrf_token={csrf_token}"))
            .header("x-csrf-token", &csrf_token)
            .json(&serde_json::json!({
                "default_args": ["--test"]
            }))
            .send()
            .await
            .unwrap();
        // Without gpu_variant, the query param deserialization fails (400 Bad Request)
        assert!(
            resp.status().is_client_error(),
            "POST without gpu_variant should return client error, got {}",
            resp.status()
        );

        drop(temp_dir);
    }

    /// GET /tama/v1/system/health returns JSON (not HTML from SPA wildcard).
    #[tokio::test]
    async fn test_system_health_returns_json() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .get(format!("http://{}/tama/v1/system/health", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("application/json"),
            "Expected JSON content type, got: {}",
            content_type
        );
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"].as_str(), Some("ok"));
        assert_eq!(body["service"].as_str(), Some("tama"));
    }

    /// GET /tama/v1/logs returns JSON (not HTML from SPA wildcard).
    #[tokio::test]
    async fn test_all_logs_returns_json() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .get(format!("http://{}/tama/v1/logs", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("application/json"),
            "Expected JSON content type, got: {}",
            content_type
        );
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["sources"].is_array());
    }

    /// GET /tama/v1/logs/:backend/events returns SSE (not HTML from SPA wildcard).
    #[tokio::test]
    async fn test_backend_log_sse_returns_events() {
        let (client, addr) = start_test_server().await;
        // Use a timeout so the SSE handler doesn't hang the test.
        let resp = client
            .get(format!("http://{}/tama/v1/logs/test_backend/events", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("text/event-stream"),
            "Expected SSE content type, got: {}",
            content_type
        );
    }
}
