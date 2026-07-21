DELETE FROM repositories
WHERE NOT EXISTS (
    SELECT 1 FROM runner_pools WHERE runner_pools.repository_id = repositories.id
)
AND NOT EXISTS (
    SELECT 1 FROM workflow_runs WHERE workflow_runs.repository_id = repositories.id
);
