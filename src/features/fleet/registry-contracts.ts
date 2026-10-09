/**
 * Browser contracts for one-time fleet enrollment and credential operations.
 * Created receipts carry a code once; replay/status schemas reject secret fields.
 */
import { z } from "zod";
import { fleetCounterSchema, fleetIdSchema, fleetTimestampSchema } from "./contracts";

const targetIds = z.array(fleetIdSchema).max(64)
  .refine((ids) => new Set(ids).size === ids.length, "Duplicate target");
// A 32-byte canonical, unpadded base64url value has two zero trailing bits.
const enrollmentCode = z.string().regex(/^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/);

export const enrollmentRequestSchema = z.strictObject({
  targetIds,
  intendedHostId: z.null().optional(),
  lifetimeMinutes: fleetCounterSchema.max(60).nullish(),
});
export const enrollmentStatusSchema = z.strictObject({
  enrollmentId: fleetIdSchema,
  expiresAt: fleetTimestampSchema,
  consumedAt: fleetTimestampSchema.nullable(),
  revokedAt: fleetTimestampSchema.nullable(),
  consumedHostId: fleetIdSchema.nullable(),
}).refine((value) => (value.consumedAt === null) === (value.consumedHostId === null),
  "Consumed enrollment must identify its host");

const metadataReplay = z.strictObject({
  status: z.literal("replay"),
  enrollment: enrollmentStatusSchema,
});
export const enrollmentIssueSchema = z.discriminatedUnion("status", [
  z.strictObject({
    status: z.literal("created"), enrollmentId: fleetIdSchema,
    code: enrollmentCode, expiresAt: fleetTimestampSchema,
  }),
  metadataReplay,
]);
export const recoveryRequestSchema = z.strictObject({ expectedRevision: fleetCounterSchema });
export const recoveryIssueSchema = z.discriminatedUnion("status", [
  z.strictObject({
    status: z.literal("created"), enrollmentId: fleetIdSchema, hostId: fleetIdSchema,
    code: enrollmentCode, expiresAt: fleetTimestampSchema,
  }),
  metadataReplay,
]);
export const rotationRequestSchema = z.strictObject({ expectedGeneration: fleetCounterSchema });
export const approvalRequestSchema = z.strictObject({
  expectedRevision: fleetCounterSchema,
  expectedInventoryRevision: fleetCounterSchema,
  expectedInventoryDigest: z.string().regex(/^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/),
  targetIds,
});
export const fleetIdempotencyKeySchema = z.string().min(1).max(128).regex(/^[\x21-\x7e]+$/);

export type EnrollmentRequest = z.input<typeof enrollmentRequestSchema>;
export type ApprovalRequest = z.input<typeof approvalRequestSchema>;
export type EnrollmentStatus = z.infer<typeof enrollmentStatusSchema>;
export type EnrollmentIssue = z.infer<typeof enrollmentIssueSchema>;
export type RecoveryIssue = z.infer<typeof recoveryIssueSchema>;
