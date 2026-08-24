//! Desired-model-state database queries (plan-191 Task 5).
//!
//! The proxy is the single writer of desired state (ADR-0010): the
//! The proxy writes this table; the tamad's own host-side store keeps
//! set via `LoadModel`/`UnloadModel` RPCs.

use anyhow::Result;
use sqlx::{PgPool, Row};

/// A row in `desired_models`: a model that should be loaded on a tamad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredModel {
    pub model_name: String,
    pub tamad_id: String,
    /// Unix epoch seconds when the model was marked desired.
    pub loaded_at: i64,
}

/// Mark a model as desired-loaded on a tamad (idempotent upsert).
pub async fn set_desired(pool: &PgPool, model_name: &str, tamad_id: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO desired_models (model_name, tamad_id, loaded_at)
         VALUES ($1, $2, EXTRACT(EPOCH FROM now())::BIGINT)
         ON CONFLICT (model_name) DO UPDATE SET
             tamad_id = EXCLUDED.tamad_id,
             loaded_at = EXCLUDED.loaded_at",
    )
    .bind(model_name)
    .bind(tamad_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a model's desired row. Returns `true` when a row was removed.
pub async fn clear_desired(pool: &PgPool, model_name: &str) -> Result<bool> {
    let result = sqlx::query("DELETE FROM desired_models WHERE model_name = $1")
        .bind(model_name)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Get the desired row for a model, if any.
pub async fn get_desired(pool: &PgPool, model_name: &str) -> Result<Option<DesiredModel>> {
    let row = sqlx::query(
        "SELECT model_name, tamad_id, loaded_at FROM desired_models WHERE model_name = $1",
    )
    .bind(model_name)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| DesiredModel {
        model_name: r.get("model_name"),
        tamad_id: r.get("tamad_id"),
        loaded_at: r.get("loaded_at"),
    }))
}

/// List all desired models, optionally filtered to one tamad.
pub async fn list_desired(pool: &PgPool, tamad_id: Option<&str>) -> Result<Vec<DesiredModel>> {
    let rows = match tamad_id {
        Some(tamad_id) => {
            sqlx::query(
                "SELECT model_name, tamad_id, loaded_at FROM desired_models \
                 WHERE tamad_id = $1 ORDER BY model_name",
            )
            .bind(tamad_id)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query(
                "SELECT model_name, tamad_id, loaded_at FROM desired_models \
                 ORDER BY model_name",
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows
        .into_iter()
        .map(|r| DesiredModel {
            model_name: r.get("model_name"),
            tamad_id: r.get("tamad_id"),
            loaded_at: r.get("loaded_at"),
        })
        .collect())
}

/// Remove all desired rows pointing at a tamad (used when a tamad is
/// deleted from the registry).
pub async fn clear_desired_for_tamad(pool: &PgPool, tamad_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM desired_models WHERE tamad_id = $1")
        .bind(tamad_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::postgres::with_schema;

    /// set_desired inserts a row; a second set with a different tamad
    /// upserts (model_name is the primary key).
    #[tokio::test]
    async fn test_set_desired_idempotent_and_upsert() {
        let guard = with_schema().await;
        let pool = &guard.pool;
        sqlx::query("INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('t1', 'a', 'grpc://a', 'grpc')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('t2', 'b', 'grpc://b', 'grpc')")
            .execute(pool)
            .await
            .unwrap();

        set_desired(pool, "model-x", "t1").await.unwrap();
        let row = get_desired(pool, "model-x").await.unwrap().unwrap();
        assert_eq!(row.tamad_id, "t1");
        assert!(row.loaded_at > 0);

        // Idempotent re-set.
        set_desired(pool, "model-x", "t1").await.unwrap();
        assert_eq!(
            list_desired(pool, None).await.unwrap().len(),
            1,
            "upsert must not duplicate"
        );

        // Conflict: same model, different tamad → row moves.
        set_desired(pool, "model-x", "t2").await.unwrap();
        let row = get_desired(pool, "model-x").await.unwrap().unwrap();
        assert_eq!(row.tamad_id, "t2");

        guard.finish().await;
    }

    /// clear_desired returns whether a row was actually removed.
    #[tokio::test]
    async fn test_clear_desired() {
        let guard = with_schema().await;
        let pool = &guard.pool;
        sqlx::query("INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('t1', 'a', 'grpc://a', 'grpc')")
            .execute(pool)
            .await
            .unwrap();

        assert!(
            !clear_desired(pool, "ghost").await.unwrap(),
            "clearing a non-existent model reports false"
        );

        set_desired(pool, "model-y", "t1").await.unwrap();
        assert!(clear_desired(pool, "model-y").await.unwrap());
        assert!(get_desired(pool, "model-y").await.unwrap().is_none());
        assert!(
            !clear_desired(pool, "model-y").await.unwrap(),
            "second clear reports false"
        );

        guard.finish().await;
    }

    /// list_desired filters by tamad when given one.
    #[tokio::test]
    async fn test_list_desired_filter() {
        let guard = with_schema().await;
        let pool = &guard.pool;
        sqlx::query("INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('t1', 'a', 'grpc://a', 'grpc')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('t2', 'b', 'grpc://b', 'grpc')")
            .execute(pool)
            .await
            .unwrap();

        set_desired(pool, "alpha", "t1").await.unwrap();
        set_desired(pool, "beta", "t1").await.unwrap();
        set_desired(pool, "gamma", "t2").await.unwrap();

        let all = list_desired(pool, None).await.unwrap();
        assert_eq!(all.len(), 3);

        let t1 = list_desired(pool, Some("t1")).await.unwrap();
        assert_eq!(t1.len(), 2);
        assert!(t1.iter().all(|d| d.tamad_id == "t1"));

        let t2 = list_desired(pool, Some("t2")).await.unwrap();
        assert_eq!(t2.len(), 1);
        assert_eq!(t2[0].model_name, "gamma");

        guard.finish().await;
    }

    /// clear_desired_for_tamad removes only that tamad's rows.
    #[tokio::test]
    async fn test_clear_desired_for_tamad() {
        let guard = with_schema().await;
        let pool = &guard.pool;
        sqlx::query("INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('t1', 'a', 'grpc://a', 'grpc')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('t2', 'b', 'grpc://b', 'grpc')")
            .execute(pool)
            .await
            .unwrap();

        set_desired(pool, "alpha", "t1").await.unwrap();
        set_desired(pool, "beta", "t2").await.unwrap();

        clear_desired_for_tamad(pool, "t1").await.unwrap();

        assert!(get_desired(pool, "alpha").await.unwrap().is_none());
        assert!(get_desired(pool, "beta").await.unwrap().is_some());

        guard.finish().await;
    }
}
