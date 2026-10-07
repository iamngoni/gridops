import { describe, expect, it } from "vitest";

import {
  agentRunIconStatus,
  agentRunLabel,
  agentRunTone,
  connectionAccount,
  effortForModel,
  fixButtonState,
  formatElapsed,
  isTerminalAgentStatus,
  modelChoices,
  parseDailyLimit,
  sentenceCase,
} from "~/features/agent/agent-state";
import type { AgentRunStatus, AiModel, JobAgentState } from "~/features/agent/agent.functions";

const ready: JobAgentState = { available: true, canRun: true, reason: null, latestRun: null };

describe("fix with agent button", () => {
  it("stays hidden when no provider is configured", () => {
    expect(fixButtonState({ agent: { ...ready, available: false }, conclusion: "failure" })).toEqual({ visible: false });
  });

  it("stays hidden for servers that don't report agent state", () => {
    expect(fixButtonState({ agent: undefined, conclusion: "failure" })).toEqual({ visible: false });
  });

  it("only appears on failed jobs", () => {
    expect(fixButtonState({ agent: ready, conclusion: "success" })).toEqual({ visible: false });
    expect(fixButtonState({ agent: ready, conclusion: null })).toEqual({ visible: false });
    expect(fixButtonState({ agent: ready, conclusion: "failure" })).toEqual({ visible: true, disabled: false, reason: null });
  });

  it("is disabled with the server's reason when the agent can't run", () => {
    expect(fixButtonState({ agent: { ...ready, canRun: false, reason: "Only administrators can start a fix." }, conclusion: "failure" }))
      .toEqual({ visible: true, disabled: true, reason: "Only administrators can start a fix." });
  });

  it("falls back to a generic reason", () => {
    const state = fixButtonState({ agent: { ...ready, canRun: false }, conclusion: "failure" });
    expect(state.visible && state.disabled && state.reason).toBeTruthy();
  });

  it("is disabled while a run for the job is unfinished, and offered again once it ends", () => {
    expect(fixButtonState({ agent: ready, conclusion: "failure", currentRunStatus: "running" })).toMatchObject({ visible: true, disabled: true });
    expect(fixButtonState({ agent: ready, conclusion: "failure", currentRunStatus: "queued" })).toMatchObject({ visible: true, disabled: true });
    expect(fixButtonState({ agent: ready, conclusion: "failure", currentRunStatus: "failed" })).toEqual({ visible: true, disabled: false, reason: null });
  });
});

describe("agent run status", () => {
  it("treats succeeded, failed, and cancelled as terminal", () => {
    const terminal = (["queued", "running", "succeeded", "failed", "cancelled"] as AgentRunStatus[]).filter((status) => isTerminalAgentStatus(status));
    expect(terminal).toEqual(["succeeded", "failed", "cancelled"]);
    expect(isTerminalAgentStatus(null)).toBe(false);
  });

  it("maps statuses onto run glyphs and tones", () => {
    expect(agentRunIconStatus("running")).toBe("in_progress");
    expect(agentRunIconStatus("succeeded")).toBe("success");
    expect(agentRunIconStatus("failed")).toBe("failure");
    expect(agentRunTone("running")).toBe("progress");
    expect(agentRunTone("failed")).toBe("danger");
    expect(agentRunTone("cancelled")).toBe("neutral");
  });

  it("describes the outcome of a finished run", () => {
    expect(agentRunLabel({ status: "succeeded", outcome: "pull_request", pullRequestNumber: 42 })).toBe("Opened pull request #42");
    expect(agentRunLabel({ status: "succeeded", outcome: "diagnosis", pullRequestNumber: null })).toBe("Diagnosed the failure");
    expect(agentRunLabel({ status: "running", outcome: null, pullRequestNumber: null })).toBe("Running");
    expect(agentRunLabel({ status: "failed", outcome: null, pullRequestNumber: null })).toBe("Failed");
  });
});

describe("elapsed time", () => {
  const start = "2026-10-07T10:00:00.000Z";

  it("formats seconds, minutes, and hours", () => {
    expect(formatElapsed(start, "2026-10-07T10:00:08.900Z")).toBe("8s");
    expect(formatElapsed(start, "2026-10-07T10:03:04.000Z")).toBe("3m 4s");
    expect(formatElapsed(start, "2026-10-07T11:02:30.000Z")).toBe("1h 2m");
  });

  it("counts an unfinished run up to now", () => {
    expect(formatElapsed(start, null, new Date("2026-10-07T10:00:45.000Z").getTime())).toBe("45s");
  });

  it("returns nothing before the run starts and never goes negative", () => {
    expect(formatElapsed(null, null)).toBeNull();
    expect(formatElapsed(start, "2026-10-07T09:59:00.000Z")).toBe("0s");
  });
});

describe("model and effort choices", () => {
  const models: AiModel[] = [
    { id: "gpt-5-codex", name: "GPT-5 Codex", reasoningEfforts: ["low", "medium", "high"], defaultEffort: "medium" },
    { id: "gpt-4.1", name: "GPT-4.1", reasoningEfforts: [], defaultEffort: null },
  ];

  it("keeps a saved model selectable when the provider doesn't list it", () => {
    expect(modelChoices([], "claude-sonnet")).toEqual([{ id: "claude-sonnet", name: "claude-sonnet", reasoningEfforts: [], defaultEffort: null }]);
    expect(modelChoices(models, "gpt-4.1")).toBe(models);
    expect(modelChoices(models, null)).toBe(models);
  });

  it("keeps a supported effort and falls back to the model's default", () => {
    expect(effortForModel(models[0], "high")).toBe("high");
    expect(effortForModel(models[0], "xhigh")).toBe("medium");
    expect(effortForModel(models[0], null)).toBe("medium");
  });

  it("drops the effort for models without effort levels and keeps it for unknown ones", () => {
    expect(effortForModel(models[1], "high")).toBeNull();
    expect(effortForModel(undefined, "high")).toBe("high");
  });
});

describe("settings helpers", () => {
  it("accepts whole daily limits from 1 to 100", () => {
    expect(parseDailyLimit("1")).toBe(1);
    expect(parseDailyLimit(" 100 ")).toBe(100);
    expect(parseDailyLimit("0")).toBeNull();
    expect(parseDailyLimit("101")).toBeNull();
    expect(parseDailyLimit("2.5")).toBeNull();
    expect(parseDailyLimit("")).toBeNull();
  });

  it("identifies an account without exposing a key", () => {
    expect(connectionAccount({ accountLabel: "ops@example.com", fingerprint: "a1b2", authMethod: "subscription_oauth" })).toBe("ops@example.com");
    expect(connectionAccount({ accountLabel: null, fingerprint: "a1b2", authMethod: "api_key" })).toBe("••••a1b2");
    expect(connectionAccount({ accountLabel: null, fingerprint: null, authMethod: "api_key" })).toBe("API key");
  });

  it("turns server vocabulary into words", () => {
    expect(sentenceCase("pull_requests")).toBe("Pull requests");
    expect(sentenceCase("high")).toBe("High");
  });
});
