export type HostResourceWarning = {
  cpuRequested: number;
  cpuBudget: number;
  memoryRequestedMb: number;
  memoryBudgetMb: number;
  runnerCount: number;
};

export function hostResourceWarning(input: {
  cpuLimit: number;
  cpuBudget: number;
  memoryBudgetMb: number;
  memoryLimitMb: number;
  runnerCount: number;
}): HostResourceWarning | null {
  const values = [
    input.cpuLimit,
    input.cpuBudget,
    input.memoryBudgetMb,
    input.memoryLimitMb,
    input.runnerCount,
  ];
  if (values.some((value) => !Number.isFinite(value) || value <= 0)) return null;

  const cpuRequested = input.cpuLimit * input.runnerCount;
  const memoryRequestedMb = input.memoryLimitMb * input.runnerCount;
  if (cpuRequested <= input.cpuBudget && memoryRequestedMb <= input.memoryBudgetMb) return null;

  return { ...input, cpuRequested, memoryRequestedMb };
}

export function formatResourceNumber(value: number) {
  return Number.isInteger(value) ? String(value) : value.toFixed(1).replace(/\.0$/, "");
}
