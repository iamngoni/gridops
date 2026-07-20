import "@tanstack/react-start/server-only";

import { nanoid } from "nanoid";

import { getConfigurationState, getConfig } from "~/server/config.server";
import { getSqlite, migrateDatabase } from "~/server/db/client.server";
import { getValidGitHubAccessToken } from "~/server/github/token.server";
import { syncUserInstallations } from "~/server/github/oauth.server";
import { githubRequest } from "~/server/github/api.server";
import { getRunnerManagerHealth, runnerManagerRequest } from "~/server/runner-manager/client.server";

export type AuthenticatedUser = { id: string; login: string };

type RawPool = {
  id: string;
  name: string;
  scope: string;
  mode: string;
  labels: string;
  image: string;
  desiredCount: number;
  minCount: number;
  maxCount: number;
  cpuLimit: number;
  memoryLimitMb: number;
  paused: number;
  state: string;
  accountLogin: string;
  repository: string | null;
  totalRunners: number;
  onlineRunners: number;
  busyRunners: number;
  failedRunners: number;
  createdAt: number;
};

type RawRunner = {
  id: string;
  name: string;
  status: string;
  busy: number;
  ephemeral: number;
  os: string;
  architecture: string;
  containerId: string | null;
  githubRunnerId: number | null;
  failureReason: string | null;
  registeredAt: number | null;
  lastHeartbeatAt: number | null;
  createdAt: number;
  poolId: string;
  poolName: string;
  poolPaused: number;
  accountLogin: string;
  repository: string | null;
  currentJobName: string | null;
  currentRunId: number | null;
};

type RawRepository = {
  id: number;
  fullName: string;
  private: number;
  archived: number;
  defaultBranch: string;
  htmlUrl: string;
  permission: string | null;
  lastSyncedAt: number;
  installationId: number;
  accountLogin: string;
  accountType: string;
  repositorySelection: string;
  poolCount: number;
  runCount: number;
  lastRunAt: number | null;
};

type RawWorkflowRun = {
  id: number;
  workflowName: string;
  runNumber: number;
  runAttempt: number;
  event: string;
  status: string;
  conclusion: string | null;
  headBranch: string | null;
  headSha: string;
  actorLogin: string | null;
  htmlUrl: string;
  startedAt: number | null;
  completedAt: number | null;
  createdAt: number;
  repository: string;
  owner: string;
  repositoryName: string;
  jobCount: number;
  activeJobs: number;
  failedJobs: number;
};

type RawWebhook = {
  id: string;
  event: string;
  action: string | null;
  installationId: number | null;
  repositoryId: number | null;
  signatureValid: number;
  status: string;
  error: string | null;
  receivedAt: number;
  processedAt: number | null;
  accountLogin: string | null;
  repository: string | null;
};

type RawAuditEvent = {
  id: string;
  actorLabel: string;
  action: string;
  targetType: string;
  targetId: string | null;
  metadata: string;
  ipAddress: string | null;
  createdAt: number;
};

type RawLogTarget = {
  id: string;
  name: string;
  status: string;
  busy: number;
  containerId: string;
  updatedAt: number;
  poolName: string;
  repository: string | null;
};

function iso(value: number | null) {
  return value ? new Date(value).toISOString() : null;
}

function jsonArray(value: string) {
  try {
    const parsed = JSON.parse(value);
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : [];
  } catch {
    return [];
  }
}

export function listRunnerPools(user: AuthenticatedUser) {
  migrateDatabase();
  const rows = getSqlite().prepare(`
    SELECT
      p.id, p.name, p.scope, p.mode, p.labels, p.image,
      p.desired_count AS desiredCount, p.min_count AS minCount,
      p.max_count AS maxCount, p.cpu_limit AS cpuLimit,
      p.memory_limit_mb AS memoryLimitMb, p.paused, p.state,
      i.account_login AS accountLogin, repo.full_name AS repository,
      COUNT(CASE WHEN r.deleted_at IS NULL THEN 1 END) AS totalRunners,
      COUNT(CASE WHEN r.deleted_at IS NULL AND r.status IN ('online', 'idle', 'busy') THEN 1 END) AS onlineRunners,
      COUNT(CASE WHEN r.deleted_at IS NULL AND r.busy = 1 THEN 1 END) AS busyRunners,
      COUNT(CASE WHEN r.deleted_at IS NULL AND r.status = 'failed' THEN 1 END) AS failedRunners,
      p.created_at AS createdAt
    FROM runner_pools p
    JOIN user_installations ui ON ui.installation_id = p.installation_id AND ui.user_id = ?
    JOIN installations i ON i.id = p.installation_id
    LEFT JOIN repositories repo ON repo.id = p.repository_id
    LEFT JOIN runners r ON r.pool_id = p.id
    GROUP BY p.id
    ORDER BY p.created_at DESC
  `).all(user.id) as RawPool[];

  return rows.map((row) => ({
    ...row,
    labels: jsonArray(row.labels),
    paused: Boolean(row.paused),
    createdAt: iso(row.createdAt)!,
  }));
}

export function listRunners(user: AuthenticatedUser) {
  migrateDatabase();
  const rows = getSqlite().prepare(`
    SELECT
      r.id, r.name, r.status, r.busy, r.ephemeral, r.os, r.architecture,
      r.container_id AS containerId, r.github_runner_id AS githubRunnerId,
      r.failure_reason AS failureReason, r.registered_at AS registeredAt,
      r.last_heartbeat_at AS lastHeartbeatAt, r.created_at AS createdAt,
      p.id AS poolId, p.name AS poolName, p.paused AS poolPaused,
      i.account_login AS accountLogin, repo.full_name AS repository,
      wj.name AS currentJobName, wj.run_id AS currentRunId
    FROM runners r
    JOIN runner_pools p ON p.id = r.pool_id
    JOIN user_installations ui ON ui.installation_id = p.installation_id AND ui.user_id = ?
    JOIN installations i ON i.id = p.installation_id
    LEFT JOIN repositories repo ON repo.id = p.repository_id
    LEFT JOIN workflow_jobs wj ON wj.id = r.current_job_id
    WHERE r.deleted_at IS NULL
    ORDER BY r.created_at DESC
  `).all(user.id) as RawRunner[];

  return rows.map((row) => ({
    ...row,
    busy: Boolean(row.busy),
    ephemeral: Boolean(row.ephemeral),
    poolPaused: Boolean(row.poolPaused),
    registeredAt: iso(row.registeredAt),
    lastHeartbeatAt: iso(row.lastHeartbeatAt),
    createdAt: iso(row.createdAt)!,
  }));
}

export function listRepositories(user: AuthenticatedUser) {
  migrateDatabase();
  const rows = getSqlite().prepare(`
    SELECT
      repo.id, repo.full_name AS fullName, repo.private, repo.archived,
      repo.default_branch AS defaultBranch, repo.html_url AS htmlUrl,
      repo.permission, repo.last_synced_at AS lastSyncedAt,
      i.id AS installationId, i.account_login AS accountLogin,
      i.account_type AS accountType, i.repository_selection AS repositorySelection,
      COUNT(DISTINCT p.id) AS poolCount,
      COUNT(DISTINCT wr.id) AS runCount,
      MAX(wr.github_updated_at) AS lastRunAt
    FROM repositories repo
    JOIN user_installations ui ON ui.installation_id = repo.installation_id AND ui.user_id = ?
    JOIN installations i ON i.id = repo.installation_id
    LEFT JOIN runner_pools p ON p.repository_id = repo.id
    LEFT JOIN workflow_runs wr ON wr.repository_id = repo.id
    GROUP BY repo.id
    ORDER BY repo.full_name ASC
  `).all(user.id) as RawRepository[];

  return rows.map((row) => ({
    ...row,
    private: Boolean(row.private),
    archived: Boolean(row.archived),
    lastSyncedAt: iso(row.lastSyncedAt)!,
    lastRunAt: iso(row.lastRunAt),
  }));
}

export function listWorkflowRuns(user: AuthenticatedUser) {
  migrateDatabase();
  const rows = getSqlite().prepare(`
    SELECT
      wr.id, wr.workflow_name AS workflowName, wr.run_number AS runNumber,
      wr.run_attempt AS runAttempt, wr.event, wr.status, wr.conclusion,
      wr.head_branch AS headBranch, wr.head_sha AS headSha,
      wr.actor_login AS actorLogin, wr.html_url AS htmlUrl,
      wr.started_at AS startedAt, wr.completed_at AS completedAt,
      wr.github_created_at AS createdAt, repo.full_name AS repository,
      repo.owner, repo.name AS repositoryName,
      COUNT(wj.id) AS jobCount,
      COUNT(CASE WHEN wj.status = 'in_progress' THEN 1 END) AS activeJobs,
      COUNT(CASE WHEN wj.conclusion = 'failure' THEN 1 END) AS failedJobs
    FROM workflow_runs wr
    JOIN repositories repo ON repo.id = wr.repository_id
    JOIN user_installations ui ON ui.installation_id = repo.installation_id AND ui.user_id = ?
    LEFT JOIN workflow_jobs wj ON wj.run_id = wr.id
    GROUP BY wr.id
    ORDER BY wr.github_created_at DESC
    LIMIT 250
  `).all(user.id) as RawWorkflowRun[];

  return rows.map((row) => ({
    ...row,
    startedAt: iso(row.startedAt),
    completedAt: iso(row.completedAt),
    createdAt: iso(row.createdAt)!,
  }));
}

export function getWorkflowRunDetail(user: AuthenticatedUser, runId: number) {
  migrateDatabase();
  const run = getSqlite().prepare(`
    SELECT wr.id, wr.workflow_name AS workflowName, wr.run_number AS runNumber,
      wr.run_attempt AS runAttempt, wr.event, wr.status, wr.conclusion,
      wr.head_branch AS headBranch, wr.head_sha AS headSha,
      wr.actor_login AS actorLogin, wr.html_url AS htmlUrl,
      wr.started_at AS startedAt, wr.completed_at AS completedAt,
      wr.github_created_at AS createdAt, repo.full_name AS repository
    FROM workflow_runs wr
    JOIN repositories repo ON repo.id = wr.repository_id
    JOIN user_installations ui ON ui.installation_id = repo.installation_id
    WHERE wr.id = ? AND ui.user_id = ?
  `).get(runId, user.id) as Omit<RawWorkflowRun, "owner" | "repositoryName" | "jobCount" | "activeJobs" | "failedJobs"> | undefined;
  if (!run) throw new Error("Workflow run does not exist or is not accessible.");

  const jobs = getSqlite().prepare(`
    SELECT id, name, status, conclusion, runner_name AS runnerName,
      runner_group_name AS runnerGroupName, labels, html_url AS htmlUrl,
      started_at AS startedAt, completed_at AS completedAt
    FROM workflow_jobs WHERE run_id = ? ORDER BY created_at ASC
  `).all(runId) as Array<{
    id: number; name: string; status: string; conclusion: string | null;
    runnerName: string | null; runnerGroupName: string | null; labels: string;
    htmlUrl: string; startedAt: number | null; completedAt: number | null;
  }>;

  return {
    ...run,
    startedAt: iso(run.startedAt),
    completedAt: iso(run.completedAt),
    createdAt: iso(run.createdAt)!,
    jobs: jobs.map((job) => ({
      ...job,
      labels: jsonArray(job.labels),
      startedAt: iso(job.startedAt),
      completedAt: iso(job.completedAt),
    })),
  };
}

export function listWebhookDeliveries(user: AuthenticatedUser) {
  migrateDatabase();
  const rows = getSqlite().prepare(`
    SELECT
      wd.id, wd.event, wd.action, wd.installation_id AS installationId,
      wd.repository_id AS repositoryId, wd.signature_valid AS signatureValid,
      wd.status, wd.error, wd.received_at AS receivedAt,
      wd.processed_at AS processedAt, i.account_login AS accountLogin,
      repo.full_name AS repository
    FROM webhook_deliveries wd
    LEFT JOIN installations i ON i.id = wd.installation_id
    LEFT JOIN repositories repo ON repo.id = wd.repository_id
    WHERE wd.installation_id IS NULL OR EXISTS (
      SELECT 1 FROM user_installations ui
      WHERE ui.installation_id = wd.installation_id AND ui.user_id = ?
    )
    ORDER BY wd.received_at DESC
    LIMIT 250
  `).all(user.id) as RawWebhook[];

  return rows.map((row) => ({
    ...row,
    signatureValid: Boolean(row.signatureValid),
    receivedAt: iso(row.receivedAt)!,
    processedAt: iso(row.processedAt),
  }));
}

export function listAuditEvents(user: AuthenticatedUser) {
  migrateDatabase();
  const rows = getSqlite().prepare(`
    SELECT id, actor_label AS actorLabel, action, target_type AS targetType,
      target_id AS targetId, metadata, ip_address AS ipAddress, created_at AS createdAt
    FROM audit_events
    WHERE actor_user_id = ? OR actor_label = 'system'
    ORDER BY created_at DESC
    LIMIT 500
  `).all(user.id) as RawAuditEvent[];

  return rows.map((row) => ({
    ...row,
    createdAt: iso(row.createdAt)!,
  }));
}

export function listLogTargets(user: AuthenticatedUser) {
  migrateDatabase();
  const rows = getSqlite().prepare(`
    SELECT r.id, r.name, r.status, r.busy, r.container_id AS containerId,
      r.updated_at AS updatedAt, p.name AS poolName, repo.full_name AS repository
    FROM runners r
    JOIN runner_pools p ON p.id = r.pool_id
    JOIN user_installations ui ON ui.installation_id = p.installation_id AND ui.user_id = ?
    LEFT JOIN repositories repo ON repo.id = p.repository_id
    WHERE r.deleted_at IS NULL AND r.container_id IS NOT NULL
    ORDER BY r.busy DESC, r.updated_at DESC
  `).all(user.id) as RawLogTarget[];

  return rows.map((row) => ({
    ...row,
    busy: Boolean(row.busy),
    updatedAt: iso(row.updatedAt)!,
  }));
}

export async function getSettingsOverview(user: AuthenticatedUser) {
  migrateDatabase();
  const stored = Object.fromEntries(
    (getSqlite().prepare("SELECT key, value FROM settings").all() as Array<{ key: string; value: string }>).map(
      (item) => [item.key, JSON.parse(item.value)],
    ),
  );

  let manager: { ok: boolean; dockerVersion?: string; apiVersion?: string; error?: string };
  try {
    const health = await getRunnerManagerHealth();
    manager = { ok: true, dockerVersion: health.dockerVersion, apiVersion: health.apiVersion };
  } catch (error) {
    manager = { ok: false, error: error instanceof Error ? error.message : "Runner manager unavailable" };
  }

  return {
    configuration: getConfigurationState(),
    manager,
    settings: {
      logRetentionDays: Number(stored.logRetentionDays ?? 30),
      webhookRetentionDays: Number(stored.webhookRetentionDays ?? 90),
      auditRetentionDays: Number(stored.auditRetentionDays ?? 365),
      reconcileIntervalSeconds: Number(stored.reconcileIntervalSeconds ?? 30),
      autoUpdateImages: Boolean(stored.autoUpdateImages ?? false),
    },
    user: { login: user.login },
    databasePath: getConfig().databasePath,
  };
}

export async function syncGitHubData(user: AuthenticatedUser) {
  const token = await getValidGitHubAccessToken(user.id);
  await syncUserInstallations(user.id, token);
  const count = (getSqlite().prepare(`
    SELECT COUNT(*) AS count FROM repositories repo
    JOIN user_installations ui ON ui.installation_id = repo.installation_id
    WHERE ui.user_id = ?
  `).get(user.id) as { count: number }).count;
  audit(user, "github.synced", "github_account", user.login, { repositories: count });
  return { repositories: count };
}

export async function controlWorkflowRun(
  user: AuthenticatedUser,
  input: { runId: number; action: "cancel" | "rerun" | "rerun-failed" },
) {
  migrateDatabase();
  const run = getSqlite().prepare(`
    SELECT wr.id, repo.owner, repo.name
    FROM workflow_runs wr
    JOIN repositories repo ON repo.id = wr.repository_id
    JOIN user_installations ui ON ui.installation_id = repo.installation_id
    WHERE wr.id = ? AND ui.user_id = ?
  `).get(input.runId, user.id) as { id: number; owner: string; name: string } | undefined;
  if (!run) throw new Error("Workflow run does not exist or is not accessible.");

  const endpoint = input.action === "cancel"
    ? "cancel"
    : input.action === "rerun-failed" ? "rerun-failed-jobs" : "rerun";
  const token = await getValidGitHubAccessToken(user.id);
  await githubRequest<void>(`/repos/${run.owner}/${run.name}/actions/runs/${run.id}/${endpoint}`, token, {
    method: "POST",
  });
  audit(user, `workflow_run.${input.action}`, "workflow_run", String(run.id), {});
  return { ok: true };
}

export async function saveSystemSettings(
  user: AuthenticatedUser,
  values: Record<string, number | boolean>,
) {
  migrateDatabase();
  const statement = getSqlite().prepare(`
    INSERT INTO settings (key, value, updated_by, updated_at)
    VALUES (?, ?, ?, ?)
    ON CONFLICT(key) DO UPDATE SET value = excluded.value,
      updated_by = excluded.updated_by, updated_at = excluded.updated_at
  `);
  const write = getSqlite().transaction(() => {
    for (const [key, value] of Object.entries(values)) {
      statement.run(key, JSON.stringify(value), user.id, Date.now());
    }
  });
  write();
  audit(user, "settings.updated", "system", "gridops", values);
  return { ok: true };
}

export async function getRunnerLogs(user: AuthenticatedUser, runnerId: string) {
  migrateDatabase();
  const runner = getSqlite().prepare(`
    SELECT r.container_id AS containerId, r.name
    FROM runners r
    JOIN runner_pools p ON p.id = r.pool_id
    JOIN user_installations ui ON ui.installation_id = p.installation_id
    WHERE r.id = ? AND ui.user_id = ? AND r.deleted_at IS NULL
  `).get(runnerId, user.id) as { containerId: string | null; name: string } | undefined;
  if (!runner?.containerId) throw new Error("Runner has no active container log stream.");
  const logs = await runnerManagerRequest<string>(`/v1/runners/${runner.containerId}/logs`);
  return { runnerId, name: runner.name, logs };
}

export function searchControlPlane(user: AuthenticatedUser, query: string) {
  migrateDatabase();
  const pattern = `%${query.trim().replaceAll("%", "\\%").replaceAll("_", "\\_")}%`;
  const repositories = getSqlite().prepare(`
    SELECT 'repository' AS kind, CAST(repo.id AS TEXT) AS id,
      repo.full_name AS title, i.account_login AS subtitle,
      '/repositories' AS href
    FROM repositories repo
    JOIN installations i ON i.id = repo.installation_id
    JOIN user_installations ui ON ui.installation_id = repo.installation_id
    WHERE ui.user_id = ? AND repo.full_name LIKE ? ESCAPE '\\'
    ORDER BY repo.full_name LIMIT 5
  `).all(user.id, pattern) as SearchResult[];
  const pools = getSqlite().prepare(`
    SELECT 'runner pool' AS kind, p.id, p.name AS title,
      COALESCE(repo.full_name, i.account_login) AS subtitle,
      '/runner-pools' AS href
    FROM runner_pools p
    JOIN installations i ON i.id = p.installation_id
    JOIN user_installations ui ON ui.installation_id = p.installation_id
    LEFT JOIN repositories repo ON repo.id = p.repository_id
    WHERE ui.user_id = ? AND p.name LIKE ? ESCAPE '\\'
    ORDER BY p.name LIMIT 5
  `).all(user.id, pattern) as SearchResult[];
  const runnersFound = getSqlite().prepare(`
    SELECT 'runner' AS kind, r.id, r.name AS title, p.name AS subtitle,
      '/runners' AS href
    FROM runners r JOIN runner_pools p ON p.id = r.pool_id
    JOIN user_installations ui ON ui.installation_id = p.installation_id
    WHERE ui.user_id = ? AND r.deleted_at IS NULL AND r.name LIKE ? ESCAPE '\\'
    ORDER BY r.updated_at DESC LIMIT 5
  `).all(user.id, pattern) as SearchResult[];
  const runs = getSqlite().prepare(`
    SELECT 'workflow run' AS kind, CAST(wr.id AS TEXT) AS id,
      wr.workflow_name AS title, repo.full_name AS subtitle,
      '/workflow-runs/' || wr.id AS href
    FROM workflow_runs wr JOIN repositories repo ON repo.id = wr.repository_id
    JOIN user_installations ui ON ui.installation_id = repo.installation_id
    WHERE ui.user_id = ? AND (wr.workflow_name LIKE ? ESCAPE '\\' OR repo.full_name LIKE ? ESCAPE '\\')
    ORDER BY wr.github_created_at DESC LIMIT 5
  `).all(user.id, pattern, pattern) as SearchResult[];
  return [...repositories, ...pools, ...runnersFound, ...runs].slice(0, 12);
}

type SearchResult = { kind: string; id: string; title: string; subtitle: string; href: string };

function audit(
  user: AuthenticatedUser,
  action: string,
  targetType: string,
  targetId: string,
  metadata: Record<string, unknown>,
) {
  getSqlite().prepare(`
    INSERT INTO audit_events (
      id, actor_user_id, actor_label, action, target_type,
      target_id, metadata, created_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
  `).run(nanoid(), user.id, user.login, action, targetType, targetId, JSON.stringify(metadata), Date.now());
}
