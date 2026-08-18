//! Provider registry database query functions (Postgres, plan-190 Task 5).
//!
//! All functions are async and take a `&PgPool` — the caller owns the pool.
//! `created_at` is a unix epoch second (`BIGINT`), unchanged from v2.

use anyhow::Result;
use sqlx::{PgPool, Row};
use std::str::FromStr;

use crate::providers::{Engine, Provider, ProviderType};

// ─── Queries ────────────────────────────────────────────────────────────────

/// Insert a new provider record. Returns the auto-assigned row id.
pub async fn insert_provider(
    pool: &PgPool,
    name: &str,
    provider_type: &str,
    engine: &str,
    tamad_id: Option<&str>,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<i64> {
    let row = sqlx::query(
        "INSERT INTO provider_registry (name, provider_type, engine, tamad_id, base_url, api_key)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id",
    )
    .bind(name)
    .bind(provider_type)
    .bind(engine)
    .bind(tamad_id)
    .bind(base_url)
    .bind(api_key)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

/// Internal: raw provider row as (id, name, provider_type, engine, tamad_id, base_url, api_key, created_at).
type RawProviderRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
);

const BY_NAME_SQL: &str =
    "SELECT id, name, provider_type, engine, tamad_id, base_url, api_key, created_at \
     FROM provider_registry WHERE name = $1";
const BY_ID_SQL: &str =
    "SELECT id, name, provider_type, engine, tamad_id, base_url, api_key, created_at \
     FROM provider_registry WHERE id = $1";
const LIST_SQL: &str =
    "SELECT id, name, provider_type, engine, tamad_id, base_url, api_key, created_at \
     FROM provider_registry ORDER BY name ASC";

fn decode_provider_row(row: &sqlx::postgres::PgRow) -> RawProviderRow {
    (
        row.get("id"),
        row.get("name"),
        row.get("provider_type"),
        row.get("engine"),
        row.get("tamad_id"),
        row.get("base_url"),
        row.get("api_key"),
        row.get::<i64, _>("created_at"),
    )
}

/// Get a provider by name.
pub async fn get_provider(pool: &PgPool, name: &str) -> Result<Option<Provider>> {
    let row = sqlx::query(BY_NAME_SQL)
        .bind(name)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(raw_to_provider(decode_provider_row(&row))?))
}

/// Get a provider by id.
pub async fn get_provider_by_id(pool: &PgPool, id: i64) -> Result<Option<Provider>> {
    let row = sqlx::query(BY_ID_SQL).bind(id).fetch_optional(pool).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(raw_to_provider(decode_provider_row(&row))?))
}

/// List all providers ordered by name.
pub async fn list_providers(pool: &PgPool) -> Result<Vec<Provider>> {
    let rows = sqlx::query(LIST_SQL).fetch_all(pool).await?;
    rows.iter()
        .map(decode_provider_row)
        .map(raw_to_provider)
        .collect()
}

/// Convert a raw row tuple to a typed Provider.
fn raw_to_provider(row: RawProviderRow) -> Result<Provider> {
    let (id, name, provider_type, engine, tamad_id, base_url, api_key, created_at) = row;
    Ok(Provider {
        id,
        name,
        provider_type: ProviderType::from_str(&provider_type)
            .map_err(|e| anyhow::anyhow!("invalid provider_type '{}': {}", provider_type, e))?,
        engine: Engine::from_str(&engine)
            .map_err(|e| anyhow::anyhow!("invalid engine '{}': {}", engine, e))?,
        tamad_id,
        base_url,
        api_key,
        created_at,
    })
}

/// Update a provider's base_url and/or api_key.
pub async fn update_provider(
    pool: &PgPool,
    name: &str,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE provider_registry
         SET base_url = COALESCE($2, base_url),
             api_key = COALESCE($3, api_key)
         WHERE name = $1",
    )
    .bind(name)
    .bind(base_url)
    .bind(api_key)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a provider by name. Returns true if a row was deleted.
pub async fn delete_provider(pool: &PgPool, name: &str) -> Result<bool> {
    let result = sqlx::query("DELETE FROM provider_registry WHERE name = $1")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Null out `tamad_id` on every provider pointing at the given (deleted)
/// tamad, so providers don't keep a dangling reference. Returns the names
/// of the affected providers (plan-191 review fix).
pub async fn clear_tamad_id_for_tamad(pool: &PgPool, tamad_id: &str) -> Result<Vec<String>> {
    let rows: Vec<String> = sqlx::query_scalar(
        "UPDATE provider_registry SET tamad_id = NULL WHERE tamad_id = $1 RETURNING name",
    )
    .bind(tamad_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::postgres::with_schema;

    /// clear_tamad_id_for_tamad nulls only that tamad's providers and
    /// returns their names; a second clear affects nothing.
    #[tokio::test]
    async fn test_clear_tamad_id_for_tamad() {
        let guard = with_schema().await;
        let pool = &guard.pool;
        insert_provider(
            pool,
            "clr-a",
            "local",
            "llama_cpp",
            Some("tamad-x"),
            None,
            None,
        )
        .await
        .unwrap();
        insert_provider(
            pool,
            "clr-b",
            "local",
            "llama_cpp",
            Some("tamad-y"),
            None,
            None,
        )
        .await
        .unwrap();
        insert_provider(
            pool,
            "clr-c",
            "remote",
            "openai",
            None,
            Some("https://x"),
            None,
        )
        .await
        .unwrap();

        let names = clear_tamad_id_for_tamad(pool, "tamad-x").await.unwrap();
        assert_eq!(names, vec!["clr-a".to_string()]);

        let a = get_provider(pool, "clr-a").await.unwrap().unwrap();
        assert_eq!(a.tamad_id, None, "affected provider must be nulled");
        let b = get_provider(pool, "clr-b").await.unwrap().unwrap();
        assert_eq!(
            b.tamad_id,
            Some("tamad-y".to_string()),
            "other providers must be untouched"
        );

        assert!(
            clear_tamad_id_for_tamad(pool, "tamad-x")
                .await
                .unwrap()
                .is_empty(),
            "second clear must affect no rows"
        );

        guard.finish().await;
    }
}
