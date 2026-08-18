-- plan-191 Task 6: tamad-hosted pull routing.
-- The proxy's `pull_backend` names a registered tamad by connection id;
-- when set, queued model pulls are dispatched to that tamad (the download
-- runs on the tamad's disk) instead of the proxy downloading locally.
--
-- Referential safety: the FK rejects unregistered tamad ids loudly, and
-- the tamad delete path clears this column before deleting (see
-- `clear_pull_backend_for_tamad`), mirroring `desired_models.tamad_id`.
ALTER TABLE app_proxy ADD COLUMN IF NOT EXISTS pull_backend TEXT REFERENCES tamad_registry(id);
