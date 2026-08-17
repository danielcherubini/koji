//! Tamad registry database query functions.
//!
//! All functions take a `&PgPool` and are async (plan-190 Task 7).

use anyhow::Result;
use sqlx::{PgPool, Row};
use std::str::FromStr;

use crate::providers::{Protocol, TamadConnection, TamadStatus};

/// Convert a raw row into a typed `TamadConnection`.
fn row_to_tamad(row: &sqlx::postgres::PgRow) -> Result<TamadConnection> {
    let id: String = row.get("id");
    let name: String = row.get("name");
    let url: String = row.get("url");
    let protocol: String = row.get("protocol");
    let token: Option<String> = row.get("token");
    let status: String = row.get("status");
    Ok(TamadConnection {
        id,
        name,
        url,
        protocol: Protocol::from_str(&protocol)
            .map_err(|e| anyhow::anyhow!("invalid protocol '{}': {}", protocol, e))?,
        token,
        status: TamadStatus::from_str(&status)
            .map_err(|e| anyhow::anyhow!("invalid status '{}': {}", status, e))?,
    })
}

const GET_BY_ID_SQL: &str =
    "SELECT id, name, url, protocol, token, status FROM tamad_registry WHERE id = $1";
const LIST_SQL: &str =
    "SELECT id, name, url, protocol, token, status FROM tamad_registry ORDER BY name ASC";

/// Insert a new tamad connection record.
pub async fn insert_tamad(
    pool: &PgPool,
    id: &str,
    name: &str,
    url: &str,
    protocol: &str,
    token: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO tamad_registry (id, name, url, protocol, token)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(name)
    .bind(url)
    .bind(protocol)
    .bind(token)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get a tamad connection by id.
pub async fn get_tamad(pool: &PgPool, id: &str) -> Result<Option<TamadConnection>> {
    let row = sqlx::query(GET_BY_ID_SQL)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|r| row_to_tamad(&r)).transpose()
}

/// List all tamad connections ordered by name.
pub async fn list_tamads(pool: &PgPool) -> Result<Vec<TamadConnection>> {
    let rows = sqlx::query(LIST_SQL).fetch_all(pool).await?;
    rows.iter().map(row_to_tamad).collect::<Result<Vec<_>>>()
}

/// Update a tamad connection's url and/or token.
pub async fn update_tamad(pool: &PgPool, id: &str, url: &str, token: Option<&str>) -> Result<()> {
    sqlx::query(
        "UPDATE tamad_registry
         SET url = $2,
             token = COALESCE($3, token)
         WHERE id = $1",
    )
    .bind(id)
    .bind(url)
    .bind(token)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a tamad connection by id. Returns true if a row was deleted.
pub async fn delete_tamad(pool: &PgPool, id: &str) -> Result<bool> {
    let result = sqlx::query("DELETE FROM tamad_registry WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Update only the status field of a tamad connection.
pub async fn update_tamad_status(pool: &PgPool, id: &str, status: &str) -> Result<()> {
    sqlx::query("UPDATE tamad_registry SET status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::postgres::with_schema;

    // ── insert_tamad ──

    #[tokio::test]
    async fn test_insert_tamad() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert_tamad(
            pool,
            "uuid-001",
            "local-tamad",
            "grpc://localhost:50051",
            "grpc",
            Some("secret-token"),
        )
        .await
        .unwrap();

        let tamad = get_tamad(pool, "uuid-001").await.unwrap().unwrap();
        assert_eq!(tamad.id, "uuid-001");
        assert_eq!(tamad.name, "local-tamad");
        assert_eq!(tamad.url, "grpc://localhost:50051");
        assert!(tamad.protocol.is_grpc());
        assert_eq!(tamad.token, Some("secret-token".to_string()));
        assert!(tamad.status.is_unknown()); // default

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_insert_tamad_http() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert_tamad(
            pool,
            "uuid-002",
            "remote-tamad",
            "http://192.168.1.100:8080",
            "http",
            None,
        )
        .await
        .unwrap();

        let tamad = get_tamad(pool, "uuid-002").await.unwrap().unwrap();
        assert!(tamad.protocol.is_http());
        assert!(tamad.token.is_none());

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_insert_duplicate_id_fails() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert_tamad(
            pool,
            "uuid-001",
            "tamad-one",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .await
        .unwrap();

        let result = insert_tamad(
            pool,
            "uuid-001",
            "tamad-two",
            "grpc://localhost:50052",
            "grpc",
            None,
        )
        .await;
        assert!(result.is_err());

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_insert_duplicate_name_fails() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert_tamad(
            pool,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .await
        .unwrap();

        let result = insert_tamad(
            pool,
            "uuid-002",
            "my-tamad",
            "grpc://localhost:50052",
            "grpc",
            None,
        )
        .await;
        assert!(result.is_err());

        guard.finish().await;
    }

    // ── get_tamad ──

    #[tokio::test]
    async fn test_get_tamad_not_found() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        let result = get_tamad(pool, "nonexistent").await.unwrap();
        assert!(result.is_none());

        guard.finish().await;
    }

    // ── list_tamads ──

    #[tokio::test]
    async fn test_list_tamads_empty() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        let result = list_tamads(pool).await.unwrap();
        assert!(result.is_empty());

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_list_tamads_ordered() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert_tamad(
            pool,
            "uuid-z",
            "z-tamad",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .await
        .unwrap();
        insert_tamad(
            pool,
            "uuid-a",
            "a-tamad",
            "http://localhost:8080",
            "http",
            None,
        )
        .await
        .unwrap();

        let result = list_tamads(pool).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "a-tamad");
        assert_eq!(result[1].name, "z-tamad");

        guard.finish().await;
    }

    // ── update_tamad ──

    #[tokio::test]
    async fn test_update_tamad_url() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert_tamad(
            pool,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            Some("old-token"),
        )
        .await
        .unwrap();

        update_tamad(pool, "uuid-001", "grpc://localhost:50099", None)
            .await
            .unwrap();

        let tamad = get_tamad(pool, "uuid-001").await.unwrap().unwrap();
        assert_eq!(tamad.url, "grpc://localhost:50099");
        // token unchanged when None passed
        assert_eq!(tamad.token, Some("old-token".to_string()));

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_update_tamad_token() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert_tamad(
            pool,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            Some("old-token"),
        )
        .await
        .unwrap();

        update_tamad(
            pool,
            "uuid-001",
            "grpc://localhost:50051",
            Some("new-token"),
        )
        .await
        .unwrap();

        let tamad = get_tamad(pool, "uuid-001").await.unwrap().unwrap();
        assert_eq!(tamad.token, Some("new-token".to_string()));

        guard.finish().await;
    }

    // ── delete_tamad ──

    #[tokio::test]
    async fn test_delete_tamad() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert_tamad(
            pool,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .await
        .unwrap();

        let deleted = delete_tamad(pool, "uuid-001").await.unwrap();
        assert!(deleted);
        assert!(get_tamad(pool, "uuid-001").await.unwrap().is_none());

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_delete_tamad_not_found() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        let deleted = delete_tamad(pool, "nonexistent").await.unwrap();
        assert!(!deleted);

        guard.finish().await;
    }

    // ── update_tamad_status ──

    #[tokio::test]
    async fn test_update_tamad_status() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert_tamad(
            pool,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .await
        .unwrap();

        // Default status is unknown
        let tamad = get_tamad(pool, "uuid-001").await.unwrap().unwrap();
        assert!(tamad.status.is_unknown());

        update_tamad_status(pool, "uuid-001", "online")
            .await
            .unwrap();

        let tamad = get_tamad(pool, "uuid-001").await.unwrap().unwrap();
        assert!(tamad.status.is_online());

        update_tamad_status(pool, "uuid-001", "offline")
            .await
            .unwrap();

        let tamad = get_tamad(pool, "uuid-001").await.unwrap().unwrap();
        assert!(tamad.status.is_offline());

        guard.finish().await;
    }
}
