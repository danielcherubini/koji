-- plan-193 T7 (cycle 2 of the shadow drop): remove `active_models`.
--
-- The probe is this migration's own: the cycle 1 migration
-- (`00000000000004_drop_desired_models.sql`) probes ONLY `desired_models` — on a
-- non-empty count it RAISES NOTICE and SKIPs that drop,
-- deferring it to the next cycle; it never aborts the deploy,
-- and it never looks at `active_models`. This second cycle
-- drops the *active-models* shadow: the tally the proxy kept
-- of its own model roster.
-- It is the GATED half — the plan's exceptions admit the probe can go
-- non-zero on a real deployment; in that case the drop is deferred to the
-- next cycle (log + skip, not drift — the desired_models drop still
-- shipped in cycle 1). On a fresh database the count is 0 and the drop
-- succeeds.
--
-- Nothing to lose: no table references *into* `active_models` (no FK points
-- at it), and the other shadow (desired_models) is already gone, so the
-- drop touches nothing else.
DO $$
DECLARE
    n integer;
BEGIN
    SELECT count(*) INTO n FROM active_models;
    IF n > 0 THEN
        RAISE NOTICE
            'plan-193 T7 probe: active_models has % row(s) — deferring its DROP to the next cycle', n;
        RETURN;
    END IF;
    DROP TABLE active_models;
    RAISE NOTICE 'plan-193 T7 probe: active_models empty — dropped';
END
$$;
