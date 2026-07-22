import {
  type CreateRunnerPoolInput,
  type UpdateRunnerPoolInput,
  parseCreateRunnerPoolInput,
  parseUpdateRunnerPoolInput,
} from "./schemas";
import { api } from "~/lib/api";

type BaseOptions = {
  installations: Array<{ id: number; accountLogin: string; accountType: string }>;
  repositories: Array<{ id: number; installationId: number; fullName: string; private: boolean }>;
  runnerGroups: Array<{ installationId: number; id: number; name: string; visibility: string; isDefault: boolean }>;
};

export type RunnerGroupOption = {
  id: number;
  name: string;
  visibility: string;
  isDefault: boolean;
};

export type RepositoryOption = {
  id: number;
  installationId: number;
  accountLogin: string;
  accountType: string;
  fullName: string;
  private: boolean;
};

export type RunnerPoolOptions = (BaseOptions & {
  authenticated: false;
  installUrl: null;
  defaults: null;
}) | (BaseOptions & {
  authenticated: true;
  installUrl: string;
  defaults: {
    provider: "docker" | "tart"; providers: Array<"docker" | "tart">; image: string; dockerImage: string; tartImage: string; labels: string[]; cpuLimit: number; memoryLimitMb: number;
    desiredCount: number; minCount: number; maxCount: number; autoscalingEnabled: boolean;
    queueScaleFactor: number; idleTimeoutMinutes: number; runnerGroupId: number;
    maxCpuLimit: number; maxMemoryLimitMb: number;
  };
});

export type RunnerPoolDetail = {
  id: string;
  installationId: number;
  repositoryId: number | null;
  repository: string | null;
  repositoryIds: number[];
  repositories: RepositoryOption[];
  accountLogin: string;
  name: string;
  scope: "repository" | "organization";
  mode: "ephemeral" | "persistent";
  provider: "docker" | "tart";
  providers: Array<"docker" | "tart">;
  labels: string[];
  image: string;
  dockerImage: string;
  tartImage: string;
  desiredCount: number;
  minCount: number;
  maxCount: number;
  cpuLimit: number;
  memoryLimitMb: number;
  maxCpuLimit?: number;
  maxMemoryLimitMb?: number;
  runnerGroupId: number;
  paused: boolean;
  state: string;
  autoscalingEnabled: boolean;
  queueScaleFactor: number;
  idleTimeoutMinutes: number;
  configurationVersion: number;
  provisionFailureCount: number;
  provisionRetryAt: string | null;
  provisionCircuitOpen: boolean;
  canManage: boolean;
};

export type RunnerPoolEvent = {
  id: string; runnerId: string | null; level: "info" | "warning" | "error"; event: string;
  message: string; metadata: string; createdAt: string;
  capacitySnapshot?: {
    cpuBudget: number; memoryBudgetMb: number; maxRunners: number;
    active: { activeRunners: number; cpu: number; memoryMb: number };
    reserved: { activeRunners: number; cpu: number; memoryMb: number };
  };
};

export const getCreateRunnerPoolOptions = () => api<RunnerPoolOptions>("/api/v1/runner-pools/options");
export const getRunnerPoolAction = ({ data }: { data: { poolId: string } }) =>
  api<RunnerPoolDetail>(`/api/v1/runner-pools/${data.poolId}`);
export const getRunnerPoolEvents = (poolId: string, page = 1, signal?: AbortSignal) =>
  api<{ authenticated: boolean; items: RunnerPoolEvent[]; total: number; page: number; perPage: number }>(
    `/api/v1/runner-pools/${encodeURIComponent(poolId)}/events?page=${page}&perPage=25`,
    { signal },
  );
export const getInstallationRunnerGroups = (installationId: number, signal?: AbortSignal) =>
  api<{ items: RunnerGroupOption[] }>(`/api/v1/installations/${installationId}/runner-groups`, { signal });
export const getInstallationRepositories = (installationId: number, signal?: AbortSignal) =>
  api<{ items: RepositoryOption[] }>(`/api/v1/installations/${installationId}/repositories`, { signal });
export const getRunnerPoolRepositories = (signal?: AbortSignal) =>
  api<{ items: RepositoryOption[] }>("/api/v1/runner-pools/repositories", { signal });

export function createRunnerPoolAction({ data }: { data: CreateRunnerPoolInput }) {
  const input = parseCreateRunnerPoolInput(data);
  return api<{ id: string }>("/api/v1/runner-pools", { method: "POST", body: input });
}

export function updateRunnerPoolAction({ data }: { data: UpdateRunnerPoolInput & { poolId: string } }) {
  const { poolId, ...values } = data;
  const input = parseUpdateRunnerPoolInput(values);
  return api<{ ok: true; configurationVersion: number; rollingReplacement: boolean }>(
    `/api/v1/runner-pools/${poolId}`,
    { method: "PUT", body: input },
  );
}

export function runnerPoolAction({ data }: { data: {
  action: "pause" | "resume" | "retry" | "reconcile" | "delete" | "scale";
  poolId: string;
  desiredCount?: number;
} }) {
  if (data.action === "delete") return api<{ ok: true }>(`/api/v1/runner-pools/${data.poolId}`, { method: "DELETE" });
  return api<{ ok: true }>(`/api/v1/runner-pools/${data.poolId}/action`, {
    method: "POST", body: { action: data.action, desiredCount: data.desiredCount },
  });
}

export function runnerAction({ data }: { data: {
  runnerId: string;
  action: "start" | "stop" | "pause" | "resume" | "restart" | "rebuild" | "delete";
} }) {
  return api<{ ok: true }>(`/api/v1/runners/${data.runnerId}/action`, { method: "POST", body: { action: data.action } });
}
