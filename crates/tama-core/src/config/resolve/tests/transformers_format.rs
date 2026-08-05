use crate::config::resolve::tests::test_helpers as h;

/// When hf_format is "transformers", build_full_args emits the model path
/// as a positional arg (first token) and does NOT emit llama.cpp-only flags.
#[test]
fn test_build_full_args_transformers_positional_model_path() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    // Create the directory structure for the model repo
    let org_dir = models_dir.join("org").join("repo");
    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");

    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.backend = "vllm".to_string();
        s.hf_format = Some("transformers".to_string());
        // transformers models have no quant, but do have a model repo id
        s.model = Some("org/repo".to_string());
        s.quant = None;
        // These are set but should NOT appear for transformers format
        s.context_length = Some(4096);
        s.num_parallel = Some(2);
        s.gpu_layers = Some(32);
        s.n_batch = Some(512);
        s.n_ubatch = Some(256);
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Positional model path should be the first token (or appear early)
    assert!(
        args.iter().any(|a| a.contains("org/repo")),
        "Expected positional model path 'org/repo' in args: {:?}",
        args
    );

    // Should NOT have llama.cpp-only flags
    assert!(
        !args.contains(&"-m".to_string()),
        "transformers format should NOT have -m flag: {:?}",
        args
    );
    assert!(
        !args.contains(&"-c".to_string()),
        "transformers format should NOT have -c flag: {:?}",
        args
    );
    assert!(
        !args.contains(&"-np".to_string()),
        "transformers format should NOT have -np flag: {:?}",
        args
    );
    assert!(
        !args.contains(&"-ngl".to_string()),
        "transformers format should NOT have -ngl flag: {:?}",
        args
    );
}

/// When hf_format is "transformers", llama.cpp-only flags are gated out
/// even when the corresponding server fields are set.
#[test]
fn test_build_full_args_transformers_no_llama_cpp_flags() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");

    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.backend = "vllm".to_string();
        s.hf_format = Some("transformers".to_string());
        s.model = Some("org/repo".to_string());
        s.quant = None;
        // All llama.cpp-specific fields set — should all be gated out
        s.context_length = Some(8192);
        s.num_parallel = Some(4);
        s.gpu_layers = Some(100);
        s.n_batch = Some(4096);
        s.n_ubatch = Some(512);
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Verify ALL llama.cpp-only flags are absent
    assert!(
        !args.contains(&"-m".to_string()),
        "Should not have -m: {:?}",
        args
    );
    assert!(
        !args.contains(&"-c".to_string()),
        "Should not have -c: {:?}",
        args
    );
    assert!(
        !args.contains(&"-np".to_string()),
        "Should not have -np: {:?}",
        args
    );
    assert!(
        !args.contains(&"-b".to_string()),
        "Should not have -b: {:?}",
        args
    );
    assert!(
        !args.contains(&"-ub".to_string()),
        "Should not have -ub: {:?}",
        args
    );
    assert!(
        !args.contains(&"-ngl".to_string()),
        "Should not have -ngl: {:?}",
        args
    );

    // Positional model path should still be present
    assert!(
        args.iter().any(|a| a.contains("org/repo")),
        "Should have positional model path: {:?}",
        args
    );
}

/// GGUF models should continue to work unchanged — -m <quant_file> is emitted.
#[test]
fn test_build_full_args_gguf_unchanged() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.hf_format = Some("gguf".to_string());
        s.context_length = Some(4096);
        s.num_parallel = Some(2);
        s.gpu_layers = Some(32);
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // GGUF should have -m with quant file path
    assert!(
        args.contains(&"-m".to_string()),
        "GGUF should have -m flag: {:?}",
        args
    );
    // The quant file path should be present (may contain 'org/repo' and end with a slash)
    assert!(
        args.iter().any(|a| a.contains("org/repo")),
        "GGUF should have model path in -m arg: {:?}",
        args
    );

    // And all llama.cpp flags should still be present
    assert!(args.contains(&"-c".to_string()));
    assert!(args.contains(&"-np".to_string()));
    assert!(args.contains(&"-ngl".to_string()));
}

/// Positional model path appears immediately after any default_args subcommand
/// (e.g. "serve") and before the first -- flag.
#[test]
fn test_build_full_args_transformers_positional_after_subcommand() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");

    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.backend = "vllm".to_string();
        s.hf_format = Some("transformers".to_string());
        s.model = Some("org/repo".to_string());
        s.quant = None;
        // Add a default_args subcommand
        s.args = vec!["serve".to_string()];
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Find the position of "serve" (subcommand) and the model path
    let serve_pos = args.iter().position(|a| *a == "serve");
    let model_path_pos = args.iter().position(|a| a.contains("org/repo"));

    assert!(
        serve_pos.is_some(),
        "Expected 'serve' subcommand in args: {:?}",
        args
    );
    assert!(
        model_path_pos.is_some(),
        "Expected positional model path in args: {:?}",
        args
    );

    // Model path should come right after the subcommand
    let serve = serve_pos.unwrap();
    let model_path = model_path_pos.unwrap();
    assert_eq!(
        model_path,
        serve + 1,
        "Model path should be immediately after 'serve' subcommand: {:?}",
        args
    );
}

/// GGUF models with default_args still work correctly — -m comes after subcommand.
#[test]
fn test_build_full_args_gguf_positional_after_subcommand() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.hf_format = Some("gguf".to_string());
        // Add a subcommand to default_args
        s.args = vec!["serve".to_string()];
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // GGUF should still have -m with quant file
    assert!(
        args.contains(&"-m".to_string()),
        "GGUF should have -m flag: {:?}",
        args
    );
}

/// Positional model path is NOT duplicated when the user already has it in args.
#[test]
fn test_build_full_args_transformers_no_duplicate_positional() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    // Create the directory structure for the model repo
    let org_dir = models_dir.join("org").join("repo");
    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");

    let config = h::sample_config(models_dir.clone());

    let server = h::sample_server(|s| {
        s.backend = "vllm".to_string();
        s.hf_format = Some("transformers".to_string());
        s.model = Some("org/repo".to_string());
        s.quant = None;
        // User already has the positional model path in args
        s.args = vec![
            "serve".to_string(),
            models_dir
                .join("org")
                .join("repo")
                .to_string_lossy()
                .to_string(),
        ];
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Count occurrences of the model path
    let repo_path_str = models_dir
        .join("org")
        .join("repo")
        .to_string_lossy()
        .to_string();
    let count = args.iter().filter(|a| a.contains(&repo_path_str)).count();
    assert_eq!(
        count, 1,
        "Positional model path should appear exactly once, got {}: {:?}",
        count, args
    );
}

/// Positional model path is NOT duplicated when the user has --model flag.
#[test]
fn test_build_full_args_transformers_no_duplicate_with_model_flag() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    // Create the directory structure for the model repo
    let org_dir = models_dir.join("org").join("repo");
    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");

    let config = h::sample_config(models_dir.clone());

    let server = h::sample_server(|s| {
        s.backend = "vllm".to_string();
        s.hf_format = Some("transformers".to_string());
        s.model = Some("org/repo".to_string());
        s.quant = None;
        // User already has --model flag in args
        s.args = vec![
            "serve".to_string(),
            "--model".to_string(),
            models_dir
                .join("org")
                .join("repo")
                .to_string_lossy()
                .to_string(),
        ];
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Count occurrences of the model path
    let repo_path_str = models_dir
        .join("org")
        .join("repo")
        .to_string_lossy()
        .to_string();
    let count = args.iter().filter(|a| a.contains(&repo_path_str)).count();
    assert_eq!(
        count, 1,
        "Positional model path should appear exactly once, got {}: {:?}",
        count, args
    );
}
