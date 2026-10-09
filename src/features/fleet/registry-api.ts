/**
 * Same-origin browser calls for fleet enrollment and credential operations.
 * Mutations never retry automatically: a lost one-time code requires status/recovery.
 */
import { z } from "zod";
import {
  fleetErrorSchema, fleetIdSchema, operationAcceptedSchema, operationStatusSchema,
  type FleetError,
} from "./contracts";
import {
  approvalRequestSchema, enrollmentIssueSchema, enrollmentRequestSchema, enrollmentStatusSchema,
  fleetIdempotencyKeySchema, recoveryIssueSchema, recoveryRequestSchema, rotationRequestSchema,
  type ApprovalRequest, type EnrollmentRequest,
} from "./registry-contracts";

/** Safe structured server failure; raw response bodies and secrets are never retained. */
export class FleetRequestError extends Error {
  readonly status: number;
  readonly code: FleetError["code"] | "invalid_response";
  readonly requestId: string | undefined;

  constructor(status: number, error?: FleetError) {
    super(error?.message ?? "Fleet returned an invalid response.");
    this.name = "FleetRequestError";
    this.status = status;
    this.code = error?.code ?? "invalid_response";
    this.requestId = error?.requestId;
  }
}

type Options = { signal?: AbortSignal; body?: unknown; key?: string; method?: "GET" | "POST" };

async function request<S extends z.ZodType>(path: string, schema: S, options: Options = {}): Promise<z.output<S>> {
  const headers = new Headers({ Accept: "application/json" });
  if (options.body !== undefined) headers.set("Content-Type", "application/json");
  if (options.key !== undefined) headers.set("Idempotency-Key", fleetIdempotencyKeySchema.parse(options.key));
  const response = await fetch(path, {
    method: options.method ?? "GET", credentials: "same-origin", cache: "no-store",
    redirect: "error", headers, signal: options.signal,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });
  if (!response.ok) {
    const parsed = fleetErrorSchema.safeParse(await response.json().catch(() => null));
    throw new FleetRequestError(response.status, parsed.success ? parsed.data : undefined);
  }
  // Approval and revoke return an empty 200. Other responses parse their specific
  // schema; do not include Zod's raw data in errors for one-time receipt paths.
  const body = schema instanceof z.ZodUndefined ? undefined : await response.json().catch(() => null);
  const parsed = schema.safeParse(body);
  if (!parsed.success) throw new FleetRequestError(response.status);
  return parsed.data;
}

const emptyResponse = z.undefined();
const hostPath = (id: string) => `/api/v1/fleet/hosts/${fleetIdSchema.parse(id)}`;
const enrollmentPath = (id: string) => `/api/v1/fleet/enrollments/${fleetIdSchema.parse(id)}`;

export const issueEnrollment = (input: EnrollmentRequest, key: string, signal?: AbortSignal) =>
  request("/api/v1/fleet/enrollments", enrollmentIssueSchema, {
    method: "POST", body: enrollmentRequestSchema.parse(input), key, signal,
  });
export const getEnrollment = (id: string, signal?: AbortSignal) =>
  request(enrollmentPath(id), enrollmentStatusSchema, { signal });
export const revokeEnrollment = (id: string, signal?: AbortSignal) =>
  request(`${enrollmentPath(id)}/revoke`, emptyResponse, { method: "POST", signal });
export const approveHost = (id: string, input: ApprovalRequest, signal?: AbortSignal) =>
  request(`${hostPath(id)}/approve`, emptyResponse, {
    method: "POST", body: approvalRequestSchema.parse(input), signal,
  });
export const recoverHost = (id: string, revision: number, key: string, signal?: AbortSignal) =>
  request(`${hostPath(id)}/recover`, recoveryIssueSchema, {
    method: "POST", body: recoveryRequestSchema.parse({ expectedRevision: revision }), key, signal,
  });
export const rotateHostCredential = (id: string, generation: number, key: string, signal?: AbortSignal) =>
  request(`${hostPath(id)}/credentials/rotate`, operationAcceptedSchema, {
    method: "POST", body: rotationRequestSchema.parse({ expectedGeneration: generation }), key, signal,
  });
export const getFleetOperation = (id: string, signal?: AbortSignal) =>
  request(`/api/v1/operations/${fleetIdSchema.parse(id)}`, operationStatusSchema, { signal });
