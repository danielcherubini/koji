use crate::config::resolve::tests::test_helpers as h;
use crate::config::Config;

/// Test that --alias is injected for llama.cpp backends when api_name is set.
#[test]
fn test_build_full_args_injects_alias_for_llama_cpp() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("unsloth").join("gemma-4-E2B-it-GGUF");
    let quant_file = org_dir.join("gemma-4-E2B-it-UD-IQ3_XXS.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "IQ3_XXS".to_string(),
        crate::config::types::QuantEntry {
            file: "gemma-4-E2B-it-UD-IQ3_XXS.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: None,
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());

    let server = h::sample_server(|s| {
        s.model = Some("unsloth/gemma-4-E2B-it-GGUF".to_string());
        s.quant = Some("IQ3_XXS".to_string());
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.api_name = Some("unsloth/gemma-4-E2B-it-GGUF".to_string());
        s.gpu_layers = Some(999);
        s.quants = quants;
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Should contain --alias with the api_name value
    assert!(
        args.contains(&"--alias".to_string()),
        "Expected --alias in args, got: {:?}",
        args
    );
    let alias_idx = args.iter().position(|a| a == "--alias").unwrap();
    assert_eq!(
        args[alias_idx + 1],
        "unsloth/gemma-4-E2B-it-GGUF",
        "Alias value should match api_name"
    );
}

/// Test that --alias is NOT injected for non-llama.cpp backends.
#[test]
fn test_build_full_args_no_alias_for_non_llama_cpp() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());

    let server = h::sample_server(|s| {
        s.backend = "tts_kokoro".to_string();
        s.model = None;
        s.quant = None;
        s.quants = std::collections::BTreeMap::new();
        s.api_name = Some("tts-model".to_string());
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.contains(&"--alias".to_string()),
        "Expected no --alias for non-llama.cpp backend, got: {:?}",
        args
    );
}

/// Test that --alias falls back to model field when api_name is not set.
#[test]
fn test_build_full_args_alias_falls_back_to_model() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.api_name = None; // No api_name set
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Should fall back to model field
    let alias_idx = args.iter().position(|a| a == "--alias");
    assert!(
        alias_idx.is_some(),
        "Expected --alias when model is set (even without api_name)"
    );
    if let Some(idx) = alias_idx {
        assert_eq!(
            args[idx + 1],
            "org/repo",
            "Alias should fall back to model field"
        );
    }
}

/// Test that --alias is not injected when user already set it in args.
#[test]
fn test_build_full_args_respects_user_alias() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.args = vec!["--alias".to_string(), "my-custom-alias".to_string()];
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.api_name = Some("api-name".to_string());
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Should only have one --alias (the user's), not a duplicate
    let alias_count = args.iter().filter(|a| *a == "--alias").count();
    assert_eq!(
        alias_count, 1,
        "Expected exactly one --alias, got {} in: {:?}",
        alias_count, args
    );
    let alias_idx = args.iter().position(|a| a == "--alias").unwrap();
    assert_eq!(
        args[alias_idx + 1],
        "my-custom-alias",
        "User's alias should be preserved"
    );
}
