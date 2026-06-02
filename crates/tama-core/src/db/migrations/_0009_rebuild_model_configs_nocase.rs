/// v9 — Rebuild model_configs with COLLATE NOCASE on repo_id (FK_OFF)
pub const MIGRATION: (i32, bool, &str) = (
    9,
    true,
    r#"
        -- Rebuild model_configs with COLLATE NOCASE on repo_id so that
        -- the UNIQUE constraint, ON CONFLICT(repo_id) upserts, and
        -- WHERE repo_id = ? lookups all match case-insensitively. HF
        -- repo ids preserve original casing but users (and our own
        -- config-key normalisation) routinely lowercase them, so a
        -- binary UNIQUE index produced duplicate rows for the same repo.

        -- The migration runner toggles `PRAGMA foreign_keys=OFF`
        -- around this migration (see `FK_OFF_MIGRATIONS`). That is
        -- required because the `DROP TABLE` below would otherwise
        -- fire `ON DELETE CASCADE` on `model_files` / `model_pulls`
        -- and wipe every referencing row. `defer_foreign_keys` does
        -- NOT prevent cascade actions, only deferred enforcement
        -- checks, so it is the wrong tool here.

        -- Deduplicate any existing rows that differ only by case
        -- (keep the row with the lowest id). Without this, the new
        -- UNIQUE constraint would fail to enforce.

        -- Before deleting duplicate parents, remap child rows to the surviving
        -- parent id so nothing becomes orphaned.  kept_id maps each repo to the
        -- id that will survive (MIN(id) per LOWER(repo_id)).
        UPDATE model_files SET model_id = (
            SELECT kept.id FROM (
                SELECT MIN(id) AS id FROM model_configs GROUP BY LOWER(repo_id)
            ) kept
            JOIN model_configs mc ON mc.id = kept.id
            WHERE LOWER(mc.repo_id) = LOWER(model_files.repo_id)
        )
        WHERE model_id IN (
            SELECT id FROM model_configs WHERE id NOT IN (
                SELECT MIN(id) FROM model_configs GROUP BY LOWER(repo_id)
            )
        );

        UPDATE model_pulls SET model_id = (
            SELECT kept.id FROM (
                SELECT MIN(id) AS id FROM model_configs GROUP BY LOWER(repo_id)
            ) kept
            JOIN model_configs mc ON mc.id = kept.id
            WHERE LOWER(mc.repo_id) = LOWER(model_pulls.repo_id)
        )
        WHERE model_id IN (
            SELECT id FROM model_configs WHERE id NOT IN (
                SELECT MIN(id) FROM model_configs GROUP BY LOWER(repo_id)
            )
        );

        -- After remapping, deduplicate any (model_id, filename) collisions
        -- caused by the merge (keep the row with the lowest id).
        DELETE FROM model_files WHERE id NOT IN (
            SELECT MIN(id) FROM model_files GROUP BY model_id, filename
        );

        DELETE FROM model_configs WHERE id NOT IN (
            SELECT MIN(id) FROM model_configs GROUP BY LOWER(repo_id)
        );

        CREATE TABLE model_configs_new (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_id       TEXT NOT NULL UNIQUE COLLATE NOCASE,
            display_name  TEXT,
            backend       TEXT NOT NULL DEFAULT 'llama_cpp',
            enabled       INTEGER NOT NULL DEFAULT 1,
            selected_quant  TEXT,
            selected_mmproj TEXT,
            context_length  INTEGER,
            gpu_layers      INTEGER,
            port            INTEGER,
            args            TEXT,
            sampling        TEXT,
            modalities      TEXT,
            profile         TEXT,
            api_name        TEXT,
            health_check    TEXT,
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        INSERT INTO model_configs_new (
            id, repo_id, display_name, backend, enabled, selected_quant,
            selected_mmproj, context_length, gpu_layers, port, args,
            sampling, modalities, profile, api_name, health_check,
            created_at, updated_at
        )
        SELECT
            id, repo_id, display_name, backend, enabled, selected_quant,
            selected_mmproj, context_length, gpu_layers, port, args,
            sampling, modalities, profile, api_name, health_check,
            created_at, updated_at
        FROM model_configs;

        DROP TABLE model_configs;
        ALTER TABLE model_configs_new RENAME TO model_configs;
    "#,
);
