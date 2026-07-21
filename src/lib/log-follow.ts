export const LOG_FOLLOW_THRESHOLD_PX = 48;

type FollowableStep = {
  conclusion: string | null;
  number: number;
  status: string;
};

export function isNearLogEnd(
  metrics: Pick<HTMLElement, "clientHeight" | "scrollHeight" | "scrollTop">,
  threshold = LOG_FOLLOW_THRESHOLD_PX,
) {
  return metrics.scrollHeight - metrics.scrollTop - metrics.clientHeight <= threshold;
}

export function advanceFollowedSteps(steps: FollowableStep[], expanded: ReadonlySet<number>) {
  const next = new Set(expanded);

  for (const step of steps) {
    if (step.status === "completed" && step.conclusion !== "failure") {
      next.delete(step.number);
    }
  }

  const followTarget = steps.find((step) => step.status === "in_progress")
    ?? steps.find((step) => step.status === "queued");
  if (followTarget) next.add(followTarget.number);

  return next;
}
