-- plan-193 T7: drop the `desired_models` shadow table.
--
-- The proxy no longer reads or writes this table: the tamad's host-side
-- store owns *desired* state (T1) and the proxy reads it off the wire
-- (`ProcessInfo.desired`, T3).
--
-- The pre-drop probe is DIAGNOSTIC ONLY: it counts the rows and, on a
-- non-zero count, RAISEs NOTICE with the count rendered — it logs
-- survivors but never blocks or defers anything. The DROP itself is
-- UNCONDITIONAL: a sqlx migration row that raises a notice and RETURNs
-- is still marked `success`, so a "log + skip, deferred to the next
-- cycle" promise could NEVER be retried — a one-shot skip would leave
-- the table alive forever. The zero-rows invariant is therefore
-- asserted in logs (the NOTICE), not in control flow.
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
        RAISE NOTICE
            'plan 193 T7 NOTICE: desired_models has % row(s) at drop time — dropping anyway; if you see this, a pre-193 proxy may still be steering — re-verify rollout-step ordering', n;
    END IF;
    DROP TABLE desired_models;
END
$$;

-- The `idx_desired_models_tamad` lookup index dies with the table; no
-- table references *into* `desired_models` (its only FK is the column-side
-- `tamad_registry(id)` reference; nothing points back at this table), so
-- a plain drop touches nothing else.
