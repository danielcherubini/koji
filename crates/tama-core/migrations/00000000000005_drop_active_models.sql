-- plan-193 T7 (cycle 2 of the shadow drop): remove `active_models`.
--
-- This is the other shadow table: the tally the proxy kept of its own
-- model roster. Same shape as the cycle-1 migration
-- (`000000000004_drop_desired_models.sql`): probe FIRST, then drop —
-- except the probe is DIAGNOSTIC ONLY here too (count, non-zero →
-- RAISE NOTICE with the count rendered; it never blocks or defers
-- anything).
--
-- The DROP is UNCONDITIONAL: a sqlx migration row that raises a notice
-- and RETURNs is still marked `success`, so a "log + skip, deferred to
-- the next cycle" promise could NEVER be retried — a one-shot skip
-- would leave the table alive forever. The zero-rows invariant is
-- asserted in logs (the NOTICE), not in control flow.
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
        RAISE NOTICE
            'plan 193 T7 NOTICE: active_models has % row(s) at drop time — dropping anyway; if you see this, a pre-193 proxy may still be steering — re-verify rollout-step ordering', n;
    END IF;
    DROP TABLE active_models;
END
$$;
