import { describe, expect, it } from "vitest";

import { hostResourceWarning } from "~/features/runner-pools/resource-risk";

describe("hostResourceWarning", () => {
  it("stays quiet when the current target fits the host budget", () => {
    expect(hostResourceWarning({
      runnerCount: 1,
      cpuLimit: 2,
      memoryLimitMb: 2048,
      cpuBudget: 9,
      memoryBudgetMb: 7604,
    })).toBeNull();
  });

  it("warns when the current target can exceed CPU or memory capacity", () => {
    expect(hostResourceWarning({
      runnerCount: 3,
      cpuLimit: 4,
      memoryLimitMb: 4096,
      cpuBudget: 9,
      memoryBudgetMb: 7604,
    })).toMatchObject({ cpuRequested: 12, memoryRequestedMb: 12_288 });
  });
});
