//! Queries for the `api_keys` table.

use anyhow::Result;
use rusqlite::Connection;

/// Count the number of active (non-revoked, non-expired) API keys.
///
/// Used to derive the `api_keys_enabled` flag on the `app_proxy` table so
/// the flag can never drift from the actual key state. The flag is a
/// derived value — the source of truth is the `api_keys` table.
pub fn count_active_keys(conn: &Connection) -> Result<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL AND (expires_at IS NULL OR expires_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        [],
        |row| row.get(0),
    )?;
    Ok(count)
}
