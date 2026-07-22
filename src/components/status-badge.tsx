import { Badge } from "./ui/badge";

export function StatusBadge({ status }: { status: string | null | undefined }) {
  const normalized = (status || "unknown").toLowerCase();
  const variant = ["success", "active", "healthy", "online", "idle", "processed"].includes(normalized)
    ? "success"
    : ["queued", "paused", "draining", "requested", "received", "waiting", "backoff", "provisioning-paused"].includes(normalized)
      ? "warning"
      : ["failure", "failed", "error", "rejected", "deleted", "dead", "blocked"].includes(normalized)
        ? "destructive"
        : ["in_progress", "running", "busy", "starting"].includes(normalized)
          ? "info"
          : "outline";
  return <Badge variant={variant}>{normalized.replaceAll(/[_-]/g, " ")}</Badge>;
}
