use crate::config::resolve::tests::test_helpers as h;
use crate::config::types::{QuantKind, SpecDecodingConfig};
use crate::config::Config;

/// Tests that spec decoding flags (--spec-type, --spec-draft-n-max, --spec-draft-n-min)
/// are injected when spec_decoding is configured on a llama.cpp backend.
#[test]
fn test_spec_decoding_flags_injected() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string(), "ngram-simple".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        };
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type should be injected with comma-separated types
    assert!(
        args.contains(&"--spec-type".to_string()),
        "Expected --spec-type flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"draft-mtp,ngram-simple".to_string()),
        "Expected spec types value, got: {:?}",
        args
    );

    // --spec-draft-n-max should be injected
    assert!(
        args.contains(&"--spec-draft-n-max".to_string()),
        "Expected --spec-draft-n-max flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"4".to_string()),
        "Expected n_max=4, got: {:?}",
        args
    );

    // --spec-draft-n-min should be injected
    assert!(
        args.contains(&"--spec-draft-n-min".to_string()),
        "Expected --spec-draft-n-min flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"2".to_string()),
        "Expected n_min=2, got: {:?}",
        args
    );

    // --spec-draft-ngl should be injected (draft-mtp is in spec_types)
    assert!(
        args.contains(&"--spec-draft-ngl".to_string()),
        "Expected --spec-draft-ngl flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"16".to_string()),
        "Expected draft_ngl=16, got: {:?}",
        args
    );
}

/// Tests that if the user already has --spec-type in their args, we don't inject another.
#[test]
fn test_spec_decoding_no_duplicate_when_in_args() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // User manually added --spec-type in args
    let server = h::sample_server(|s| {
        s.args = vec!["--spec-type draft-mtp".to_string()];
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string(), "ngram-simple".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: None,
        };
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type should appear exactly once (user's version, not duplicated)
    let spec_type_count = args.iter().filter(|a| *a == "--spec-type").count();
    assert_eq!(
        spec_type_count, 1,
        "--spec-type should appear exactly once, got {} in: {:?}",
        spec_type_count, args
    );

    // n_max and n_min should still be injected (they weren't in user args)
    assert!(
        args.contains(&"--spec-draft-n-max".to_string()),
        "Expected --spec-draft-n-max flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"--spec-draft-n-min".to_string()),
        "Expected --spec-draft-n-min flag, got: {:?}",
        args
    );
}

/// Tests that --spec-draft-ngl is only injected when "draft-mtp" is in spec_types.
#[test]
fn test_spec_decoding_draft_ngl_only_for_mtp() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // spec_types does NOT contain "draft-mtp", so draft_ngl should NOT be injected
    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["ngram-simple".to_string()], // No draft-mtp
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16), // Set but should be ignored without draft-mtp
        };
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type should be injected
    assert!(
        args.contains(&"--spec-type".to_string()),
        "Expected --spec-type flag, got: {:?}",
        args
    );

    // --spec-draft-ngl should NOT be injected (no draft-mtp in spec_types)
    assert!(
        !args.contains(&"--spec-draft-ngl".to_string()),
        "Expected no --spec-draft-ngl when draft-mtp not in spec_types, got: {:?}",
        args
    );
}

/// Tests that draft_ngl=99 is injected as-is (not truncated, not quoted).
#[test]
fn test_spec_decoding_draft_ngl_value_99() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(99),
        };
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-draft-ngl should be present
    assert!(
        args.contains(&"--spec-draft-ngl".to_string()),
        "Expected --spec-draft-ngl flag, got: {:?}",
        args
    );

    // Value should be "99" — not truncated, not quoted
    assert!(
        args.contains(&"99".to_string()),
        "Expected draft_ngl value 99, got: {:?}",
        args
    );

    // Verify the value is exactly "99" (not "9" or "'99'")
    let ngl_pos = args
        .iter()
        .position(|a| a == "--spec-draft-ngl")
        .expect("--spec-draft-ngl not found");
    let ngl_value = &args[ngl_pos + 1];
    assert_eq!(
        ngl_value, "99",
        "draft_ngl value should be exactly '99', got '{}'",
        ngl_value
    );
}

/// Tests that empty spec_types produces no spec decoding flags.
#[test]
fn test_spec_decoding_empty_types_no_flags() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // Empty spec_types → no spec decoding flags should be injected
    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec![], // Empty
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        };
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // No spec decoding flags should be present
    assert!(
        !args.contains(&"--spec-type".to_string()),
        "Expected no --spec-type when spec_types is empty, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-max".to_string()),
        "Expected no --spec-draft-n-max when spec_types is empty, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-min".to_string()),
        "Expected no --spec-draft-n-min when spec_types is empty, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-ngl".to_string()),
        "Expected no --spec-draft-ngl when spec_types is empty, got: {:?}",
        args
    );
}

/// Tests that `--spec-draft-model <path>` is injected when `mtp_model` is set,
/// `draft-mtp` is in `spec_types`, and the referenced quant has `kind = Mtp`.
#[test]
fn test_mtp_model_injected_when_draft_mtp_enabled() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");
    let mtp_file = org_dir.join("mtp-F16.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");
    std::fs::write(&mtp_file, b"dummy mtp content").expect("Failed to write mtp file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: crate::config::types::QuantKind::Model,
            size_bytes: None,
            context_length: Some(8192),
        },
    );
    quants.insert(
        "mtp-F16".to_string(),
        crate::config::types::QuantEntry {
            file: "mtp-F16.gguf".to_string(),
            kind: crate::config::types::QuantKind::Mtp,
            size_bytes: None,
            context_length: None,
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());

    let server = h::sample_server(|s| {
        s.model = Some("org/repo".to_string());
        s.quant = Some("Q4_K_M".to_string());
        s.mtp_model = Some("mtp-F16".to_string());
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.quants = quants;
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: None,
            n_min: None,
            draft_ngl: None,
        };
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-draft-model flag should be injected
    assert!(
        args.contains(&"--spec-draft-model".to_string()),
        "Expected --spec-draft-model flag, got: {:?}",
        args
    );

    // The value should be the resolved mtp path
    let expected_path = org_dir.join("mtp-F16.gguf").to_string_lossy().to_string();
    assert!(
        args.contains(&expected_path),
        "Expected mtp path '{}' in args, got: {:?}",
        expected_path,
        args
    );
}

/// Tests that `--spec-draft-model` is NOT injected when `mtp_model` is set
/// but `draft-mtp` is NOT in `spec_types`.
#[test]
fn test_mtp_model_not_injected_without_draft_mtp() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");
    let mtp_file = org_dir.join("mtp-F16.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");
    std::fs::write(&mtp_file, b"dummy mtp content").expect("Failed to write mtp file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: crate::config::types::QuantKind::Model,
            size_bytes: None,
            context_length: Some(8192),
        },
    );
    quants.insert(
        "mtp-F16".to_string(),
        crate::config::types::QuantEntry {
            file: "mtp-F16.gguf".to_string(),
            kind: crate::config::types::QuantKind::Mtp,
            size_bytes: None,
            context_length: None,
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());

    let server = h::sample_server(|s| {
        s.model = Some("org/repo".to_string());
        s.quant = Some("Q4_K_M".to_string());
        s.mtp_model = Some("mtp-F16".to_string());
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.quants = quants;
        s.spec_decoding = SpecDecodingConfig {
            // Empty spec_types - draft-mtp not enabled
            spec_types: vec![],
            n_max: None,
            n_min: None,
            draft_ngl: None,
        };
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-draft-model should NOT be injected
    assert!(
        !args.contains(&"--spec-draft-model".to_string()),
        "Expected no --spec-draft-model when draft-mtp not in spec_types, got: {:?}",
        args
    );
}

/// Tests that no duplicate `--spec-draft-model` is injected when the user already has
/// it in their `args`.
#[test]
fn test_mtp_model_no_duplicate_when_in_args() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");
    let mtp_file = org_dir.join("mtp-F16.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");
    std::fs::write(&mtp_file, b"dummy mtp content").expect("Failed to write mtp file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: crate::config::types::QuantKind::Model,
            size_bytes: None,
            context_length: Some(8192),
        },
    );
    quants.insert(
        "mtp-F16".to_string(),
        crate::config::types::QuantEntry {
            file: "mtp-F16.gguf".to_string(),
            kind: crate::config::types::QuantKind::Mtp,
            size_bytes: None,
            context_length: None,
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());

    // User already has --spec-draft-model in args
    let server = h::sample_server(|s| {
        s.args = vec!["--spec-draft-model /custom/path.gguf".to_string()];
        s.model = Some("org/repo".to_string());
        s.quant = Some("Q4_K_M".to_string());
        s.mtp_model = Some("mtp-F16".to_string());
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.quants = quants;
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: None,
            n_min: None,
            draft_ngl: None,
        };
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-draft-model should appear exactly once (user's version, not duplicated)
    let mtp_count = args.iter().filter(|a| *a == "--spec-draft-model").count();
    assert_eq!(
        mtp_count, 1,
        "--spec-draft-model should appear exactly once, got {} in: {:?}",
        mtp_count, args
    );
}

/// Tests that `--spec-draft-model` is NOT injected when `mtp_model` is None,
/// even when `draft-mtp` is in `spec_types`.
#[test]
fn test_mtp_model_not_injected_when_none() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    // mtp_model is None (default) but draft-mtp is in spec_types
    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: None,
            n_min: None,
            draft_ngl: None,
        };
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-draft-model should NOT be injected
    assert!(
        !args.contains(&"--spec-draft-model".to_string()),
        "Expected no --spec-draft-model when mtp_model is None, got: {:?}",
        args
    );
}

/// Tests that a warning is logged (no panic) when the `mtp_model` entry
/// has a `kind` that is not `Mtp`.
#[test]
fn test_mtp_model_warns_on_kind_mismatch() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");
    // mtp_name points at a file with kind=Model (mismatched)
    let wrong_kind_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");
    std::fs::write(&wrong_kind_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: crate::config::types::QuantKind::Model,
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());

    // mtp_model references a quant that has kind=Model (mismatched)
    let server = h::sample_server(|s| {
        s.model = Some("org/repo".to_string());
        s.quant = Some("Q4_K_M".to_string());
        s.mtp_model = Some("Q4_K_M".to_string());
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
        s.quants = quants;
        s.spec_decoding = SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: None,
            n_min: None,
            draft_ngl: None,
        };
    });

    let backend = h::sample_backend();

    // Should not panic - the kind mismatch should emit a warning and
    // skip the --spec-draft-model injection.
    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-draft-model should NOT be injected since the kind doesn't match
    assert!(
        !args.contains(&"--spec-draft-model".to_string()),
        "Expected no --spec-draft-model when kind mismatches, got: {:?}",
        args
    );
}
