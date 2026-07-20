import "@tanstack/react-start/server-only";

import { createHmac, timingSafeEqual } from "node:crypto";

import { nanoid } from "nanoid";

import { getConfig } from "../config.server";
import { getSqlite, migrateDatabase } from "../db/client.server";

const MAX_WEBHOOK_BYTES = 25 * 1024 * 1024;

type JsonObject = Record<string, unknown>;

function object(value: unknown): JsonObject | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonObject)
    : null;
}

function string(value: unknown) {
  return typeof value === "string" ? value : null;
}

function number(value: unknown) {
  return typeof value === "number" ? value : null;
}

function date(value: unknown) {
  const parsed = typeof value === "string" ? Date.parse(value) : Number.NaN;
  return Number.isNaN(parsed) ? null : parsed;
}

function verifyWebhookSignature(body: string, signature: string | null) {
  const secret = getConfig().githubWebhookSecret;
  if (!secret || !signature?.startsWith("sha256=")) return false;

  const expected = Buffer.from(
    `sha256=${createHmac("sha256", secret).update(body).digest("hex")}`,
  );
  const supplied = Buffer.from(signature);
  return expected.length === supplied.length && timingSafeEqual(expected, supplied);
}

export async function receiveGitHubWebhook(request: Request) {
  const contentLength = Number(request.headers.get("content-length") ?? 0);
  if (contentLength > MAX_WEBHOOK_BYTES) {
    return Response.json({ error: "Webhook payload exceeds 25 MB." }, { status: 413 });
  }

  const body = await request.text();
  if (Buffer.byteLength(body) > MAX_WEBHOOK_BYTES) {
    return Response.json({ error: "Webhook payload exceeds 25 MB." }, { status: 413 });
  }

  const deliveryId = request.headers.get("x-github-delivery");
  const event = request.headers.get("x-github-event");
  const signatureValid = verifyWebhookSignature(
    body,
    request.headers.get("x-hub-signature-256"),
  );

  if (!deliveryId || !event) {
    return Response.json({ error: "Missing GitHub delivery headers." }, { status: 400 });
  }

  let payload: JsonObject;
  try {
    payload = object(JSON.parse(body)) ?? {};
  } catch {
    return Response.json({ error: "Webhook payload is not valid JSON." }, { status: 400 });
  }

  migrateDatabase();
  const sqlite = getSqlite();
  const action = string(payload.action);
  const installationId = number(object(payload.installation)?.id);
  const repositoryId = number(object(payload.repository)?.id);
  const receivedAt = Date.now();

  const inserted = sqlite
    .prepare(`
      INSERT INTO webhook_deliveries (
        id, event, action, hook_id, installation_id, repository_id,
        signature_valid, status, payload, received_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, 'received', ?, ?)
      ON CONFLICT(id) DO NOTHING
    `)
    .run(
      deliveryId,
      event,
      action,
      number(object(payload.hook)?.id),
      installationId,
      repositoryId,
      signatureValid ? 1 : 0,
      JSON.stringify(payload),
      receivedAt,
    );

  if (inserted.changes === 0) {
    return Response.json({ accepted: true, duplicate: true }, { status: 202 });
  }

  if (!signatureValid) {
    sqlite
      .prepare("UPDATE webhook_deliveries SET status = 'rejected', error = ? WHERE id = ?")
      .run("Invalid or unavailable webhook signature.", deliveryId);
    return Response.json({ error: "Invalid webhook signature." }, { status: 401 });
  }

  try {
    processWebhook(event, action, payload);
    sqlite
      .prepare("UPDATE webhook_deliveries SET status = 'processed', processed_at = ? WHERE id = ?")
      .run(Date.now(), deliveryId);
    return Response.json({ accepted: true }, { status: 202 });
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown webhook processing error";
    sqlite
      .prepare("UPDATE webhook_deliveries SET status = 'failed', error = ?, processed_at = ? WHERE id = ?")
      .run(message.slice(0, 2_000), Date.now(), deliveryId);
    return Response.json({ accepted: true, processing: "failed" }, { status: 202 });
  }
}

function processWebhook(event: string, action: string | null, payload: JsonObject) {
  switch (event) {
    case "ping":
      return;
    case "installation":
      processInstallation(action, payload);
      return;
    case "installation_repositories":
      processInstallationRepositories(payload);
      return;
    case "workflow_run":
      processWorkflowRun(payload);
      return;
    case "workflow_job":
      processWorkflowJob(action, payload);
      return;
    case "github_app_authorization":
      if (action === "revoked") processAuthorizationRevoked(payload);
      return;
    default:
      return;
  }
}

function processInstallation(action: string | null, payload: JsonObject) {
  const sqlite = getSqlite();
  const installation = object(payload.installation);
  const account = object(installation?.account);
  const id = number(installation?.id);
  if (!installation || !account || !id) return;

  if (action === "deleted") {
    sqlite.prepare("DELETE FROM installations WHERE id = ?").run(id);
    return;
  }

  const now = Date.now();
  sqlite
    .prepare(`
      INSERT INTO installations (
        id, account_id, account_login, account_type, account_avatar_url,
        target_type, repository_selection, permissions, events,
        suspended_at, last_synced_at, created_at, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(id) DO UPDATE SET
        account_login = excluded.account_login,
        account_type = excluded.account_type,
        account_avatar_url = excluded.account_avatar_url,
        repository_selection = excluded.repository_selection,
        permissions = excluded.permissions,
        events = excluded.events,
        suspended_at = excluded.suspended_at,
        last_synced_at = excluded.last_synced_at,
        updated_at = excluded.updated_at
    `)
    .run(
      id,
      number(account.id),
      string(account.login),
      string(account.type) ?? "User",
      string(account.avatar_url),
      string(installation.target_type) ?? "User",
      string(installation.repository_selection) ?? "selected",
      JSON.stringify(object(installation.permissions) ?? {}),
      JSON.stringify(Array.isArray(installation.events) ? installation.events : []),
      date(installation.suspended_at),
      now,
      now,
      now,
    );
}

function processInstallationRepositories(payload: JsonObject) {
  const installationId = number(object(payload.installation)?.id);
  if (!installationId) return;

  const added = Array.isArray(payload.repositories_added) ? payload.repositories_added : [];
  const removed = Array.isArray(payload.repositories_removed) ? payload.repositories_removed : [];
  const sqlite = getSqlite();

  for (const repositoryValue of added) {
    const repository = object(repositoryValue);
    const id = number(repository?.id);
    const fullName = string(repository?.full_name);
    if (!repository || !id || !fullName) continue;
    const [owner, name] = fullName.split("/");
    const now = Date.now();

    sqlite
      .prepare(`
        INSERT INTO repositories (
          id, installation_id, owner, name, full_name, private, archived,
          default_branch, html_url, last_synced_at, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
          installation_id = excluded.installation_id,
          full_name = excluded.full_name,
          private = excluded.private,
          last_synced_at = excluded.last_synced_at,
          updated_at = excluded.updated_at
      `)
      .run(
        id,
        installationId,
        owner ?? "unknown",
        name ?? fullName,
        fullName,
        repository.private ? 1 : 0,
        string(repository.default_branch) ?? "master",
        string(repository.html_url) ?? `https://github.com/${fullName}`,
        now,
        now,
        now,
      );
  }

  for (const repositoryValue of removed) {
    const id = number(object(repositoryValue)?.id);
    if (id) sqlite.prepare("DELETE FROM repositories WHERE id = ?").run(id);
  }
}

function processWorkflowRun(payload: JsonObject) {
  const workflow = object(payload.workflow_run);
  const repository = object(payload.repository);
  const id = number(workflow?.id);
  const repositoryId = number(repository?.id);
  if (!workflow || !id || !repositoryId) return;

  const repositoryExists = getSqlite()
    .prepare("SELECT 1 FROM repositories WHERE id = ?")
    .get(repositoryId);
  if (!repositoryExists) throw new Error(`Repository ${repositoryId} is not synced yet.`);

  const now = Date.now();
  getSqlite()
    .prepare(`
      INSERT INTO workflow_runs (
        id, repository_id, workflow_id, workflow_name, run_number, run_attempt,
        event, status, conclusion, head_branch, head_sha, actor_login, html_url,
        started_at, completed_at, github_created_at, github_updated_at, created_at, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(id) DO UPDATE SET
        workflow_name = excluded.workflow_name,
        run_attempt = excluded.run_attempt,
        status = excluded.status,
        conclusion = excluded.conclusion,
        started_at = excluded.started_at,
        completed_at = excluded.completed_at,
        github_updated_at = excluded.github_updated_at,
        updated_at = excluded.updated_at
    `)
    .run(
      id,
      repositoryId,
      number(workflow.workflow_id),
      string(workflow.name) ?? string(workflow.display_title) ?? "Workflow",
      number(workflow.run_number) ?? 0,
      number(workflow.run_attempt) ?? 1,
      string(workflow.event) ?? "unknown",
      string(workflow.status) ?? "queued",
      string(workflow.conclusion),
      string(workflow.head_branch),
      string(workflow.head_sha) ?? "unknown",
      string(object(workflow.actor)?.login),
      string(workflow.html_url) ?? "",
      date(workflow.run_started_at),
      date(workflow.updated_at) && string(workflow.status) === "completed"
        ? date(workflow.updated_at)
        : null,
      date(workflow.created_at) ?? now,
      date(workflow.updated_at) ?? now,
      now,
      now,
    );
}

function processWorkflowJob(action: string | null, payload: JsonObject) {
  const job = object(payload.workflow_job);
  const id = number(job?.id);
  const runId = number(job?.run_id);
  if (!job || !id || !runId) return;

  const runExists = getSqlite().prepare("SELECT 1 FROM workflow_runs WHERE id = ?").get(runId);
  if (!runExists) throw new Error(`Workflow run ${runId} is not synced yet.`);

  const now = Date.now();
  getSqlite()
    .prepare(`
      INSERT INTO workflow_jobs (
        id, run_id, name, status, conclusion, runner_id, runner_name,
        runner_group_id, runner_group_name, labels, html_url,
        started_at, completed_at, created_at, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(id) DO UPDATE SET
        status = excluded.status,
        conclusion = excluded.conclusion,
        runner_id = excluded.runner_id,
        runner_name = excluded.runner_name,
        runner_group_id = excluded.runner_group_id,
        runner_group_name = excluded.runner_group_name,
        labels = excluded.labels,
        started_at = excluded.started_at,
        completed_at = excluded.completed_at,
        updated_at = excluded.updated_at
    `)
    .run(
      id,
      runId,
      string(job.name) ?? "Job",
      string(job.status) ?? action ?? "queued",
      string(job.conclusion),
      number(job.runner_id),
      string(job.runner_name),
      number(job.runner_group_id),
      string(job.runner_group_name),
      JSON.stringify(Array.isArray(job.labels) ? job.labels : []),
      string(job.html_url) ?? "",
      date(job.started_at),
      date(job.completed_at),
      now,
      now,
    );

  getSqlite()
    .prepare(`
      INSERT INTO runner_events (id, level, event, message, metadata, created_at)
      VALUES (?, 'info', ?, ?, ?, ?)
    `)
    .run(
      nanoid(),
      `Workflow job ${action ?? string(job.status) ?? "updated"}`,
      `${string(job.name) ?? "Job"} · run ${runId}`,
      JSON.stringify({ jobId: id, runId, labels: job.labels ?? [] }),
      now,
    );

  const status = string(job.status) ?? action ?? "queued";
  const runnerName = string(job.runner_name);
  if (runnerName) {
    if (status === "in_progress") {
      getSqlite().prepare(`
        UPDATE runners SET busy = 1, status = 'busy', current_job_id = ?,
          last_heartbeat_at = ?, updated_at = ?
        WHERE name = ? AND deleted_at IS NULL
      `).run(id, now, now, runnerName);
    } else if (status === "completed") {
      getSqlite().prepare(`
        UPDATE runners SET busy = 0, status = 'online', current_job_id = NULL,
          last_heartbeat_at = ?, updated_at = ?
        WHERE name = ? AND deleted_at IS NULL
      `).run(now, now, runnerName);
    }
  }

  if (status === "queued") scaleForQueuedJob(payload, job, now);
}

function scaleForQueuedJob(payload: JsonObject, job: JsonObject, now: number) {
  const repositoryId = number(object(payload.repository)?.id);
  if (!repositoryId) return;
  const requestedLabels = (Array.isArray(job.labels) ? job.labels : [])
    .filter((label): label is string => typeof label === "string");
  const defaultLabels = new Set(["self-hosted", "linux", "windows", "macos", "x64", "x86", "arm", "arm64"]);
  const customLabels = requestedLabels.filter((label) => !defaultLabels.has(label.toLowerCase()));

  const candidates = getSqlite().prepare(`
    SELECT p.id, p.labels, p.desired_count AS desiredCount,
      p.max_count AS maxCount, p.queue_scale_factor AS queueScaleFactor,
      p.scope, p.repository_id AS repositoryId,
      COUNT(CASE WHEN r.deleted_at IS NULL AND r.status IN ('starting','online','idle','busy','paused') THEN 1 END) AS activeCount
    FROM runner_pools p
    JOIN repositories event_repo ON event_repo.id = ?
    LEFT JOIN runners r ON r.pool_id = p.id
    WHERE p.autoscaling_enabled = 1 AND p.paused = 0
      AND (
        p.repository_id = event_repo.id OR
        (p.scope = 'organization' AND p.installation_id = event_repo.installation_id)
      )
    GROUP BY p.id
    ORDER BY CASE WHEN p.repository_id = event_repo.id THEN 0 ELSE 1 END, p.created_at ASC
  `).all(repositoryId) as Array<{
    id: string;
    labels: string;
    desiredCount: number;
    maxCount: number;
    queueScaleFactor: number;
    scope: string;
    repositoryId: number | null;
    activeCount: number;
  }>;

  const pool = candidates.find((candidate) => {
    let labels: string[] = [];
    try { labels = JSON.parse(candidate.labels) as string[]; } catch { return false; }
    const available = new Set(labels.map((label) => label.toLowerCase()));
    return customLabels.every((label) => available.has(label.toLowerCase()));
  });
  if (!pool) return;

  const target = Math.min(
    pool.maxCount,
    Math.max(pool.desiredCount, pool.activeCount + pool.queueScaleFactor),
  );
  if (target <= pool.desiredCount) return;

  getSqlite().prepare(`
    UPDATE runner_pools SET desired_count = ?, state = 'scaling', updated_at = ? WHERE id = ?
  `).run(target, now, pool.id);
  getSqlite().prepare(`
    INSERT INTO runner_events (id, pool_id, level, event, message, metadata, created_at)
    VALUES (?, ?, 'info', 'Autoscale requested', ?, ?, ?)
  `).run(
    nanoid(),
    pool.id,
    `Queued job raised desired capacity from ${pool.desiredCount} to ${target}`,
    JSON.stringify({ jobId: number(job.id), labels: requestedLabels, desiredCount: target }),
    now,
  );
  getSqlite().prepare(`
    INSERT INTO audit_events (id, actor_label, action, target_type, target_id, metadata, created_at)
    VALUES (?, 'system', 'runner_pool.autoscaled', 'runner_pool', ?, ?, ?)
  `).run(nanoid(), pool.id, JSON.stringify({ jobId: number(job.id), desiredCount: target }), now);
}

function processAuthorizationRevoked(payload: JsonObject) {
  const senderId = number(object(payload.sender)?.id);
  if (!senderId) return;

  getSqlite()
    .prepare("DELETE FROM users WHERE github_id = ?")
    .run(senderId);
}
