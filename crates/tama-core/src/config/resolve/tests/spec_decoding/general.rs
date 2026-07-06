use crate::config::resolve::tests::test_helpers as h;
use crate::config::types::SpecDecodingConfig;

/// Tests that multiple spec_types are joined with commas in --spec-type.
#[test]
fn test_spec_decoding_multi_type_comma_separated() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec![
                "draft-mtp".to_string(),
                "ngram-simple".to_string(),
                "ngram-mod".to_string(),
            ],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: None,
        };
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type value should be comma-separated
    assert!(
        args.contains(&"--spec-type".to_string()),
        "Expected --spec-type flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"draft-mtp,ngram-simple,ngram-mod".to_string()),
        "Expected comma-separated spec types, got: {:?}",
        args
    );
}

/// Tests that a non-llama backend does NOT inject spec decoding flags.
#[test]
fn test_spec_decoding_non_llama_backend_no_flags() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // Use a non-llama backend
    let server = h::sample_server(|s| {
        s.backend = "ollama".to_string();
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        };
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // No spec decoding flags should be present for non-llama backend
    assert!(
        !args.contains(&"--spec-type".to_string()),
        "Expected no --spec-type for non-llama backend, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-max".to_string()),
        "Expected no --spec-draft-n-max for non-llama backend, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-min".to_string()),
        "Expected no --spec-draft-n-min for non-llama backend, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-ngl".to_string()),
        "Expected no --spec-draft-ngl for non-llama backend, got: {:?}",
        args
    );
}

/// Tests that each of the 4 spec decoding flags has its own already_has guard,
/// so pre-existing flags in user args are not duplicated.
#[test]
fn test_spec_decoding_all_already_has_checks() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // User already has all 4 flags in args
    let server = h::sample_server(|s| {
        s.args = vec![
            "--spec-type user-type".to_string(),
            "--spec-draft-n-max 8".to_string(),
            "--spec-draft-n-min 1".to_string(),
            "--spec-draft-ngl 32".to_string(),
        ];
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        };
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Each flag should appear exactly once (user's version, not duplicated)
    let spec_type_count = args.iter().filter(|a| *a == "--spec-type").count();
    assert_eq!(
        spec_type_count, 1,
        "--spec-type should appear exactly once, got {} in: {:?}",
        spec_type_count, args
    );

    let n_max_count = args.iter().filter(|a| *a == "--spec-draft-n-max").count();
    assert_eq!(
        n_max_count, 1,
        "--spec-draft-n-max should appear exactly once, got {} in: {:?}",
        n_max_count, args
    );

    let n_min_count = args.iter().filter(|a| *a == "--spec-draft-n-min").count();
    assert_eq!(
        n_min_count, 1,
        "--spec-draft-n-min should appear exactly once, got {} in: {:?}",
        n_min_count, args
    );

    let ngl_count = args.iter().filter(|a| *a == "--spec-draft-ngl").count();
    assert_eq!(
        ngl_count, 1,
        "--spec-draft-ngl should appear exactly once, got {} in: {:?}",
        ngl_count, args
    );
}
