import { cn } from "~/lib/utils";

export type RunState = "queued" | "waiting" | "in_progress" | "success" | "failure" | "cancelled" | "skipped";
export type HealthTone = "success" | "progress" | "warning" | "danger" | "neutral";

const RUN_STATES: Record<string, RunState> = {
  queued: "queued",
  pending: "queued",
  requested: "queued",
  waiting: "waiting",
  action_required: "waiting",
  in_progress: "in_progress",
  success: "success",
  failure: "failure",
  timed_out: "failure",
  startup_failure: "failure",
  cancelled: "cancelled",
  skipped: "skipped",
  neutral: "skipped",
  stale: "skipped",
};

/** GitHub run and job vocabulary; everything else is a resource health state. */
export function runState(status: string | null | undefined): RunState | null {
  return RUN_STATES[(status ?? "").toLowerCase()] ?? null;
}

export function healthTone(status: string | null | undefined): HealthTone {
  const normalized = (status ?? "").toLowerCase();
  if (["active", "healthy", "online", "idle", "processed", "success", "ready", "connected", "verified", "configured"].includes(normalized)) return "success";
  if (["busy", "running", "in_progress", "starting", "scaling", "provisioning", "updating", "rebuilding"].includes(normalized)) return "progress";
  if (["paused", "draining", "waiting", "backoff", "queued", "received", "requested", "provisioning-paused", "pending", "required", "polling"].includes(normalized)) return "warning";
  if (["failed", "failure", "error", "rejected", "dead", "blocked", "invalid", "offline", "suspended"].includes(normalized)) return "danger";
  return "neutral";
}

export function statusLabel(status: string | null | undefined) {
  const text = (status || "unknown").replaceAll(/[_-]/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

const toneColor: Record<HealthTone, string> = {
  success: "var(--success)",
  progress: "var(--info)",
  warning: "var(--warning)",
  danger: "var(--danger)",
  neutral: "var(--neutral)",
};

/** Linear-style circular status glyph for GitHub runs, jobs, and steps. */
export function RunStatusIcon({ status, className, size = 14 }: { status: string | null | undefined; className?: string; size?: number }) {
  const state = runState(status) ?? "skipped";
  const common = { width: size, height: size, viewBox: "0 0 14 14", fill: "none", "aria-hidden": true as const, className: cn("shrink-0", className) };
  switch (state) {
    case "queued":
      return <svg {...common}><circle cx="7" cy="7" r="5.75" stroke="var(--neutral)" strokeWidth="1.5" strokeDasharray="1.6 1.9" /></svg>;
    case "waiting":
      return <svg {...common}><circle cx="7" cy="7" r="5.75" stroke="var(--warning)" strokeWidth="1.5" /><path d="M7 4.2V7l1.8 1.2" stroke="var(--warning)" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" /></svg>;
    case "in_progress":
      return <svg {...common}><circle cx="7" cy="7" r="5.75" stroke="var(--progress)" strokeWidth="1.5" /><g className="status-spin"><path d="M7 3.25A3.75 3.75 0 0 1 7 10.75Z" fill="var(--progress)" /></g></svg>;
    case "success":
      return <svg {...common}><circle cx="7" cy="7" r="6.5" fill="var(--success)" /><path d="m4.4 7.1 1.75 1.75L9.6 5.4" stroke="var(--panel)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>;
    case "failure":
      return <svg {...common}><circle cx="7" cy="7" r="6.5" fill="var(--danger)" /><path d="m5 5 4 4M9 5 5 9" stroke="var(--panel)" strokeWidth="1.5" strokeLinecap="round" /></svg>;
    case "cancelled":
      return <svg {...common}><circle cx="7" cy="7" r="6.5" fill="var(--neutral)" /><path d="m5 5 4 4M9 5 5 9" stroke="var(--panel)" strokeWidth="1.5" strokeLinecap="round" /></svg>;
    default:
      return <svg {...common}><circle cx="7" cy="7" r="5.75" stroke="var(--neutral)" strokeWidth="1.5" /><path d="m4.9 9.1 4.2-4.2" stroke="var(--neutral)" strokeWidth="1.5" strokeLinecap="round" /></svg>;
  }
}

export function StatusDot({ status, tone, className, pulse }: { status?: string | null; tone?: HealthTone; className?: string; pulse?: boolean }) {
  const resolved = tone ?? healthTone(status);
  return (
    <span
      aria-hidden="true"
      className={cn("inline-block size-2 shrink-0 rounded-full", (pulse ?? resolved === "progress") && "live-pulse", className)}
      style={{ backgroundColor: toneColor[resolved] }}
    />
  );
}

/** Icon plus human label, used in properties panels and dense rows. */
export function StatusBadge({ status, className, iconOnly = false }: { status: string | null | undefined; className?: string; iconOnly?: boolean }) {
  const label = statusLabel(status);
  const icon = runState(status) ? <RunStatusIcon status={status} /> : <StatusDot status={status} />;
  if (iconOnly) return <span className={cn("inline-flex", className)} title={label}>{icon}<span className="sr-only">{label}</span></span>;
  return <span className={cn("inline-flex min-w-0 items-center gap-1.5 text-xs font-medium text-secondary-foreground", className)}>{icon}<span className="truncate">{label}</span></span>;
}
