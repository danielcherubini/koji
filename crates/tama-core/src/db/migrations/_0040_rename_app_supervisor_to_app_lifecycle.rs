pub const MIGRATION: super::Migration = (
    40,
    false,
    r#"ALTER TABLE app_supervisor RENAME TO app_lifecycle;"#,
);
