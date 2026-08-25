-- plan-193 T7: drop the `desired_models` shadow table.
--
-- The proxy no longer reads or writes this table: the tamad's host-side
-- store owns *desired* state (T1) and the proxy reads it off the wire
-- (`ProcessInfo.desired`, T3).
--
-- GUIDED + RETRYABLE (round-2 P1): non-zero count → RAISE EXCEPTION —
-- the failed migration is NOT recorded as applied, so sqlx retries it on
-- every subsequent boot; zero count → drop.
--
-- The no-steering premise holds by construction: only pre-plan-193
-- `tama` proxies ever steered this table (T5b/T7 removed the
-- steering). Rollout caveat: the drop must land AFTER the pre-plan-193
-- proxy was retired (rollout ladder step ordering).
DO $$
DECLARE
    n integer;
BEGIN
    SELECT count(*) INTO n FROM desired_models;
    IF n > 0 THEN
        RAISE EXCEPTION
            'desired_models still has % row(s) — these lifecycle tables must be empty before plan-193 T7 drop (a pre-plan-193 proxy may still own steering state in them). Verify no pre-plan-193 proxy exists, then DELETE the remaining rows; sqlx retries this migration on the next proxy boot.', n;
    END IF;
    DROP TABLE desired_models;
END
$$;

-- The `idx_desired_models_tamad` lookup index dies with the table; no
-- table references *into* `desired_models` (its only FK is the column-side
-- `tamad_registry(id)` reference; nothing points back at this table), so
-- a plain drop touches nothing else.
