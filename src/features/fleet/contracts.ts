/**
 * Runtime schemas for fleet browser requests and authorized API projections.
 * Physical host OS and backend execution OS are independent capabilities.
 */
import { z } from "zod";

const safeInteger = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const counter = safeInteger.min(1);
const uuid = z.string().refine((input) => input.length === 32 || input.length === 36
  || (input.length === 45 && input.startsWith("urn:uuid:"))
  || (input.length === 38 && input.startsWith("{") && input.endsWith("}")))
  .transform((input) => {
  const value = input.startsWith("urn:uuid:") ? input.slice(9)
    : input.startsWith("{") && input.endsWith("}") ? input.slice(1, -1) : input;
  return value.toLowerCase();
}).pipe(z.string().regex(/^(?:[0-9a-f]{32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/)
  .refine((value) => /[1-9a-f]/.test(value), "UUID must not be nil"))
  .transform((value) => value.includes("-") ? value : `${value.slice(0, 8)}-${value.slice(8, 12)}-${value.slice(12, 16)}-${value.slice(16, 20)}-${value.slice(20)}`);
const text = z.string().refine((value) => [...value].length > 0 && [...value].length <= 128 && !/[\p{Cc}]/u.test(value));
const timestamp = z.string().max(32)
  .regex(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]00:00)$/)
  .refine((value) => {
    const match = /^(\d{4}-\d{2}-\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?/.exec(value);
    if (!match || Number(match[2]) > 23 || Number(match[3]) > 59 || Number(match[4]) > 59) return false;
    const fraction = match[5] ?? "";
    if (/[1-9]/.test(fraction.slice(3))) return false;
    const parsed = new Date(value);
    return Number.isFinite(parsed.getTime()) && parsed.toISOString().slice(0, 10) === match[1];
  }).transform((value) => new Date(value).toISOString());
const os = z.enum(["linux", "macos", "windows"]);
const architecture = z.enum(["x64", "arm64"]);
const runtime = z.enum(["docker", "tart_vm", "native_process"]);
const cursor = z.string().min(1).max(4096).regex(/^[a-zA-Z0-9._-]+$/);
const operationUrl = z.string().startsWith("/api/v1/operations/").superRefine((value, context) => {
  if (!uuid.safeParse(value.slice("/api/v1/operations/".length)).success) {
    context.addIssue({ code: "custom", message: "Invalid operation URL" });
  }
}).transform((value) => `/api/v1/operations/${uuid.parse(value.slice("/api/v1/operations/".length))}`);

export const rejectionReasonSchema = z.enum([
  "forbidden", "revoked_grant", "native_trust_required", "docker_socket_trust_required",
  "incompatible_runtime", "incompatible_os", "incompatible_architecture", "unsupported_mode",
  "capability_unverified", "stale_health", "stale_inventory", "offline", "pressure_unknown",
  "resource_pressure", "resource_unsupported", "session_required", "session_unavailable", "image_unavailable",
  "physical_budget", "child_budget", "pool_limit", "lifecycle_blocked", "profile_revision_changed",
  "authority_unavailable", "no_eligible_host",
]);
export const resourceAmountSchema = z.strictObject({
  cpuMillis: safeInteger, memoryMib: safeInteger, diskBytes: safeInteger,
});
export const hostSummarySchema = z.strictObject({
  id: uuid, name: text, hostOs: os, architecture,
  enrollment: z.enum(["pending", "approved", "revoked"]),
  lifecycle: z.enum(["active", "paused", "draining", "maintenance", "retired"]),
  integrity: z.enum(["unverified", "verified", "quarantined"]),
  freshness: z.enum(["fresh", "stale", "offline", "unknown"]),
  pressure: z.enum(["normal", "pressured", "unknown"]),
  revision: counter, agentVersion: text.nullish(), lastHeartbeatAt: timestamp.nullish(),
  backendCount: safeInteger, runnerCount: safeInteger,
  budget: resourceAmountSchema.nullish(), allocated: resourceAmountSchema.nullish(),
});
export const backendSummarySchema = z.strictObject({
  id: uuid, hostId: uuid, runtimeKind: runtime, executionOs: os, architecture,
  readiness: z.enum(["ready", "reconciling", "unavailable", "unsupported", "unknown"]),
  reason: rejectionReasonSchema.nullish(), revision: counter,
});
export const operationAcceptedSchema = z.strictObject({
  operationId: uuid, status: z.literal("accepted"), statusUrl: operationUrl,
}).refine((value) => uuid.safeParse(value.statusUrl.slice("/api/v1/operations/".length)).data === value.operationId, "Operation URL must match its ID");
export const operationStatusSchema = z.strictObject({
  operationId: uuid,
  status: z.enum(["queued", "running", "succeeded", "failed", "cancelled", "blocked"]),
  reason: rejectionReasonSchema.nullish(), updatedAt: timestamp,
});
export const placementExplanationSchema = z.strictObject({
  selected: z.strictObject({ hostId: uuid, backendId: uuid }).nullish(),
  rejections: z.array(z.strictObject({ hostId: uuid.nullish(), backendId: uuid.nullish(), reason: rejectionReasonSchema })).max(1000),
});
export const placementOutcomeSchema = z.discriminatedUnion("status", [
  z.strictObject({ status: z.literal("queued"), operationId: uuid, workloadId: uuid, placementId: z.null(), explanation: placementExplanationSchema }),
  z.strictObject({ status: z.literal("admitted"), operationId: uuid, workloadId: uuid, placementId: uuid, explanation: placementExplanationSchema }),
]).refine((value) => value.status === "queued" ? value.explanation.selected == null : value.explanation.selected != null, "Selection must match placement state");
export const hostActionRequestSchema = z.strictObject({
  action: z.enum(["pause", "resume", "drain", "maintenance", "retire", "revoke"]), expectedRevision: counter,
});
export const submitWorkloadRequestSchema = z.strictObject({
  poolId: text, profileId: uuid, targetId: uuid, expectedRevision: counter,
});
export const fleetEventSchema = z.strictObject({
  id: uuid, cursor, hostId: uuid.nullish(), placementId: uuid.nullish(),
  kind: z.enum(["host_enrolled", "host_approved", "host_changed", "backend_changed", "placement_queued", "placement_admitted", "command_changed", "log_gap", "adoption_changed", "upgrade_changed"]),
  observedAt: timestamp,
});
export const logChunkSchema = z.strictObject({
  streamId: uuid, placementId: uuid,
  text: z.string().refine((value) => new TextEncoder().encode(value).byteLength <= 262144, "Log chunk too large"),
  nextCursor: cursor, eof: z.boolean(), droppedBytes: counter.nullish(),
});
export const logMetadataSchema = z.strictObject({
  streamId: uuid, placementId: uuid, availability: z.enum(["pending", "available", "expired"]), startCursor: cursor.nullish(),
});
const errorDetails = z.discriminatedUnion("kind", [
  z.strictObject({ kind: z.literal("revision"), currentRevision: counter }),
  z.strictObject({ kind: z.literal("invalid_field"), field: text }),
  z.strictObject({ kind: z.literal("limit"), maximum: safeInteger }),
  z.strictObject({ kind: z.literal("retry"), retryAfterMs: safeInteger }),
  z.strictObject({ kind: z.literal("cursor_gap") }),
  z.strictObject({ kind: z.literal("readiness"), reason: rejectionReasonSchema }),
]);
export const fleetErrorSchema = z.strictObject({
  code: z.enum(["invalid_request", "unauthenticated", "forbidden", "revision_conflict", "idempotency_conflict", "enrollment_expired", "grant_expired", "cursor_expired", "cursor_gap", "payload_too_large", "rate_limited", "dependency_unavailable", "not_ready", "contract_range"]),
  message: text, requestId: uuid, details: errorDetails.nullish(),
});
export function pageSchema<T extends z.ZodType>(item: T) {
  return z.strictObject({ items: z.array(item).max(100), nextCursor: cursor.nullish() });
}

export type HostSummary = z.infer<typeof hostSummarySchema>;
export type BackendSummary = z.infer<typeof backendSummarySchema>;
export type OperationStatus = z.infer<typeof operationStatusSchema>;
export type PlacementExplanation = z.infer<typeof placementExplanationSchema>;
export type FleetEvent = z.infer<typeof fleetEventSchema>;
export type FleetError = z.infer<typeof fleetErrorSchema>;
