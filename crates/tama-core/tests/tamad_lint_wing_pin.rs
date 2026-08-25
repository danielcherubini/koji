//! Shape pin for PROST codegen lint denial.
//!
//! `crates/tama-core/build.rs` codegens `OUT_DIR/tamad.rs` (tonic/prost
//! RPC wrappers from `proto/tamad.proto`), and that generated code is
//! not touchable by our edits — so the `result_large_err` denial lives
//! in the CARGO MANIFEST at the crate level (a `[lints]` table pinning
//! `result_large_err` to `allow`), not in per-file lints and not in CI
//! env. The trips for that lint are probe-sensitive: at the same
//! pinned toolchain (1.95.0) local ubuntu hits 1 generated fn and CI
//! ubuntu hits 8+ — prost codegen's `size_of` wobbles per
//! environment, so no fixed threshold parity can be expected.
//!
//! This test asserts the shape: a `[lints` section exists, and within
//! it `result_large_err` maps to `allow`. Remove the pin and this
//! breaks the workspace gate.

use std::fs;
use std::path::Path;

#[test]
/// The tamad codegen denial is linted at the MANIFEST level (a `[lints]`
/// entry), not flagged via per-file lints or CI env — so the shape is
/// verified here and nothing lighter can remove it.
fn test_tamad_codeline_is_linted_not_flagged() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let text = fs::read_to_string(&manifest).expect("read crate manifest");
    let lines: Vec<&str> = text.lines().collect();

    // Locate the first `[lints` section header (e.g. `[lints]` /
    // `[lints.clippy]`).
    let lints_at = lines
        .iter()
        .position(|l| l.starts_with("[lints"))
        .expect("Cargo.toml must define a [lints] section");

    // Within that table, find the `result_large_err` key before the next
    // `[`-braced section header opens.
    let table: &[&str] = &lines[lints_at + 1..];
    let mut key_at = None;
    let mut next_section = None;
    for (i, line) in table.iter().enumerate() {
        if line.starts_with('[') {
            next_section = Some(i);
            break;
        }
        if line.starts_with("result_large_err") {
            key_at = Some(i);
            break;
        }
    }

    let Some(key_at) = key_at else {
        panic!("no `result_large_err` key in the [lints] table");
    };

    // The value pinning `allow` must be on the key line or on a
    // continuation line before the next section header.
    let limit = next_section.unwrap_or(table.len());
    let pinned = (key_at..limit).any(|i| table[i].contains("= \"allow\""));
    assert!(
        pinned,
        "`result_large_err` in the [lints] table must be pinned to \"allow\""
    );
}
