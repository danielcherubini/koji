/// v16 — Add benchmark_type to benchmarks
pub const MIGRATION: (i32, bool, &str) = (
    16,
    false,
    r#"
        ALTER TABLE benchmarks ADD COLUMN benchmark_type TEXT;
    "#,
);
