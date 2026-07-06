use crate::config::resolve::tests::test_helpers as h;

/// Tests that context length is multiplied by num_parallel in build_full_args.
#[test]
fn test_build_full_args_context_multiplied_by_num_parallel() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    let server = h::sample_server(|s| {
        s.context_length = Some(4096);
        s.num_parallel = Some(2);
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Context should be 4096 * 2 = 8192
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"8192".to_string()),
        "Expected -c 8192 (4096*2), got: {:?}",
        args
    );
    // Raw context value should NOT appear alone
    assert!(
        !args.contains(&"4096".to_string()),
        "Raw context 4096 should not appear, got: {:?}",
        args
    );
}

/// Tests that saturating_mul prevents overflow for large context × num_parallel.
#[test]
fn test_build_full_args_context_saturating_overflow() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // context_length=1_000_000, num_parallel=10_000
    // 1_000_000 * 10_000 = 10_000_000_000 > u32::MAX (4_294_967_295)
    // saturating_mul should clamp to u32::MAX without panicking
    let server = h::sample_server(|s| {
        s.context_length = Some(1_000_000);
        s.num_parallel = Some(10_000);
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args should not panic with large values");

    assert!(args.contains(&"-c".to_string()));
    // Should be clamped to u32::MAX (4294967295), not overflow
    assert!(
        args.contains(&"4294967295".to_string()),
        "Expected -c 4294967295 (u32::MAX from saturating_mul), got: {:?}",
        args
    );
}

/// Tests that context is NOT multiplied when num_parallel is None (defaults to 1).
#[test]
fn test_build_full_args_context_no_num_parallel_defaults_to_one() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // num_parallel is None → should default to 1, so ctx stays at 8192
    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = None; // No parallel setting
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Context should be 8192 * 1 = 8192 (unchanged)
    assert!(args.contains(&"-c".to_string()));
    assert!(args.contains(&"8192".to_string()));
}

/// Tests that -np flag is injected when num_parallel > 1.
#[test]
fn test_build_full_args_injects_np_flag() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // num_parallel=2 → should inject -np 2
    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = Some(2);
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // -np flag should be present with value 2
    assert!(
        args.contains(&"-np".to_string()),
        "Expected -np flag in args, got: {:?}",
        args
    );
    assert!(
        args.contains(&"2".to_string()),
        "Expected value 2 after -np, got: {:?}",
        args
    );
    // -c should still be multiplied by num_parallel
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"16384".to_string()),
        "Expected -c 16384 (8192*2), got: {:?}",
        args
    );
}

/// Tests that -np flag is NOT injected when num_parallel is 0 (auto).
#[test]
fn test_build_full_args_no_np_when_auto() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // num_parallel=0 → should NOT inject -np (0 = auto)
    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = Some(0);
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // -np should NOT be present when num_parallel is 0 (auto)
    assert!(
        !args.contains(&"-np".to_string()),
        "Expected no -np flag when num_parallel=0, got: {:?}",
        args
    );
}

/// Tests that -np flag IS injected when num_parallel is 1.
#[test]
fn test_build_full_args_np_when_one() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);
    let backend = h::sample_backend();

    // num_parallel=1 → should inject -np 1
    let server = h::sample_server(|s| {
        s.context_length = Some(8192);
        s.num_parallel = Some(1);
    });

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // -np 1 SHOULD be present when num_parallel is 1
    assert!(
        args.contains(&"-np".to_string()),
        "Expected -np flag when num_parallel=1, got: {:?}",
        args
    );
    assert!(
        args.contains(&"1".to_string()),
        "Expected value 1 after -np flag, got: {:?}",
        args
    );
}
