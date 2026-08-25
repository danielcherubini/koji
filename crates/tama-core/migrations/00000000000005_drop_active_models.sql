-- plan-193 T7 (cycle 2 of the shadow drop): remove `active_models`.
--
-- This is the other shadow table: the tally the proxy kept of its own
-- model roster. Same shape as the cycle-1 migration
-- (`000000000004_drop_desired_models.sql`): probe FIRST, then drop —
-- except the probe here is now the GATE, guided + RETRYABLE
-- (round-2 P1): non-zero count → RAISE EXCEPTION (failed migration →
-- sqlx retries it on every subsequent boot); zero count → drop.
--
-- The no-steering premise holds by construction: only pre-plan-193
-- `tama` proxies ever steered this table (T5b/T7 removed the
-- steering). Rollout caveat: the drop must land AFTER the pre-plan-193
-- proxy was retired (rollout ladder step ordering).
--
-- Nothing to lose: no table references *into* `active_models` (no FK
-- points at it), and the other shadow (desired_models) is already
-- gone, so the drop touches nothing else.
DO $$
DECLARE
    n integer;
BEGIN
    SELECT count(*) INTO n FROM active_models;
    IF n > 0 THEN
        RAISE EXCEPTION
            'active_models still has % row(s) — these lifecycle tables must be empty before plan-193 T7 drop (a pre-plan-193 proxy may still own steering state in them). Verify no pre-plan-193 proxy exists, then DELETE the remaining rows; sqlx retries this migration on the next proxy boot.', n;
    END IF;
    DROP TABLE active_models;
END
$$;
