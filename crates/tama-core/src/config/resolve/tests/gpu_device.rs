use crate::config::resolve::tests::test_helpers as h;

/// When `gpu_device = Some("ROCm0")` and backend is llama_cpp, `--device` is NOT injected by
/// `build_full_args` — GPU isolation is now handled via env vars at spawn time instead.
#[test]
fn test_gpu_device_not_injected_as_cli_arg() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.gpu_device = Some("ROCm0".to_string());
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| *a == "--device"),
        "Expected no --device in args (env-var isolation used instead), got: {:?}",
        args
    );
}

/// When `gpu_device = None`, no `--device` flag is added.
#[test]
fn test_gpu_device_none_no_injection() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.gpu_device = None;
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| *a == "--device"),
        "Expected no --device when gpu_device is None, got: {:?}",
        args
    );
}

/// When `--device` is in `server.args` (user-provided), it is preserved by `build_full_args`.
/// The `gpu_device` config field no longer causes injection — only user-provided flags survive.
#[test]
fn test_user_device_flag_preserved() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.args = vec!["--device cuda0".to_string()];
        s.gpu_device = Some("ROCm0".to_string());
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // The user's --device cuda0 (from server.args) should be preserved
    assert!(
        args.windows(2).any(|w| w == ["--device", "cuda0"]),
        "User's --device cuda0 should be preserved, got: {:?}",
        args
    );
    // gpu_device should NOT cause an additional --device injection
    let device_count = args.iter().filter(|a| *a == "--device").count();
    assert_eq!(
        device_count, 1,
        "Expected exactly one --device (user's only, no injection from gpu_device), got {} in: {:?}",
        device_count, args
    );
}

/// When `gpu_device` is set but backend is non-llama.cpp, no `--device` flag is added.
#[test]
fn test_gpu_device_not_injected_for_non_llama_cpp() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.backend = "ik_llama".to_string();
        s.gpu_device = Some("ROCm0".to_string());
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| *a == "--device"),
        "Expected no --device for non-llama.cpp backend, got: {:?}",
        args
    );
}

/// When `gpu_device = Some("   ")`, no `--device` flag is added (whitespace-only).
#[test]
fn test_gpu_device_empty_string_no_injection() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.gpu_device = Some("   ".to_string());
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| *a == "--device"),
        "Expected no --device when gpu_device is whitespace-only, got: {:?}",
        args
    );
}
