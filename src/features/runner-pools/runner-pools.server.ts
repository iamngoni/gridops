import "@tanstack/react-start/server-only";

import { and, eq } from "drizzle-orm";
import { nanoid } from "nanoid";

import type { CreateRunnerPoolInput } from "./schemas";
import { getConfig } from "~/server/config.server";
import { getDb, getSqlite, migrateDatabase } from "~/server/db/client.server";
import {
  auditEvents,
  installations,
  repositories,
  runnerEvents,
  runnerPools,
  runners,
  userInstallations,
} from "~/server/db/schema";
import { githubRequest } from "~/server/github/api.server";
import { getValidGitHubAccessToken } from "~/server/github/token.server";
import { runnerManagerRequest } from "~/server/runner-manager/client.server";

type AuthenticatedUser = { id: string; login: string };

export function getRunnerPoolFormOptions(user: AuthenticatedUser) {
  migrateDatabase();
  const allowedInstallations = getDb()
    .select({
      id: installations.id,
      accountLogin: installations.accountLogin,
      accountType: installations.accountType,
    })
    .from(userInstallations)
    .innerJoin(installations, eq(installations.id, userInstallations.installationId))
    .where(eq(userInstallations.userId, user.id))
    .all();

  const installationIds = new Set(allowedInstallations.map((item) => item.id));
  const allowedRepositories = getDb()
    .select({
      id: repositories.id,
      installationId: repositories.installationId,
      fullName: repositories.fullName,
      private: repositories.private,
    })
    .from(repositories)
    .all()
    .filter((repository) => installationIds.has(repository.installationId));

  return {
    installations: allowedInstallations,
    repositories: allowedRepositories,
    defaults: {
      image: getConfig().runnerImage,
      labels: ["gridops"],
      cpuLimit: 2,
      memoryLimitMb: 4096,
      desiredCount: 0,
      minCount: 0,
      maxCount: 10,
      autoscalingEnabled: true,
      queueScaleFactor: 1,
      idleTimeoutMinutes: 5,
      runnerGroupId: 1,
    },
  };
}

export async function createRunnerPool(user: AuthenticatedUser, input: CreateRunnerPoolInput) {
  migrateDatabase();
  const access = getDb()
    .select({ installationId: userInstallations.installationId })
    .from(userInstallations)
    .where(
      and(
        eq(userInstallations.userId, user.id),
        eq(userInstallations.installationId, input.installationId),
      ),
    )
    .get();
  if (!access) throw new Error("You do not have access to this GitHub installation.");

  if (input.repositoryId) {
    const repository = getDb()
      .select({ installationId: repositories.installationId })
      .from(repositories)
      .where(eq(repositories.id, input.repositoryId))
      .get();
    if (!repository || repository.installationId !== input.installationId) {
      throw new Error("The selected repository is not part of this installation.");
    }
  }

  const poolId = nanoid();
  getDb()
    .insert(runnerPools)
    .values({
      id: poolId,
      installationId: input.installationId,
      repositoryId: input.repositoryId,
      name: input.name,
      scope: input.scope,
      mode: input.mode,
      labels: Array.from(new Set([input.name, ...input.labels])),
      image: input.image,
      desiredCount: input.desiredCount,
      minCount: input.minCount,
      maxCount: input.maxCount,
      autoscalingEnabled: input.autoscalingEnabled,
      queueScaleFactor: input.queueScaleFactor,
      idleTimeoutMinutes: input.idleTimeoutMinutes,
      cpuLimit: input.cpuLimit,
      memoryLimitMb: input.memoryLimitMb,
      runnerGroupId: input.runnerGroupId,
      ephemeral: input.mode === "ephemeral",
      createdBy: user.id,
    })
    .run();

  audit(user, "runner_pool.created", "runner_pool", poolId, {
    name: input.name,
    scope: input.scope,
    desiredCount: input.desiredCount,
  });

  const provisioned: Array<{ runnerId: string; status: string; error?: string }> = [];
  for (let index = 0; index < input.desiredCount; index += 1) {
    try {
      const runner = await provisionRunner(user, poolId);
      provisioned.push({ runnerId: runner.id, status: runner.status });
    } catch (error) {
      provisioned.push({
        runnerId: "unavailable",
        status: "failed",
        error: error instanceof Error ? error.message : "Unknown provisioning error",
      });
    }
  }

  return { id: poolId, provisioned };
}

export async function provisionRunner(user: AuthenticatedUser, poolId: string) {
  migrateDatabase();
  const pool = getDb()
    .select({
      pool: runnerPools,
      installation: installations,
      repository: repositories,
    })
    .from(runnerPools)
    .innerJoin(installations, eq(installations.id, runnerPools.installationId))
    .leftJoin(repositories, eq(repositories.id, runnerPools.repositoryId))
    .where(eq(runnerPools.id, poolId))
    .get();
  if (!pool) throw new Error("Runner pool does not exist.");
  if (pool.pool.paused) throw new Error("Runner pool is paused.");

  const access = getDb()
    .select()
    .from(userInstallations)
    .where(
      and(
        eq(userInstallations.userId, user.id),
        eq(userInstallations.installationId, pool.pool.installationId),
      ),
    )
    .get();
  if (!access) throw new Error("You do not have access to this runner pool.");

  const runnerId = nanoid();
  const runnerName = `${pool.pool.name}-${nanoid(8).toLowerCase()}`;
  getDb()
    .insert(runners)
    .values({
      id: runnerId,
      poolId,
      name: runnerName,
      status: "starting",
      ephemeral: pool.pool.ephemeral,
    })
    .run();

  try {
    const accessToken = await getValidGitHubAccessToken(user.id);
    const endpoint = pool.repository
      ? `/repos/${pool.repository.owner}/${pool.repository.name}/actions/runners/generate-jitconfig`
      : `/orgs/${pool.installation.accountLogin}/actions/runners/generate-jitconfig`;
    const jit = await githubRequest<{
      runner: { id: number; name: string; status: string; busy: boolean };
      encoded_jit_config: string;
    }>(endpoint, accessToken, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        name: runnerName,
        runner_group_id: pool.pool.runnerGroupId,
        labels: pool.pool.labels,
        work_folder: "_work",
      }),
    });

    const manager = await runnerManagerRequest<{
      id: string;
      name: string;
      state: string;
      createdAt: string;
    }>("/v1/runners", {
      method: "POST",
      body: JSON.stringify({
        runnerId,
        poolId,
        name: runnerName,
        image: pool.pool.image,
        jitConfig: jit.encoded_jit_config,
        cpuLimit: pool.pool.cpuLimit,
        memoryLimitMb: pool.pool.memoryLimitMb,
        network: getConfig().runnerNetwork,
      }),
    });

    getDb()
      .update(runners)
      .set({
        githubRunnerId: jit.runner.id,
        containerId: manager.id,
        containerName: manager.name,
        status: manager.state === "running" ? "online" : manager.state,
        registeredAt: new Date(),
        lastHeartbeatAt: new Date(),
      })
      .where(eq(runners.id, runnerId))
      .run();

    getDb()
      .insert(runnerEvents)
      .values({
        id: nanoid(),
        runnerId,
        poolId,
        event: "Runner started",
        message: `${runnerName} started in pool ${pool.pool.name}`,
        metadata: { containerId: manager.id, githubRunnerId: jit.runner.id },
      })
      .run();

    audit(user, "runner.provisioned", "runner", runnerId, {
      poolId,
      containerId: manager.id,
      githubRunnerId: jit.runner.id,
    });

    return { id: runnerId, status: "online" };
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown provisioning error";
    getDb()
      .update(runners)
      .set({ status: "failed", failureReason: message })
      .where(eq(runners.id, runnerId))
      .run();
    getDb()
      .insert(runnerEvents)
      .values({
        id: nanoid(),
        runnerId,
        poolId,
        level: "error",
        event: "Runner provisioning failed",
        message,
      })
      .run();
    throw error;
  }
}

type PoolAccessRow = {
  id: string;
  installationId: number;
  repositoryId: number | null;
  accountLogin: string;
  repositoryOwner: string | null;
  repositoryName: string | null;
  desiredCount: number;
  minCount: number;
  maxCount: number;
  paused: number;
};

type RunnerAccessRow = PoolAccessRow & {
  runnerId: string;
  runnerName: string;
  containerId: string | null;
  githubRunnerId: number | null;
  runnerStatus: string;
  busy: number;
};

function poolAccess(user: AuthenticatedUser, poolId: string) {
  migrateDatabase();
  const pool = getSqlite().prepare(`
    SELECT p.id, p.installation_id AS installationId,
      p.repository_id AS repositoryId, i.account_login AS accountLogin,
      repo.owner AS repositoryOwner, repo.name AS repositoryName,
      p.desired_count AS desiredCount, p.min_count AS minCount,
      p.max_count AS maxCount, p.paused
    FROM runner_pools p
    JOIN user_installations ui ON ui.installation_id = p.installation_id
    JOIN installations i ON i.id = p.installation_id
    LEFT JOIN repositories repo ON repo.id = p.repository_id
    WHERE p.id = ? AND ui.user_id = ?
  `).get(poolId, user.id) as PoolAccessRow | undefined;
  if (!pool) throw new Error("Runner pool does not exist or is not accessible.");
  return pool;
}

function runnerAccess(user: AuthenticatedUser, runnerId: string) {
  migrateDatabase();
  const runner = getSqlite().prepare(`
    SELECT p.id, p.installation_id AS installationId,
      p.repository_id AS repositoryId, i.account_login AS accountLogin,
      repo.owner AS repositoryOwner, repo.name AS repositoryName,
      p.desired_count AS desiredCount, p.min_count AS minCount,
      p.max_count AS maxCount, p.paused,
      r.id AS runnerId, r.name AS runnerName, r.container_id AS containerId,
      r.github_runner_id AS githubRunnerId, r.status AS runnerStatus, r.busy
    FROM runners r
    JOIN runner_pools p ON p.id = r.pool_id
    JOIN user_installations ui ON ui.installation_id = p.installation_id
    JOIN installations i ON i.id = p.installation_id
    LEFT JOIN repositories repo ON repo.id = p.repository_id
    WHERE r.id = ? AND ui.user_id = ? AND r.deleted_at IS NULL
  `).get(runnerId, user.id) as RunnerAccessRow | undefined;
  if (!runner) throw new Error("Runner does not exist or is not accessible.");
  return runner;
}

function runnersForPool(user: AuthenticatedUser, poolId: string) {
  poolAccess(user, poolId);
  return getSqlite().prepare(`
    SELECT p.id, p.installation_id AS installationId,
      p.repository_id AS repositoryId, i.account_login AS accountLogin,
      repo.owner AS repositoryOwner, repo.name AS repositoryName,
      p.desired_count AS desiredCount, p.min_count AS minCount,
      p.max_count AS maxCount, p.paused,
      r.id AS runnerId, r.name AS runnerName, r.container_id AS containerId,
      r.github_runner_id AS githubRunnerId, r.status AS runnerStatus, r.busy
    FROM runners r
    JOIN runner_pools p ON p.id = r.pool_id
    JOIN installations i ON i.id = p.installation_id
    LEFT JOIN repositories repo ON repo.id = p.repository_id
    WHERE p.id = ? AND r.deleted_at IS NULL
    ORDER BY r.created_at DESC
  `).all(poolId) as RunnerAccessRow[];
}

async function deleteRunnerResources(user: AuthenticatedUser, runner: RunnerAccessRow) {
  if (runner.containerId) {
    await runnerManagerRequest(`/v1/runners/${runner.containerId}`, { method: "DELETE" })
      .catch((error) => {
        if (!(error instanceof Error && error.message.includes("404"))) throw error;
      });
  }

  if (runner.githubRunnerId) {
    const token = await getValidGitHubAccessToken(user.id);
    const path = runner.repositoryOwner && runner.repositoryName
      ? `/repos/${runner.repositoryOwner}/${runner.repositoryName}/actions/runners/${runner.githubRunnerId}`
      : `/orgs/${runner.accountLogin}/actions/runners/${runner.githubRunnerId}`;
    await githubRequest<void>(path, token, { method: "DELETE" }).catch((error) => {
      if (!(error instanceof Error && error.message.includes("404"))) throw error;
    });
  }

  getSqlite().prepare(`
    UPDATE runners SET status = 'deleted', busy = 0, deleted_at = ?, updated_at = ?
    WHERE id = ?
  `).run(Date.now(), Date.now(), runner.runnerId);
  getSqlite().prepare(`
    INSERT INTO runner_events (id, runner_id, pool_id, event, message, metadata, created_at)
    VALUES (?, ?, ?, 'Runner deleted', ?, '{}', ?)
  `).run(nanoid(), runner.runnerId, runner.id, `${runner.runnerName} was removed`, Date.now());
}

export async function setRunnerPoolPaused(
  user: AuthenticatedUser,
  poolId: string,
  paused: boolean,
) {
  poolAccess(user, poolId);
  getSqlite().prepare(`
    UPDATE runner_pools SET paused = ?, state = ?, updated_at = ? WHERE id = ?
  `).run(paused ? 1 : 0, paused ? "draining" : "active", Date.now(), poolId);

  if (paused) {
    const idle = runnersForPool(user, poolId).filter((runner) => !runner.busy);
    for (const runner of idle) await deleteRunnerResources(user, runner);
  } else {
    await reconcileRunnerPool(user, poolId);
  }

  audit(user, paused ? "runner_pool.paused" : "runner_pool.resumed", "runner_pool", poolId, {});
  return { ok: true };
}

export async function scaleRunnerPool(
  user: AuthenticatedUser,
  poolId: string,
  desiredCount: number,
) {
  const pool = poolAccess(user, poolId);
  if (desiredCount < pool.minCount || desiredCount > pool.maxCount) {
    throw new Error(`Desired capacity must be between ${pool.minCount} and ${pool.maxCount}.`);
  }
  getSqlite().prepare(`
    UPDATE runner_pools SET desired_count = ?, updated_at = ? WHERE id = ?
  `).run(desiredCount, Date.now(), poolId);
  const result = await reconcileRunnerPool(user, poolId);
  audit(user, "runner_pool.scaled", "runner_pool", poolId, { desiredCount });
  return result;
}

export async function reconcileRunnerPool(user: AuthenticatedUser, poolId: string) {
  const pool = poolAccess(user, poolId);
  const known = runnersForPool(user, poolId);
  let managerRunners: Array<{
    id: string;
    state: string;
    labels: Record<string, string>;
  }> = [];
  try {
    const response = await runnerManagerRequest<{ runners: typeof managerRunners }>("/v1/runners");
    managerRunners = response.runners;
  } catch (error) {
    if (known.some((runner) => runner.containerId)) throw error;
  }

  const byContainer = new Map(managerRunners.map((container) => [container.id, container]));
  for (const runner of known) {
    if (!runner.containerId) continue;
    const container = byContainer.get(runner.containerId);
    const state = container?.state ?? "missing";
    const status = state === "running" ? (runner.busy ? "busy" : "online")
      : state === "paused" ? "paused"
      : state === "exited" || state === "dead" || state === "missing" ? "stopped"
      : state;
    getSqlite().prepare("UPDATE runners SET status = ?, last_heartbeat_at = ?, updated_at = ? WHERE id = ?")
      .run(status, Date.now(), Date.now(), runner.runnerId);
  }

  if (pool.paused) return { ok: true, desired: pool.desiredCount, active: 0, provisioned: 0, removed: 0 };

  const refreshed = runnersForPool(user, poolId);
  const active = refreshed.filter((runner) => ["starting", "online", "idle", "busy", "paused"].includes(runner.runnerStatus));
  let provisioned = 0;
  let removed = 0;

  if (active.length < pool.desiredCount) {
    for (let count = active.length; count < pool.desiredCount; count += 1) {
      await provisionRunner(user, poolId);
      provisioned += 1;
    }
  } else if (active.length > pool.desiredCount) {
    const removable = active.filter((runner) => !runner.busy).slice(0, active.length - pool.desiredCount);
    for (const runner of removable) {
      await deleteRunnerResources(user, runner);
      removed += 1;
    }
  }

  const finalActive = runnersForPool(user, poolId)
    .filter((runner) => ["starting", "online", "idle", "busy", "paused"].includes(runner.runnerStatus)).length;
  getSqlite().prepare("UPDATE runner_pools SET state = ?, updated_at = ? WHERE id = ?")
    .run(finalActive > pool.desiredCount ? "draining" : "active", Date.now(), poolId);
  return { ok: true, desired: pool.desiredCount, active: finalActive, provisioned, removed };
}

export async function deleteRunnerPool(user: AuthenticatedUser, poolId: string) {
  poolAccess(user, poolId);
  getSqlite().prepare("UPDATE runner_pools SET paused = 1, state = 'deleting', updated_at = ? WHERE id = ?")
    .run(Date.now(), poolId);
  const poolRunners = runnersForPool(user, poolId);
  for (const runner of poolRunners) await deleteRunnerResources(user, runner);
  audit(user, "runner_pool.deleted", "runner_pool", poolId, { runners: poolRunners.length });
  getSqlite().prepare("DELETE FROM runner_pools WHERE id = ?").run(poolId);
  return { ok: true };
}

export async function controlRunner(
  user: AuthenticatedUser,
  runnerId: string,
  action: "stop" | "pause" | "resume" | "restart" | "rebuild" | "delete",
) {
  const runner = runnerAccess(user, runnerId);
  if (action === "delete") {
    await deleteRunnerResources(user, runner);
  } else if (action === "rebuild") {
    await deleteRunnerResources(user, runner);
    await provisionRunner(user, runner.id);
  } else {
    if (!runner.containerId) throw new Error("Runner has no managed container.");
    await runnerManagerRequest(`/v1/runners/${runner.containerId}/${action}`, { method: "POST" });
    const status = action === "resume" || action === "restart" ? "online" : action === "stop" ? "stopped" : "paused";
    getSqlite().prepare("UPDATE runners SET status = ?, updated_at = ? WHERE id = ?")
      .run(status, Date.now(), runnerId);
  }
  audit(user, `runner.${action}`, "runner", runnerId, { poolId: runner.id });
  return { ok: true };
}

function audit(
  user: AuthenticatedUser,
  action: string,
  targetType: string,
  targetId: string,
  metadata: Record<string, unknown>,
) {
  getDb()
    .insert(auditEvents)
    .values({
      id: nanoid(),
      actorUserId: user.id,
      actorLabel: user.login,
      action,
      targetType,
      targetId,
      metadata,
    })
    .run();
}
