import { createServerFn } from "@tanstack/react-start";
import { getRequest, setResponseHeaders } from "@tanstack/react-start/server";
import { z } from "zod";

import { getSessionUser } from "~/server/auth/session.server";
import {
  controlWorkflowRun,
  getRunnerLogs,
  getWorkflowRunDetail,
  getSettingsOverview,
  listAuditEvents,
  listLogTargets,
  listRepositories,
  listRunnerPools,
  listRunners,
  listWebhookDeliveries,
  listWorkflowRuns,
  saveSystemSettings,
  searchControlPlane,
  syncGitHubData,
} from "./operations.server";

function privateResponse() {
  setResponseHeaders(new Headers({ "Cache-Control": "private, no-store", Vary: "Cookie" }));
}

function userOrNull() {
  privateResponse();
  const user = getSessionUser(getRequest());
  return user ? { id: user.id, login: user.login } : null;
}

function requireUser() {
  const user = userOrNull();
  if (!user) throw new Error("Connect GitHub to use this control.");
  return user;
}

function page<T>(load: (user: { id: string; login: string }) => T) {
  const user = userOrNull();
  return user
    ? { authenticated: true as const, items: load(user) }
    : { authenticated: false as const, items: [] as never[] };
}

export const getRunnerPoolsPage = createServerFn({ method: "GET" }).handler(() => page(listRunnerPools));
export const getRunnersPage = createServerFn({ method: "GET" }).handler(() => page(listRunners));
export const getRepositoriesPage = createServerFn({ method: "GET" }).handler(() => page(listRepositories));
export const getWorkflowRunsPage = createServerFn({ method: "GET" }).handler(() => page(listWorkflowRuns));
export const getWorkflowRunDetailAction = createServerFn({ method: "GET" })
  .validator(z.object({ runId: z.number().int().positive() }))
  .handler(({ data }) => getWorkflowRunDetail(requireUser(), data.runId));
export const getWebhooksPage = createServerFn({ method: "GET" }).handler(() => page(listWebhookDeliveries));
export const getAuditLogPage = createServerFn({ method: "GET" }).handler(() => page(listAuditEvents));
export const getLiveLogsPage = createServerFn({ method: "GET" }).handler(() => page(listLogTargets));

export const getSettingsPage = createServerFn({ method: "GET" }).handler(async () => {
  const user = userOrNull();
  return user
    ? { authenticated: true as const, data: await getSettingsOverview(user) }
    : { authenticated: false as const, data: null };
});

export const syncGitHubAction = createServerFn({ method: "POST" }).handler(async () => {
  return syncGitHubData(requireUser());
});

export const workflowRunAction = createServerFn({ method: "POST" })
  .validator(z.object({
    runId: z.number().int().positive(),
    action: z.enum(["cancel", "rerun", "rerun-failed"]),
  }))
  .handler(({ data }) => controlWorkflowRun(requireUser(), data));

export const runnerLogsAction = createServerFn({ method: "POST" })
  .validator(z.object({ runnerId: z.string().min(1) }))
  .handler(({ data }) => getRunnerLogs(requireUser(), data.runnerId));

export const searchAction = createServerFn({ method: "POST" })
  .validator(z.object({ query: z.string().trim().min(2).max(100) }))
  .handler(({ data }) => searchControlPlane(requireUser(), data.query));

export const saveSettingsAction = createServerFn({ method: "POST" })
  .validator(z.object({
    logRetentionDays: z.number().int().min(1).max(3650),
    webhookRetentionDays: z.number().int().min(1).max(3650),
    auditRetentionDays: z.number().int().min(1).max(3650),
    reconcileIntervalSeconds: z.number().int().min(5).max(3600),
    autoUpdateImages: z.boolean(),
  }))
  .handler(({ data }) => saveSystemSettings(requireUser(), data));
