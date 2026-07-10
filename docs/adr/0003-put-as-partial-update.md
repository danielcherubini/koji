# PUT as partial update (PATCH added as alias)

The Tama Management API uses PUT endpoints that behave as partial merges — only provided fields change, omitted fields preserve existing DB values. This deviates from standard REST where PUT means full replace. The decision was made to avoid breaking existing clients (web UI, programmatic) that send partial payloads, with PATCH added as a truly surgical alias. A future change may make PUT strict (full replace) once all clients migrate to PATCH.
