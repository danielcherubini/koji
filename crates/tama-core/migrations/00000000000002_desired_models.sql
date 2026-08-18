-- plan-191 Task 5: desired model state (proxy-side source of truth).
--
-- The proxy tracks which models SHOULD be loaded on which tamad; the
-- reconciler loop converges the tamads' actual process tables to this
-- desired set via LoadModel/UnloadModel RPCs (ADR-0010).
CREATE TABLE desired_models (
    model_name  TEXT PRIMARY KEY,
    tamad_id    TEXT NOT NULL REFERENCES tamad_registry(id),
    loaded_at   BIGINT NOT NULL
);

-- The reconciler ticks `WHERE tamad_id = $1` per tamad every second
-- (list_desired), so the lookup column gets its own index.
CREATE INDEX idx_desired_models_tamad ON desired_models(tamad_id);
