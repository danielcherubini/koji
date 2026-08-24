-- plan-193 T7: drop the `desired_models` shadow table.
--
-- The proxy no longer reads or writes this table: the tamad's host-side
-- store owns *desired* state (T1) and the proxy reads it off the wire
-- (`ProcessInfo.desired`, T3).
--
-- Data guard: probe `desired_models` itself before the drop. Plan rule:
-- "Any row = abort (log + skip; note in the commit)" — so a non-empty
-- `desired_models` defers this drop to the next cycle (RAISE NOTICE +
-- skip, NOT a hard failure): the drop MUST not crash proxy startup.
-- A non-empty `active_models` is deliberately NOT observed here: its gate
-- lives entirely in migration 00000000000005 (log + skip, next cycle).
-- "Drop desired_models (always)": zero rows → the drop always happens.
DO $$
DECLARE
    n integer;
BEGIN
    SELECT count(*) INTO n FROM desired_models;
    IF n > 0 THEN
        RAISE NOTICE
            'plan-193 T7 probe: desired_models has % row(s) — deferring its DROP to the next cycle', n;
        RETURN;
    END IF;
    DROP TABLE desired_models;
    RAISE NOTICE 'plan-193 T7 probe: desired_models empty — dropped';
END
$$;

-- The `idx_desired_models_tamad` lookup index dies with the table; no
-- table references *into* `desired_models` (its only FK is the column-side
-- `tamad_registry(id)` reference; nothing points back at this table), so
-- a plain drop touches nothing else.
