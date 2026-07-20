import { unlink } from "node:fs/promises";

import { nanoid } from "nanoid";

import { reconcileRunnerPool } from "../features/runner-pools/runner-pools.server";
import { getSqlite, migrateDatabase } from "../server/db/client.server";

type PoolCandidate = {
  id: string;
  userId: string;
  login: string;
  autoscalingEnabled: number;
  desiredCount: number;
  minCount: number;
  idleTimeoutMinutes: number;
  repositoryId: number | null;
  installationId: number;
};

let stopping = false;

function settingNumber(key: string, fallback: number) {
  const row = getSqlite().prepare("SELECT value FROM settings WHERE key = ?").get(key) as { value: string } | undefined;
  if (!row) return fallback;
  try {
    const value = Number(JSON.parse(row.value));
    return Number.isFinite(value) ? value : fallback;
  } catch {
    return fallback;
  }
}

function poolCandidates() {
  return getSqlite().prepare(`
    SELECT p.id, p.autoscaling_enabled AS autoscalingEnabled,
      p.desired_count AS desiredCount, p.min_count AS minCount,
      p.idle_timeout_minutes AS idleTimeoutMinutes,
      p.repository_id AS repositoryId, p.installation_id AS installationId,
      u.id AS userId, u.login
    FROM runner_pools p
    JOIN user_installations ui ON ui.installation_id = p.installation_id
    JOIN users u ON u.id = ui.user_id
    WHERE p.paused = 0 AND p.state != 'deleting'
    GROUP BY p.id
    ORDER BY p.created_at ASC
  `).all() as PoolCandidate[];
}

function maybeScaleDown(pool: PoolCandidate) {
  if (!pool.autoscalingEnabled || pool.desiredCount <= pool.minCount) return;
  const cutoff = Date.now() - pool.idleTimeoutMinutes * 60_000;
  const activity = getSqlite().prepare(`
    SELECT
      COUNT(CASE WHEN r.deleted_at IS NULL AND r.busy = 1 THEN 1 END) AS busy,
      MAX(CASE WHEN r.deleted_at IS NULL THEN r.updated_at END) AS lastRunnerUpdate
    FROM runners r WHERE r.pool_id = ?
  `).get(pool.id) as { busy: number; lastRunnerUpdate: number | null };

  const queued = pool.repositoryId
    ? (getSqlite().prepare(`
        SELECT COUNT(*) AS count FROM workflow_jobs wj
        JOIN workflow_runs wr ON wr.id = wj.run_id
        WHERE wr.repository_id = ? AND wj.status = 'queued'
      `).get(pool.repositoryId) as { count: number }).count
    : (getSqlite().prepare(`
        SELECT COUNT(*) AS count FROM workflow_jobs wj
        JOIN workflow_runs wr ON wr.id = wj.run_id
        JOIN repositories repo ON repo.id = wr.repository_id
        WHERE repo.installation_id = ? AND wj.status = 'queued'
      `).get(pool.installationId) as { count: number }).count;

  if (queued > 0 || activity.busy > 0 || (activity.lastRunnerUpdate ?? Date.now()) > cutoff) return;
  getSqlite().prepare(`
    UPDATE runner_pools SET desired_count = ?, state = 'scaling', updated_at = ? WHERE id = ?
  `).run(pool.minCount, Date.now(), pool.id);
  getSqlite().prepare(`
    INSERT INTO audit_events (id, actor_label, action, target_type, target_id, metadata, created_at)
    VALUES (?, 'system', 'runner_pool.scaled_down', 'runner_pool', ?, ?, ?)
  `).run(nanoid(), pool.id, JSON.stringify({ desiredCount: pool.minCount }), Date.now());
}

async function reconcileAll() {
  migrateDatabase();
  for (const pool of poolCandidates()) {
    try {
      maybeScaleDown(pool);
      await reconcileRunnerPool({ id: pool.userId, login: pool.login }, pool.id);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Unknown reconciliation error";
      getSqlite().prepare(`
        INSERT INTO runner_events (id, pool_id, level, event, message, metadata, created_at)
        VALUES (?, ?, 'error', 'Reconciliation failed', ?, '{}', ?)
      `).run(nanoid(), pool.id, message.slice(0, 2_000), Date.now());
      console.error(`[reconciler] ${pool.id}: ${message}`);
    }
  }
}

async function cleanupRetention() {
  const now = Date.now();
  getSqlite().prepare("DELETE FROM sessions WHERE expires_at < ?").run(now);
  getSqlite().prepare("DELETE FROM oauth_states WHERE expires_at < ?").run(now);

  const webhookCutoff = now - settingNumber("webhookRetentionDays", 90) * 86_400_000;
  const auditCutoff = now - settingNumber("auditRetentionDays", 365) * 86_400_000;
  getSqlite().prepare("DELETE FROM webhook_deliveries WHERE received_at < ?").run(webhookCutoff);
  getSqlite().prepare("DELETE FROM audit_events WHERE created_at < ?").run(auditCutoff);

  const expired = getSqlite().prepare(`
    SELECT id, path FROM log_streams WHERE expires_at IS NOT NULL AND expires_at < ?
  `).all(now) as Array<{ id: string; path: string }>;
  for (const stream of expired) {
    await unlink(stream.path).catch(() => undefined);
    getSqlite().prepare("DELETE FROM log_streams WHERE id = ?").run(stream.id);
  }
}

async function loop() {
  while (!stopping) {
    const started = Date.now();
    await reconcileAll();
    await cleanupRetention();
    const interval = Math.max(5, settingNumber("reconcileIntervalSeconds", 30)) * 1_000;
    const wait = Math.max(1_000, interval - (Date.now() - started));
    await new Promise((resolve) => setTimeout(resolve, wait));
  }
}

process.on("SIGTERM", () => { stopping = true; });
process.on("SIGINT", () => { stopping = true; });

console.info("GridOps reconciler started");
await loop();
