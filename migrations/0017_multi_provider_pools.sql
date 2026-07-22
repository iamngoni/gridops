ALTER TABLE runner_pools ADD COLUMN providers TEXT NOT NULL DEFAULT '["docker"]'
  CHECK (json_valid(providers) AND json_type(providers) = 'array');

ALTER TABLE runner_pools ADD COLUMN docker_image TEXT NOT NULL
  DEFAULT 'ghcr.io/actions/actions-runner:latest';

ALTER TABLE runner_pools ADD COLUMN tart_image TEXT NOT NULL
  DEFAULT 'gridops-macos-tahoe-base';

UPDATE runner_pools
SET providers = json_array(provider),
    docker_image = CASE WHEN provider = 'docker' THEN image ELSE docker_image END,
    tart_image = CASE WHEN provider = 'tart' THEN image ELSE tart_image END;

ALTER TABLE runners ADD COLUMN provider TEXT NOT NULL DEFAULT 'docker'
  CHECK (provider IN ('docker','tart'));

UPDATE runners
SET provider = COALESCE(
  (SELECT provider FROM runner_pools WHERE runner_pools.id = runners.pool_id),
  'docker'
);
