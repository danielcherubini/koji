-- plan-195 task 3: durable structured-logging fields on the singleton
-- app_general row. Column DEFAULTs cover pre-existing rows from older
-- databases: a resumed database gains the columns with safe values
-- (no directives; the documented 7 day / 50k row / 256MB retention).
ALTER TABLE app_general ADD COLUMN log_directives TEXT;           -- NULL = none
ALTER TABLE app_general ADD COLUMN log_retention_days INTEGER NOT NULL DEFAULT 7;
ALTER TABLE app_general ADD COLUMN log_retention_rows BIGINT NOT NULL DEFAULT 50000;
ALTER TABLE app_general ADD COLUMN log_retention_max_mb BIGINT NOT NULL DEFAULT 256;
