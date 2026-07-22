ALTER TABLE runner_pools ADD COLUMN provider TEXT NOT NULL DEFAULT 'docker'
  CHECK (provider IN ('docker','tart'));
