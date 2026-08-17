//! Model alias database query functions (Postgres, plan-190 Task 5).
//!
//! All functions are async and take a `&PgPool` — the caller owns the pool.
//!
//! Case-insensitive parity: v2 used `COLLATE NOCASE` on `model_aliases.name`
//! (the v1 squashed migration intentionally has no case-insensitive index).
//! Duplicate detection is therefore made explicit with `lower()` in
//! [`insert_alias`] / [`update_alias`].

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

/// Response type for alias queries that include the resolved model name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasResponse {
    pub id: i64,
    pub name: String,
    pub model_id: i64,
    pub model_name: String, // COALESCE(api_name, repo_id)
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// SELECT list for [`AliasResponse`] rows (joined with `model_configs`).
const CACHE_SQL: &str = "SELECT a.name, COALESCE(m.api_name, m.repo_id) AS model_name \
     FROM model_aliases a JOIN model_configs m ON m.id = a.model_id \
     WHERE a.enabled ORDER BY a.name ASC";
const ALL_SQL: &str =
    "SELECT a.id, a.name, a.model_id, COALESCE(m.api_name, m.repo_id) AS model_name, \
     a.description, a.enabled, \
     to_char(a.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
     to_char(a.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_at \
     FROM model_aliases a JOIN model_configs m ON m.id = a.model_id \
     ORDER BY a.name ASC";
const BY_ID_SQL: &str =
    "SELECT a.id, a.name, a.model_id, COALESCE(m.api_name, m.repo_id) AS model_name, \
     a.description, a.enabled, \
     to_char(a.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
     to_char(a.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_at \
     FROM model_aliases a JOIN model_configs m ON m.id = a.model_id \
     WHERE a.id = $1";

/// Decode an alias row selected with `ALIAS_SELECT` order.
fn decode_alias(row: &sqlx::postgres::PgRow) -> AliasResponse {
    AliasResponse {
        id: row.get("id"),
        name: row.get("name"),
        model_id: row.get("model_id"),
        model_name: row.get("model_name"),
        description: row.get("description"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

/// Load all aliases joined with model_configs to get the resolved model name.
/// Used by ProxyState to populate the in-memory cache.
/// Returns (alias_name, resolved_model_name) pairs.
/// resolved_model_name = COALESCE(api_name, repo_id)
pub async fn load_aliases_for_cache(pool: &PgPool) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query(CACHE_SQL).fetch_all(pool).await?;
    Ok(rows
        .iter()
        .map(|r| (r.get("name"), r.get("model_name")))
        .collect())
}

/// Load all aliases with model names for the web API.
pub async fn get_all_aliases(pool: &PgPool) -> Result<Vec<AliasResponse>> {
    let rows = sqlx::query(ALL_SQL).fetch_all(pool).await?;
    Ok(rows.iter().map(decode_alias).collect())
}

/// Get a single alias by integer id.
pub async fn get_alias_by_id(pool: &PgPool, id: i64) -> Result<Option<AliasResponse>> {
    let row = sqlx::query(BY_ID_SQL).bind(id).fetch_optional(pool).await?;
    Ok(row.map(|r| decode_alias(&r)))
}

/// Insert a new alias. Returns the new row's id.
///
/// Case-insensitive duplicate check (v2 `COLLATE NOCASE` parity): a name that
/// differs from an existing alias only in case is rejected.
pub async fn insert_alias(
    pool: &PgPool,
    name: &str,
    model_id: i64,
    description: Option<&str>,
) -> Result<i64> {
    let dup: Option<i64> =
        sqlx::query("SELECT id FROM model_aliases WHERE lower(name) = lower($1)")
            .bind(name)
            .fetch_optional(pool)
            .await?
            .map(|r| r.get("id"));
    if dup.is_some() {
        return Err(anyhow!(
            "alias name '{}' already exists (case-insensitive)",
            name
        ));
    }
    let row = sqlx::query(
        "INSERT INTO model_aliases (name, model_id, description)
         VALUES ($1, $2, $3)
         RETURNING id",
    )
    .bind(name)
    .bind(model_id)
    .bind(description)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

/// Fields to update on an alias. `None` leaves the column unchanged;
/// `description: Some(None)` clears it.
#[derive(Debug, Default)]
pub struct AliasUpdate<'a> {
    pub name: Option<&'a str>,
    pub model_id: Option<i64>,
    pub description: Option<Option<&'a str>>,
    pub enabled: Option<bool>,
}

/// Update an existing alias. Only updates fields that are Some.
///
/// When `name` is provided, a case-insensitive duplicate check (v2 parity)
/// rejects it if another alias already holds the name modulo case.
pub async fn update_alias(pool: &PgPool, id: i64, update: AliasUpdate<'_>) -> Result<()> {
    if let Some(name) = update.name {
        let dup: Option<i64> =
            sqlx::query("SELECT id FROM model_aliases WHERE lower(name) = lower($1) AND id <> $2")
                .bind(name)
                .bind(id)
                .fetch_optional(pool)
                .await?
                .map(|r| r.get("id"));
        if dup.is_some() {
            return Err(anyhow!(
                "alias name '{}' already exists (case-insensitive)",
                name
            ));
        }
    }

    let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new("UPDATE model_aliases SET ");
    let mut first = true;
    if let Some(n) = update.name {
        if !first {
            qb.push(", ");
        }
        qb.push("name = ").push_bind(n);
        first = false;
    }
    if let Some(m) = update.model_id {
        if !first {
            qb.push(", ");
        }
        qb.push("model_id = ").push_bind(m);
        first = false;
    }
    if let Some(desc) = update.description {
        if !first {
            qb.push(", ");
        }
        qb.push("description = ").push_bind(desc);
        first = false;
    }
    if let Some(e) = update.enabled {
        if !first {
            qb.push(", ");
        }
        qb.push("enabled = ").push_bind(e);
        first = false;
    }

    if first {
        // Nothing to update.
        return Ok(());
    }

    qb.push(", updated_at = now() WHERE id = ").push_bind(id);
    qb.build().execute(pool).await?;
    Ok(())
}

/// Delete an alias by id.
pub async fn delete_alias(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM model_aliases WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
