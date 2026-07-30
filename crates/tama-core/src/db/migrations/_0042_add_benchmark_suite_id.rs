pub const MIGRATION: super::Migration = (
    42,
    false,
    r#"ALTER TABLE benchmarks ADD COLUMN suite_id TEXT;"#,
);
