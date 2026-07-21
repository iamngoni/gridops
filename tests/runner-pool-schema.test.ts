import { describe, expect, it } from "vitest";

import { createRunnerPoolSchema, updateRunnerPoolSchema } from "~/features/runner-pools/schemas";

const validPool = {
  installationId: 123,
  repositoryId: 456,
  name: "linux-general",
  scope: "repository" as const,
  mode: "ephemeral" as const,
  labels: ["gridops", "docker"],
  image: "ghcr.io/actions/actions-runner:latest",
  desiredCount: 1,
  minCount: 0,
  maxCount: 10,
  autoscalingEnabled: true,
  queueScaleFactor: 1,
  idleTimeoutMinutes: 5,
  cpuLimit: 2,
  memoryLimitMb: 4096,
  runnerGroupId: 1,
};

describe("runner pool validation", () => {
  it("accepts a complete repository pool", () => {
    expect(createRunnerPoolSchema.parse(validPool)).toMatchObject(validPool);
  });

  it("requires a repository for repository scope", () => {
    const result = createRunnerPoolSchema.safeParse({ ...validPool, repositoryId: null });
    expect(result.success).toBe(false);
  });

  it("rejects a repository on organization scope", () => {
    const result = createRunnerPoolSchema.safeParse({ ...validPool, scope: "organization" });
    expect(result.success).toBe(false);
  });

  it("enforces minimum, desired, and maximum capacity ordering", () => {
    expect(createRunnerPoolSchema.safeParse({ ...validPool, minCount: 4, desiredCount: 2 }).success).toBe(false);
    expect(createRunnerPoolSchema.safeParse({ ...validPool, desiredCount: 11 }).success).toBe(false);
  });

  it("rejects unsafe names and excessive resources", () => {
    expect(createRunnerPoolSchema.safeParse({ ...validPool, name: "Bad Pool" }).success).toBe(false);
    expect(createRunnerPoolSchema.safeParse({ ...validPool, cpuLimit: 128 }).success).toBe(false);
  });

  it("validates editable configuration without allowing a destination change", () => {
    const { installationId: _installationId, repositoryId: _repositoryId, scope: _scope, ...configuration } = validPool;
    expect(updateRunnerPoolSchema.parse(configuration)).toEqual(configuration);
    expect(updateRunnerPoolSchema.safeParse({ ...configuration, desiredCount: 11, maxCount: 10 }).success).toBe(false);
    expect(updateRunnerPoolSchema.safeParse({ ...configuration, installationId: 999 }).success).toBe(true);
    expect("installationId" in updateRunnerPoolSchema.parse({ ...configuration, installationId: 999 })).toBe(false);
  });
});
