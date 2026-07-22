ALTER TABLE runner_pools ADD COLUMN provision_failure_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE runner_pools ADD COLUMN provision_retry_at INTEGER;
ALTER TABLE runner_pools ADD COLUMN provision_circuit_open INTEGER NOT NULL DEFAULT 0;
