import { api } from "~/lib/api";

export type AiProviderId = "codex" | "claude_code" | "openai" | "anthropic" | "openrouter";
export type AiAuthMethod = "subscription_oauth" | "api_key";
export type AiSubscriptionProviderId = "codex" | "claude_code";
export type AiApiKeyProviderId = "openai" | "anthropic" | "openrouter";

export type AiProviderOption = {
  id: AiProviderId; name: string; authMethod: AiAuthMethod; description: string; keyHint: string | null;
};

export type AiConnectionStatus = "ready" | "needs_reconnect" | "error";

export type AiConnection = {
  id: string; provider: AiProviderId; providerName: string; authMethod: AiAuthMethod;
  accountLabel: string | null; fingerprint: string | null; status: AiConnectionStatus; statusDetail: string | null;
  createdAt: string; updatedAt: string;
};

export type AgentTriggerMode = "manual" | "automatic";

export type AgentSettings = {
  connectionId: string | null; model: string | null; reasoningEffort: string | null;
  triggerMode: AgentTriggerMode; dailyLimit: number;
};

export type AgentGitHubPermissions = {
  ready: boolean;
  missing: Array<{ installationId: number; account: string; permissions: string[] }>;
  appSettingsUrl: string | null;
};

export type AiSettings = {
  canManage: boolean; providers: AiProviderOption[]; connections: AiConnection[]; agent: AgentSettings;
  configured: boolean; github: AgentGitHubPermissions;
};

export type AiModel = { id: string; name: string; reasoningEfforts: string[]; defaultEffort: string | null };

export type AiOAuthAttempt = { attemptId: string; authorizeUrl: string; expiresAt: string; instructions: string };

export type AgentRunStatus = "queued" | "running" | "succeeded" | "failed" | "cancelled";
export type AgentRunOutcome = "pull_request" | "diagnosis";

export type AgentRunSummary = {
  id: string; status: AgentRunStatus; outcome: AgentRunOutcome | null; pullRequestUrl: string | null;
  pullRequestNumber: number | null; createdAt: string; completedAt: string | null;
};

/** The agent fields `GET /workflow-jobs/{id}/logs` adds; older servers omit them. */
export type JobAgentState = { available: boolean; canRun: boolean; reason: string | null; latestRun: AgentRunSummary | null };

export type AgentRunEvent = {
  id: number; kind: "status" | "message" | "tool" | "result" | "error"; title: string; detail: string | null; createdAt: string;
};

export type AgentRun = {
  id: string; jobId: number; runId: number; repository: string; jobName: string;
  status: AgentRunStatus; stage: string | null; outcome: AgentRunOutcome | null;
  summary: string | null; diagnosis: string | null; pullRequestUrl: string | null; pullRequestNumber: number | null;
  branch: string | null; error: string | null; trigger: "manual" | "automatic"; provider: string | null;
  model: string | null; requestedBy: string | null; createdAt: string; startedAt: string | null;
  completedAt: string | null; canCancel: boolean; events: AgentRunEvent[];
};

export const getAiSettings = () => api<AiSettings>("/api/v1/settings/ai");

export const connectAiApiKey = (data: { provider: AiApiKeyProviderId; apiKey: string }) =>
  api<AiConnection>("/api/v1/settings/ai/api-key", { method: "POST", body: data });

export const startAiOAuth = (data: { provider: AiSubscriptionProviderId }) =>
  api<AiOAuthAttempt>("/api/v1/settings/ai/oauth/start", { method: "POST", body: data });

export const completeAiOAuth = (data: { attemptId: string; callbackValue: string }) =>
  api<AiConnection>("/api/v1/settings/ai/oauth/complete", { method: "POST", body: data });

export const removeAiConnection = (connectionId: string) =>
  api<void>(`/api/v1/settings/ai/connections/${encodeURIComponent(connectionId)}`, { method: "DELETE" });

export const getAiConnectionModels = (connectionId: string, signal?: AbortSignal) =>
  api<{ models: AiModel[] }>(`/api/v1/settings/ai/connections/${encodeURIComponent(connectionId)}/models`, { signal });

export const saveAgentSettings = (data: AgentSettings) =>
  api<AiSettings>("/api/v1/settings/ai/agent", { method: "PUT", body: data });

export const startAgentRun = (jobId: number) =>
  api<AgentRun>(`/api/v1/workflow-jobs/${jobId}/agent-runs`, { method: "POST" });

export const getAgentRun = (agentRunId: string, signal?: AbortSignal) =>
  api<AgentRun>(`/api/v1/agent-runs/${encodeURIComponent(agentRunId)}`, { signal });

export const cancelAgentRun = (agentRunId: string) =>
  api<AgentRun>(`/api/v1/agent-runs/${encodeURIComponent(agentRunId)}/cancel`, { method: "POST" });
