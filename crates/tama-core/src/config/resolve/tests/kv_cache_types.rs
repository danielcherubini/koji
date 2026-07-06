use crate::config::resolve::tests::test_helpers as h;

/// Tests that -ctk and -ctv flags are injected when cache_type_k/v are set
/// and backend is llama.cpp.
#[test]
fn test_kv_cache_type_args_injected_when_set() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.cache_type_k = Some("q4_0".to_string());
        s.cache_type_v = Some("q8_0".to_string());
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // -ctk q4_0 should be present
    assert!(
        args.windows(2).any(|w| w == ["-ctk", "q4_0"]),
        "Expected -ctk q4_0 in args, got: {:?}",
        args
    );
    // -ctv q8_0 should be present
    assert!(
        args.windows(2).any(|w| w == ["-ctv", "q8_0"]),
        "Expected -ctv q8_0 in args, got: {:?}",
        args
    );
}

/// Tests that -ctk and -ctv are NOT injected when cache_type_k/v are None
/// on a llama.cpp backend.
#[test]
fn test_kv_cache_type_args_not_injected_when_none() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.cache_type_k = None;
        s.cache_type_v = None;
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // -ctk and -ctv should NOT be present
    assert!(
        !args.iter().any(|a| *a == "-ctk"),
        "Expected no -ctk when cache_type_k is None, got: {:?}",
        args
    );
    assert!(
        !args.iter().any(|a| *a == "-ctv"),
        "Expected no -ctv when cache_type_v is None, got: {:?}",
        args
    );
}

/// Tests that -ctk and -ctv are NOT injected for non-llama.cpp backends,
/// even when cache_type_k/v are set.
#[test]
fn test_kv_cache_type_args_not_injected_for_non_llama_backend() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.backend = "ollama".to_string();
        s.cache_type_k = Some("q4_0".to_string());
        s.cache_type_v = Some("q8_0".to_string());
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // -ctk and -ctv should NOT be present for non-llama.cpp backends
    assert!(
        !args.iter().any(|a| *a == "-ctk"),
        "Expected no -ctk for non-llama.cpp backend, got: {:?}",
        args
    );
    assert!(
        !args.iter().any(|a| *a == "-ctv"),
        "Expected no -ctv for non-llama.cpp backend, got: {:?}",
        args
    );
}

/// Tests that -ctk and -ctv are not duplicated when already present in
/// user-provided args on a llama.cpp backend.
#[test]
fn test_kv_cache_type_args_no_duplicate_when_in_user_args() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.args = vec!["-ctk f16".to_string(), "-ctv f16".to_string()];
        s.cache_type_k = Some("q4_0".to_string());
        s.cache_type_v = Some("q8_0".to_string());
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // -ctk should appear exactly once (from args, not injected)
    let ctk_count = args.iter().filter(|a| *a == "-ctk").count();
    assert_eq!(
        ctk_count, 1,
        "Expected exactly one -ctk (no duplicate), got {} in: {:?}",
        ctk_count, args
    );
    // -ctv should appear exactly once
    let ctv_count = args.iter().filter(|a| *a == "-ctv").count();
    assert_eq!(
        ctv_count, 1,
        "Expected exactly one -ctv (no duplicate), got {} in: {:?}",
        ctv_count, args
    );
}

/// Tests that -ctk and -ctv are NOT injected when cache_type_k/v are empty
/// strings on a llama.cpp backend.
#[test]
fn test_kv_cache_type_args_not_injected_for_empty_string() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.cache_type_k = Some("".to_string());
        s.cache_type_v = Some("".to_string());
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| *a == "-ctk"),
        "Expected no -ctk when cache_type_k is empty string, got: {:?}",
        args
    );
    assert!(
        !args.iter().any(|a| *a == "-ctv"),
        "Expected no -ctv when cache_type_v is empty string, got: {:?}",
        args
    );
}
