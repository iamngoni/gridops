export const LOG_FOLLOW_THRESHOLD_PX = 48;

export function isNearLogEnd(
  metrics: Pick<HTMLElement, "clientHeight" | "scrollHeight" | "scrollTop">,
  threshold = LOG_FOLLOW_THRESHOLD_PX,
) {
  return metrics.scrollHeight - metrics.scrollTop - metrics.clientHeight <= threshold;
}
