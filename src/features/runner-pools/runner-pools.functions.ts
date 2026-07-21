import {
  createRunnerPoolSchema,
  type CreateRunnerPoolInput,
  updateRunnerPoolSchema,
  type UpdateRunnerPoolInput,
} from "./schemas";
import { api } from "~/lib/api";

type BaseOptions = {
  installations: Array<{ id: number; accountLogin: string; accountType: string }>;
  repositories: Array<{ id: number; installationId: number; fullName: string; private: boolean }>;
  runnerGroups: Array<{ installationId: number; id: number; name: string; visibility: string; isDefault: boolean }>;
};

export type RunnerPoolOptions = (BaseOptions & {
  authenticated: false;
  installUrl: null;
  defaults: null;
}) | (BaseOptions & {
  authenticated: true;
  installUrl: string;
  defaults: {
    image: string; labels: string[]; cpuLimit: number; memoryLimitMb: number;
    desiredCount: number; minCount: number; maxCount: number; autoscalingEnabled: boolean;
    queueScaleFactor: number; idleTimeoutMinutes: number; runnerGroupId: number;
  };
});

export type RunnerPoolDetail = {
  id: string;
  installationId: number;
  repositoryId: number | null;
  repository: string | null;
  accountLogin: string;
  name: string;
  scope: "repository" | "organization";
  mode: "ephemeral" | "persistent";
  labels: string[];
  image: string;
  desiredCount: number;
  minCount: number;
  maxCount: number;
  cpuLimit: number;
  memoryLimitMb: number;
  runnerGroupId: number;
  paused: boolean;
  state: string;
  autoscalingEnabled: boolean;
  queueScaleFactor: number;
  idleTimeoutMinutes: number;
  configurationVersion: number;
};

export const getCreateRunnerPoolOptions = () => api<RunnerPoolOptions>("/api/v1/runner-pools/options");
export const getRunnerPoolAction = ({ data }: { data: { poolId: string } }) =>
  api<RunnerPoolDetail>(`/api/v1/runner-pools/${data.poolId}`);

export function createRunnerPoolAction({ data }: { data: CreateRunnerPoolInput }) {
  const input = createRunnerPoolSchema.parse(data);
  return api<{ id: string }>("/api/v1/runner-pools", { method: "POST", body: input });
}

export function updateRunnerPoolAction({ data }: { data: UpdateRunnerPoolInput & { poolId: string } }) {
  const { poolId, ...values } = data;
  const input = updateRunnerPoolSchema.parse(values);
  return api<{ ok: true; configurationVersion: number; rollingReplacement: boolean }>(
    `/api/v1/runner-pools/${poolId}`,
    { method: "PUT", body: input },
  );
}

export function runnerPoolAction({ data }: { data: {
  action: "pause" | "resume" | "reconcile" | "delete" | "scale";
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
