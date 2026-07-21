import { api } from "~/lib/api";

type Page<T> = { authenticated: boolean; items: T[] };

export type Repository = {
  id: number; fullName: string; private: boolean; archived: boolean; defaultBranch: string;
  htmlUrl: string; permission: string | null; lastSyncedAt: string; installationId: number;
  accountLogin: string; accountType: string; repositorySelection: string; poolCount: number;
  runCount: number; lastRunAt: string | null;
};

export type RunnerPool = {
  id: string; name: string; scope: string; mode: string; labels: string[]; image: string;
  desiredCount: number; minCount: number; maxCount: number; cpuLimit: number; memoryLimitMb: number;
  paused: boolean; state: string; accountLogin: string; repository: string | null;
  totalRunners: number; onlineRunners: number; busyRunners: number; failedRunners: number; createdAt: string;
};

export type Runner = {
  id: string; name: string; status: string; busy: boolean; ephemeral: boolean; os: string;
  architecture: string; containerId: string | null; githubRunnerId: number | null;
  failureReason: string | null; registeredAt: string | null; lastHeartbeatAt: string | null;
  createdAt: string; poolId: string; poolName: string; poolPaused: boolean; accountLogin: string;
  repository: string | null; currentJobName: string | null; currentRunId: number | null;
};

export type WorkflowRun = {
  id: number; workflowName: string; runNumber: number; runAttempt: number; event: string;
  status: string; conclusion: string | null; headBranch: string | null; headSha: string;
  actorLogin: string | null; htmlUrl: string; startedAt: string | null; completedAt: string | null;
  createdAt: string; repository: string; jobCount: number; activeJobs: number; failedJobs: number;
};

export type WorkflowRunDetail = Omit<WorkflowRun, "jobCount" | "activeJobs" | "failedJobs"> & {
  jobs: Array<{ id: number; name: string; status: string; conclusion: string | null; runnerName: string | null;
    runnerGroupName: string | null; labels: string[]; htmlUrl: string; startedAt: string | null; completedAt: string | null }>;
};

export type WebhookDelivery = {
  id: string; event: string; action: string | null; installationId: number | null;
  repositoryId: number | null; signatureValid: boolean; status: string; error: string | null;
  receivedAt: string; processedAt: string | null; accountLogin: string | null; repository: string | null;
};

export type AuditEvent = {
  id: string; actorLabel: string; action: string; targetType: string; targetId: string | null;
  metadata: string; ipAddress: string | null; createdAt: string;
};

export type LogTarget = {
  id: string; runnerId?: string | null; name: string; status: string; busy: boolean;
  containerId: string | null; updatedAt: string; poolName: string; repository: string | null;
  kind: "live" | "archive"; sizeBytes?: number;
};

export type SettingsPage = {
  authenticated: boolean;
  data: null | {
    configuration: {
      githubOAuth: boolean; githubAppControl: boolean; webhookVerification: boolean;
      secureStorage: boolean; runnerManager: boolean; installationTokens: boolean;
      callbackUrl: string; webhookUrl: string;
    };
    manager: { ok: boolean; dockerVersion?: string; apiVersion?: string; error?: string };
    settings: { logRetentionDays: number; webhookRetentionDays: number; auditRetentionDays: number;
      reconcileIntervalSeconds: number; autoUpdateImages: boolean };
    user: { login: string };
  };
};

export const getRunnerPoolsPage = () => api<Page<RunnerPool>>("/api/v1/runner-pools");
export const getRunnersPage = () => api<Page<Runner>>("/api/v1/runners");
export const getRepositoriesPage = () => api<Page<Repository>>("/api/v1/repositories");
export const getWorkflowRunsPage = () => api<Page<WorkflowRun>>("/api/v1/workflow-runs");
export const getWebhooksPage = () => api<Page<WebhookDelivery>>("/api/v1/webhooks");
export const getAuditLogPage = () => api<Page<AuditEvent>>("/api/v1/audit");
export const getLiveLogsPage = () => api<Page<LogTarget>>("/api/v1/log-streams");
export const getSettingsPage = () => api<SettingsPage>("/api/v1/settings");

export const getWorkflowRunDetailAction = ({ data }: { data: { runId: number } }) =>
  api<WorkflowRunDetail>(`/api/v1/workflow-runs/${data.runId}`);
export const syncGitHubAction = () => api<{ repositories: number }>("/api/v1/repositories/sync", { method: "POST" });
export const workflowRunAction = ({ data }: { data: { runId: number; action: "cancel" | "rerun" | "rerun-failed" } }) =>
  api<{ ok: true }>(`/api/v1/workflow-runs/${data.runId}/action`, { method: "POST", body: { action: data.action } });
export const runnerLogsAction = ({ data }: { data: { runnerId: string } }) =>
  api<{ runnerId: string; name: string; logs: string }>(`/api/v1/runners/${data.runnerId}/logs`);
export const archivedLogsAction = ({ data }: { data: { streamId: string } }) =>
  api<{ streamId: string; name: string; logs: string }>(`/api/v1/log-streams/${data.streamId}/logs`);
export const searchAction = ({ data }: { data: { query: string } }) =>
  api<Array<{ kind: string; id: string; title: string; subtitle: string; href: string }>>(`/api/v1/search?q=${encodeURIComponent(data.query)}`);
export const retryWebhookAction = ({ data }: { data: { deliveryId: string } }) =>
  api<{ ok: true }>(`/api/v1/webhooks/${data.deliveryId}/retry`, { method: "POST" });
export const saveSettingsAction = ({ data }: { data: {
  logRetentionDays: number; webhookRetentionDays: number; auditRetentionDays: number;
  reconcileIntervalSeconds: number; autoUpdateImages: boolean;
} }) => api<{ ok: true }>("/api/v1/settings", { method: "PUT", body: data });
export const createGitHubAppManifestAction = ({ data }: { data: {
  ownerType: "user" | "organization"; organization?: string; name?: string;
} }) => api<{ action: string; state: string; manifest: string; webhookActive: boolean }>(
  "/api/v1/github-app/manifest",
  { method: "POST", body: data },
);
