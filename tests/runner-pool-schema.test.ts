import { describe, expect, it } from "vitest";

import {
  createRunnerPoolSchema,
  parseCreateRunnerPoolInput,
  updateRunnerPoolSchema,
} from "~/features/runner-pools/schemas";

const validPool = {
  installationId: 123,
  repositoryIds: [456],
  name: "linux-general",
  scope: "repository" as const,
  mode: "ephemeral" as const,
  provider: "docker" as const,
  providers: ["docker" as const],
  labels: ["gridops", "docker"],
  image: "ghcr.io/actions/actions-runner:latest",
  dockerImage: "ghcr.io/actions/actions-runner:latest",
  tartImage: "gridops-macos-tahoe-base",
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
    const result = createRunnerPoolSchema.safeParse({ ...validPool, repositoryIds: [] });
    expect(result.success).toBe(false);
  });

  it("turns validation failures into concise user-facing errors", () => {
    expect(() => parseCreateRunnerPoolInput({ ...validPool, repositoryIds: [] }))
      .toThrow("Choose at least one repository for a repository-scoped pool.");
  });

  it("rejects a repository on organization scope", () => {
    const result = createRunnerPoolSchema.safeParse({ ...validPool, scope: "organization" });
    expect(result.success).toBe(false);
  });

  it("limits repository assignments to maximum runner capacity", () => {
    const result = createRunnerPoolSchema.safeParse({
      ...validPool,
      repositoryIds: [1, 2, 3],
      maxCount: 2,
    });
    expect(result.success).toBe(false);
  });

  it("enforces minimum, desired, and maximum capacity ordering", () => {
    expect(createRunnerPoolSchema.safeParse({ ...validPool, minCount: 4, desiredCount: 2 }).success).toBe(false);
    expect(createRunnerPoolSchema.safeParse({ ...validPool, desiredCount: 11 }).success).toBe(false);
  });

  it("rejects unsafe names while allowing resource values above the old configuration cap", () => {
    expect(createRunnerPoolSchema.safeParse({ ...validPool, name: "Bad Pool" }).success).toBe(false);
    expect(createRunnerPoolSchema.safeParse({ ...validPool, cpuLimit: 128 }).success).toBe(true);
    expect(createRunnerPoolSchema.safeParse({ ...validPool, labels: ["macOS"] }).success).toBe(false);
  });

  it("enforces Tart's ephemeral VM resource shape", () => {
    const tartPool = {
      ...validPool,
      provider: "tart" as const,
      providers: ["tart" as const],
      image: "gridops-macos-tahoe-base",
    };
    expect(createRunnerPoolSchema.safeParse(tartPool).success).toBe(true);
    expect(createRunnerPoolSchema.safeParse({ ...tartPool, mode: "persistent" }).success).toBe(false);
    expect(createRunnerPoolSchema.safeParse({ ...tartPool, cpuLimit: 1.5 }).success).toBe(false);
    expect(createRunnerPoolSchema.safeParse({ ...tartPool, memoryLimitMb: 1_024 }).success).toBe(true);
  });

  it("accepts a mixed Docker and Tart pool with one shared capacity limit", () => {
    const mixedPool = {
      ...validPool,
      providers: ["docker" as const, "tart" as const],
    };
    expect(createRunnerPoolSchema.safeParse(mixedPool).success).toBe(true);
    expect(createRunnerPoolSchema.safeParse({ ...mixedPool, mode: "persistent" }).success).toBe(false);
    expect(createRunnerPoolSchema.safeParse({ ...mixedPool, providers: [] }).success).toBe(false);
  });

  it("validates editable configuration without allowing a destination change", () => {
    const { installationId: _installationId, scope: _scope, ...configuration } = validPool;
    expect(updateRunnerPoolSchema.parse(configuration)).toEqual(configuration);
    expect(updateRunnerPoolSchema.safeParse({ ...configuration, desiredCount: 11, maxCount: 10 }).success).toBe(false);
    expect(updateRunnerPoolSchema.safeParse({ ...configuration, installationId: 999 }).success).toBe(true);
    expect("installationId" in updateRunnerPoolSchema.parse({ ...configuration, installationId: 999 })).toBe(false);
  });
});
