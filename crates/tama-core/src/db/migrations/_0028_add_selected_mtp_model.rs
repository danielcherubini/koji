/// v28 — Add selected_mtp_model column to model_configs.
/// Stores the MTP draft model filename (e.g. "mtp-F16.gguf") selected by the
/// user. Mirrors the existing `selected_mmproj` column for vision projectors.
/// Mirrors the COLLATE NOCASE used by other string columns on this table so
/// upserts compare case-insensitively.
pub const MIGRATION: (i32, bool, &str) = (
    28,
    false,
    r#"
        ALTER TABLE model_configs ADD COLUMN selected_mtp_model TEXT COLLATE NOCASE;
    "#,
);
