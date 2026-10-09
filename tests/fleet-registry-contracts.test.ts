/** Checks one-time receipt boundaries before enrollment UI can retain API data. */
import { describe, expect, it } from "vitest";
import {
  approvalRequestSchema, enrollmentIssueSchema, enrollmentRequestSchema, enrollmentStatusSchema,
  fleetIdempotencyKeySchema, recoveryIssueSchema, recoveryRequestSchema, rotationRequestSchema,
} from "../src/features/fleet/registry-contracts";

const id = "00000001-0000-4000-8000-000000000001";
const stamp = "2026-10-09T00:00:00.000Z";
const metadata = {
  enrollmentId: id, expiresAt: stamp, consumedAt: null, revokedAt: null, consumedHostId: null,
};
const created = { status: "created", enrollmentId: id, code: "A".repeat(43), expiresAt: stamp };

describe("registry browser contracts", () => {
  it("binds approval to the exact inventory revision, digest and bounded unique scope", () => {
    const approval = { expectedRevision: 1, expectedInventoryRevision: 2,
      expectedInventoryDigest: "A".repeat(43), targetIds: [id] };
    expect(approvalRequestSchema.safeParse(approval).success).toBe(true);
    for (const delta of [
      { expectedRevision: 0 }, { expectedInventoryRevision: 0 },
      { expectedInventoryDigest: `${"A".repeat(42)}B` }, { expectedInventoryDigest: null },
      { targetIds: [id, id.toUpperCase()] }, { allowNative: true }, { active: true },
    ]) expect(approvalRequestSchema.safeParse({ ...approval, ...delta }).success).toBe(false);
  });

  it("keeps one-time codes out of replay and status projections", () => {
    expect(enrollmentIssueSchema.safeParse(created).success).toBe(true);
    const replay = { status: "replay", enrollment: metadata };
    expect(enrollmentIssueSchema.safeParse(replay).success).toBe(true);
    expect(enrollmentIssueSchema.safeParse({ ...replay, code: created.code }).success).toBe(false);
    expect(enrollmentStatusSchema.safeParse({ ...metadata, credential: created.code }).success).toBe(false);
    expect(enrollmentIssueSchema.safeParse({ ...created, credential: created.code }).success).toBe(false);
    expect(enrollmentIssueSchema.safeParse({ ...created, code: `${"A".repeat(42)}B` }).success).toBe(false);
  });

  it("requires the original host on a recovery receipt and consumed metadata", () => {
    expect(recoveryIssueSchema.safeParse(created).success).toBe(false);
    expect(recoveryIssueSchema.safeParse({ ...created, hostId: id }).success).toBe(true);
    expect(enrollmentStatusSchema.safeParse({ ...metadata, consumedAt: stamp }).success).toBe(false);
    expect(enrollmentStatusSchema.safeParse({ ...metadata, consumedHostId: id }).success).toBe(false);
    expect(enrollmentStatusSchema.safeParse({ ...metadata, consumedAt: stamp, consumedHostId: id }).success).toBe(true);
    expect(recoveryIssueSchema.safeParse({ status: "replay", enrollment: metadata }).success).toBe(true);
  });

  it("bounds exact scope and prevents accidental recovery through ordinary issuance", () => {
    expect(enrollmentRequestSchema.safeParse({ targetIds: [] }).success).toBe(true);
    expect(enrollmentRequestSchema.safeParse({ targetIds: [id], lifetimeMinutes: 60 }).success).toBe(true);
    expect(enrollmentRequestSchema.safeParse({ targetIds: [id, id.toUpperCase()] }).success).toBe(false);
    expect(enrollmentRequestSchema.safeParse({ targetIds: [id], intendedHostId: id }).success).toBe(false);
    expect(enrollmentRequestSchema.safeParse({ targetIds: [], lifetimeMinutes: 61 }).success).toBe(false);
    expect(enrollmentRequestSchema.safeParse({ targetIds: [], lifetimeMinutes: 0 }).success).toBe(false);
    expect(enrollmentRequestSchema.safeParse({ targetIds: [], hostOs: "windows" }).success).toBe(false);
    expect(enrollmentRequestSchema.safeParse({ targetIds: Array(65).fill(id) }).success).toBe(false);
  });

  it("requires exact revisions and generations and printable ASCII retry keys", () => {
    expect(recoveryRequestSchema.safeParse({ expectedRevision: 1 }).success).toBe(true);
    expect(rotationRequestSchema.safeParse({ expectedGeneration: 1 }).success).toBe(true);
    for (const value of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, "1", null]) {
      expect(recoveryRequestSchema.safeParse({ expectedRevision: value }).success).toBe(false);
      expect(rotationRequestSchema.safeParse({ expectedGeneration: value }).success).toBe(false);
    }
    expect(rotationRequestSchema.safeParse({ expectedGeneration: 1, nextSecret: created.code }).success).toBe(false);
    for (const key of ["", " ", "secret\n", "nonascii-é", "a".repeat(129)]) {
      expect(fleetIdempotencyKeySchema.safeParse(key).success).toBe(false);
    }
    expect(fleetIdempotencyKeySchema.safeParse("a".repeat(128)).success).toBe(true);
  });
});
