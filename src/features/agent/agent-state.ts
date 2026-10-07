import type {
  AgentRunOutcome, AgentRunStatus, AiApiKeyProviderId, AiConnection, AiModel, AiProviderId, AiSubscriptionProviderId, JobAgentState,
} from "./agent.functions";
import type { HealthTone } from "~/components/status-icon";

/** How often an unfinished agent run is re-read while its page is visible. */
export const AGENT_RUN_POLL_MS = 2_500;

export const DAILY_LIMIT_MIN = 1;
export const DAILY_LIMIT_MAX = 100;

export function isTerminalAgentStatus(status: AgentRunStatus | null | undefined) {
  return status === "succeeded" || status === "failed" || status === "cancelled";
}

export type FixButtonState =
  | { visible: false }
  | { visible: true; disabled: boolean; reason: string | null };

/**
 * Whether a job offers "Fix with agent". Hidden unless an AI provider is set up
 * and the job failed; disabled, with the server's reason, when it can't run, and
 * while a run for this job is still going.
 */
export function fixButtonState({
  agent,
  conclusion,
  currentRunStatus,
}: {
  agent: JobAgentState | null | undefined;
  conclusion: string | null | undefined;
  currentRunStatus?: AgentRunStatus | null;
}): FixButtonState {
  if (!agent?.available || conclusion !== "failure") return { visible: false };
  if (currentRunStatus && !isTerminalAgentStatus(currentRunStatus)) {
    return { visible: true, disabled: true, reason: "The agent is already working on this job." };
  }
  if (!agent.canRun) return { visible: true, disabled: true, reason: agent.reason ?? "The agent can’t run for this job right now." };
  return { visible: true, disabled: false, reason: null };
}

/** The glyph vocabulary `RunStatusIcon` understands. */
export function agentRunIconStatus(status: AgentRunStatus) {
  switch (status) {
    case "queued": return "queued";
    case "running": return "in_progress";
    case "succeeded": return "success";
    case "failed": return "failure";
    case "cancelled": return "cancelled";
  }
}

export function agentRunTone(status: AgentRunStatus): HealthTone {
  switch (status) {
    case "queued": return "warning";
    case "running": return "progress";
    case "succeeded": return "success";
    case "failed": return "danger";
    case "cancelled": return "neutral";
  }
}

/** One line for a run's state: "Opened pull request #42", "Running", "Failed". */
export function agentRunLabel(run: { status: AgentRunStatus; outcome: AgentRunOutcome | null; pullRequestNumber: number | null }) {
  switch (run.status) {
    case "queued": return "Queued";
    case "running": return "Running";
    case "failed": return "Failed";
    case "cancelled": return "Cancelled";
    case "succeeded":
      if (run.outcome === "pull_request") return run.pullRequestNumber ? `Opened pull request #${run.pullRequestNumber}` : "Opened a pull request";
      if (run.outcome === "diagnosis") return "Diagnosed the failure";
      return "Finished";
  }
}

/** Elapsed time as "8s", "3m 4s", or "1h 2m"; open-ended runs count up to `now`. */
export function formatElapsed(startedAt: string | null | undefined, completedAt: string | null | undefined, now = Date.now()) {
  if (!startedAt) return null;
  const start = new Date(startedAt).getTime();
  const end = completedAt ? new Date(completedAt).getTime() : now;
  if (Number.isNaN(start) || Number.isNaN(end)) return null;
  const seconds = Math.max(0, Math.floor((end - start) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

export function triggerLabel(trigger: "manual" | "automatic") {
  return trigger === "automatic" ? "Automatic" : "Manual";
}

/** Server vocabulary as words: "high" → "High", "pull_requests" → "Pull requests". */
export function sentenceCase(value: string) {
  const text = value.replaceAll(/[_-]/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export function isSubscriptionProvider(id: AiProviderId): id is AiSubscriptionProviderId {
  return id === "codex" || id === "claude_code";
}

export function isApiKeyProvider(id: AiProviderId): id is AiApiKeyProviderId {
  return id === "openai" || id === "anthropic" || id === "openrouter";
}

/** How a connection identifies its account without revealing a key. */
export function connectionAccount(connection: Pick<AiConnection, "accountLabel" | "fingerprint" | "authMethod">) {
  if (connection.accountLabel) return connection.accountLabel;
  if (connection.fingerprint) return `••••${connection.fingerprint}`;
  return connection.authMethod === "api_key" ? "API key" : "Subscription";
}

export function connectionTone(status: AiConnection["status"]): HealthTone {
  if (status === "ready") return "success";
  if (status === "needs_reconnect") return "warning";
  return "danger";
}

export function connectionStatusLabel(status: AiConnection["status"]) {
  if (status === "ready") return "Ready";
  if (status === "needs_reconnect") return "Needs reconnecting";
  return "Error";
}

/**
 * Model choices for a connection. A saved model the provider no longer lists
 * (or that couldn't be loaded) stays selectable so saving doesn't drop it.
 */
export function modelChoices(models: AiModel[], savedModel: string | null): AiModel[] {
  if (!savedModel || models.some((model) => model.id === savedModel)) return models;
  return [...models, { id: savedModel, name: savedModel, reasoningEfforts: [], defaultEffort: null }];
}

/**
 * The reasoning effort to keep after choosing a model: the current one when the
 * model supports it, else the model's default. Unknown models keep `current`.
 */
export function effortForModel(model: AiModel | undefined, current: string | null) {
  if (!model) return current;
  if (!model.reasoningEfforts.length) return null;
  if (current && model.reasoningEfforts.includes(current)) return current;
  return model.defaultEffort && model.reasoningEfforts.includes(model.defaultEffort) ? model.defaultEffort : model.reasoningEfforts[0] ?? null;
}

/** A daily limit within the server's accepted range, or null when it isn't a whole number. */
export function parseDailyLimit(value: string) {
  if (!/^\d+$/.test(value.trim())) return null;
  const limit = Number(value);
  return limit >= DAILY_LIMIT_MIN && limit <= DAILY_LIMIT_MAX ? limit : null;
}
