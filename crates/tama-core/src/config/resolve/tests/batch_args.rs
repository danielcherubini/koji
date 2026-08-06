use crate::config::resolve::tests::test_helpers as h;

/// When n_batch is Some, build_full_args produces a `-b N` flag.
#[test]
fn test_build_full_args_injects_n_batch() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.n_batch = Some(4096);
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        args.contains(&"-b".to_string()),
        "Expected -b flag: {:?}",
        args
    );
    assert!(
        args.contains(&"4096".to_string()),
        "Expected 4096 value: {:?}",
        args
    );
}

/// When n_ubatch is Some, build_full_args produces a `-ub N` flag.
#[test]
fn test_build_full_args_injects_n_ubatch() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.n_ubatch = Some(512);
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        args.contains(&"-ub".to_string()),
        "Expected -ub flag: {:?}",
        args
    );
    assert!(
        args.contains(&"512".to_string()),
        "Expected 512 value: {:?}",
        args
    );
}

/// When both n_batch and n_ubatch are Some, both flags are produced.
#[test]
fn test_build_full_args_injects_both_batch_and_ubatch() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.n_batch = Some(4096);
        s.n_ubatch = Some(512);
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        args.contains(&"-b".to_string()),
        "Expected -b flag: {:?}",
        args
    );
    assert!(
        args.contains(&"4096".to_string()),
        "Expected 4096 value: {:?}",
        args
    );
    assert!(
        args.contains(&"-ub".to_string()),
        "Expected -ub flag: {:?}",
        args
    );
    assert!(
        args.contains(&"512".to_string()),
        "Expected 512 value: {:?}",
        args
    );
}

/// When n_batch/n_ubatch are None, no `-b`/`-ub` flags appear.
#[test]
fn test_build_full_args_none_batch_no_flags() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.n_batch = None;
        s.n_ubatch = None;
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| a == "-b"),
        "Should not have -b flag when n_batch is None: {:?}",
        args
    );
    assert!(
        !args.iter().any(|a| a == "-ub"),
        "Should not have -ub flag when n_ubatch is None: {:?}",
        args
    );
}

/// When n_batch Some(4096) and args already contain `-b 2048`,
/// only one `-b` appears (the typed field wins — no duplicate).
#[test]
fn test_build_full_args_no_duplicate_batch_flag() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.n_batch = Some(4096);
        // Legacy leftover flag in args
        s.args = vec!["-b 2048".to_string()];
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Only one -b flag should appear (from typed field, not from args)
    let b_count = args.iter().filter(|a| *a == "-b").count();
    assert_eq!(
        b_count, 1,
        "Expected exactly one -b flag, got {}: {:?}",
        b_count, args
    );

    // The value should be 4096 (typed field wins over leftover in args)
    let b_pos = args.iter().position(|a| *a == "-b").expect("no -b found");
    assert_eq!(
        args[b_pos + 1],
        "4096",
        "Expected value 4096 (typed field), got {}: {:?}",
        &args[b_pos + 1],
        args
    );
}

/// When n_ubatch Some(512) and args already contain `-ub 256`,
/// only one `-ub` appears (no duplicate).
#[test]
fn test_build_full_args_no_duplicate_ubatch_flag() {
    let (_temp_dir, models_dir) = h::temp_model_dir();
    let config = h::sample_config(models_dir);

    let server = h::sample_server(|s| {
        s.n_ubatch = Some(512);
        // Legacy leftover flag in args
        s.args = vec!["-ub 256".to_string()];
    });

    let backend = h::sample_backend();

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    let ub_count = args.iter().filter(|a| *a == "-ub").count();
    assert_eq!(
        ub_count, 1,
        "Expected exactly one -ub flag, got {}: {:?}",
        ub_count, args
    );

    let ub_pos = args.iter().position(|a| *a == "-ub").expect("no -ub found");
    assert_eq!(
        args[ub_pos + 1],
        "512",
        "Expected value 512 (typed field), got {}: {:?}",
        &args[ub_pos + 1],
        args
    );
}
