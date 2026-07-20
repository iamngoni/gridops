import "@tanstack/react-start/server-only";

import type { DashboardOverview } from "./types";
import { getConfigurationState } from "~/server/config.server";
import { getSqlite, migrateDatabase } from "~/server/db/client.server";

type CountRow = { count: number };

export function loadDashboardOverview(): DashboardOverview {
  migrateDatabase();
  const sqlite = getSqlite();

  const runners = sqlite
    .prepare("SELECT COUNT(*) AS count FROM runners WHERE deleted_at IS NULL")
    .get() as CountRow;
  const online = sqlite
    .prepare("SELECT COUNT(*) AS count FROM runners WHERE deleted_at IS NULL AND status IN ('idle', 'busy', 'online')")
    .get() as CountRow;
  const busy = sqlite
    .prepare("SELECT COUNT(*) AS count FROM runners WHERE deleted_at IS NULL AND busy = 1")
    .get() as CountRow;
  const queued = sqlite
    .prepare("SELECT COUNT(*) AS count FROM workflow_jobs WHERE status = 'queued'")
    .get() as CountRow;
  const installations = sqlite
    .prepare("SELECT COUNT(*) AS count FROM installations WHERE suspended_at IS NULL")
    .get() as CountRow;
  const completedRuns = sqlite
    .prepare("SELECT COUNT(*) AS count FROM workflow_runs WHERE completed_at IS NOT NULL")
    .get() as CountRow;
  const successfulRuns = sqlite
    .prepare("SELECT COUNT(*) AS count FROM workflow_runs WHERE conclusion = 'success'")
    .get() as CountRow;

  const pools = sqlite
    .prepare(`
      SELECT
        p.id,
        p.name,
        p.scope,
        p.desired_count AS desired,
        p.mode,
        p.state,
        p.paused,
        SUM(CASE WHEN r.deleted_at IS NULL AND r.status IN ('idle', 'busy', 'online') THEN 1 ELSE 0 END) AS online,
        SUM(CASE WHEN r.deleted_at IS NULL AND r.busy = 1 THEN 1 ELSE 0 END) AS busy
      FROM runner_pools p
      LEFT JOIN runners r ON r.pool_id = p.id
      GROUP BY p.id
      ORDER BY p.created_at DESC
      LIMIT 8
    `)
    .all() as Array<{
      id: string;
      name: string;
      scope: string;
      desired: number;
      mode: string;
      state: string;
      paused: number;
      online: number;
      busy: number;
    }>;

  const runs = sqlite
    .prepare(`
      SELECT
        wr.id,
        repo.full_name AS repository,
        wr.workflow_name AS workflow,
        wr.head_branch AS branch,
        wr.status,
        wr.conclusion,
        wr.started_at AS startedAt,
        wr.completed_at AS completedAt,
        wr.html_url AS htmlUrl
      FROM workflow_runs wr
      JOIN repositories repo ON repo.id = wr.repository_id
      ORDER BY wr.github_created_at DESC
      LIMIT 6
    `)
    .all() as Array<{
      id: number;
      repository: string;
      workflow: string;
      branch: string | null;
      status: string;
      conclusion: string | null;
      startedAt: number | null;
      completedAt: number | null;
      htmlUrl: string;
    }>;

  const activity = sqlite
    .prepare(`
      SELECT id, level, event, message, created_at AS createdAt
      FROM runner_events
      ORDER BY created_at DESC
      LIMIT 8
    `)
    .all() as Array<{
      id: string;
      level: string;
      event: string;
      message: string;
      createdAt: number;
    }>;

  return {
    configuration: getConfigurationState(),
    metrics: {
      runners: runners.count,
      online: online.count,
      busy: busy.count,
      queuedJobs: queued.count,
      successRate:
        completedRuns.count > 0
          ? Math.round((successfulRuns.count / completedRuns.count) * 1000) / 10
          : null,
    },
    pools: pools.map((pool) => ({
      id: pool.id,
      name: pool.name,
      scope: pool.scope,
      desired: pool.desired,
      online: pool.online,
      busy: pool.busy,
      queue: 0,
      mode: pool.mode,
      status: pool.paused ? "paused" : pool.state,
    })),
    runs: runs.map((run) => ({
      ...run,
      startedAt: run.startedAt ? new Date(run.startedAt).toISOString() : null,
      completedAt: run.completedAt ? new Date(run.completedAt).toISOString() : null,
    })),
    activity: activity.map((item) => ({
      ...item,
      createdAt: new Date(item.createdAt).toISOString(),
    })),
    installations: installations.count,
  };
}
