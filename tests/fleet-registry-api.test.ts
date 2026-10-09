/** Verifies transport policy and parsing without persisting one-time enrollment codes. */
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  FleetRequestError, approveHost, getEnrollment, getFleetOperation, issueEnrollment, recoverHost,
  revokeEnrollment, rotateHostCredential,
} from "../src/features/fleet/registry-api";

const id = "00000001-0000-4000-8000-000000000001";
const stamp = "2026-10-09T00:00:00.000Z";
const created = { status: "created", enrollmentId: id, code: "A".repeat(43), expiresAt: stamp };
const metadata = { enrollmentId: id, expiresAt: stamp, consumedAt: null, revokedAt: null, consumedHostId: null };
const mockFetch = (body: unknown, status = 200) => {
  const fetcher = vi.fn().mockResolvedValue(Response.json(body, { status }));
  vi.stubGlobal("fetch", fetcher);
  return fetcher;
};
const firstCall = (fetcher: ReturnType<typeof vi.fn>) => {
  const call = fetcher.mock.calls[0];
  if (!call) throw new Error("Expected a fleet request");
  return call;
};

afterEach(() => vi.unstubAllGlobals());

describe("fleet registry browser transport", () => {
  it("approves the reviewed inventory once without granting execution policy", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    vi.stubGlobal("fetch", fetcher);
    const input = { expectedRevision: 3, expectedInventoryRevision: 8,
      expectedInventoryDigest: "A".repeat(43), targetIds: [id] };
    await expect(approveHost(id, input)).resolves.toBeUndefined();
    const [path, options] = firstCall(fetcher);
    expect(path).toBe(`/api/v1/fleet/hosts/${id}/approve`);
    expect(JSON.parse(options.body)).toEqual(input);
    expect(options).toMatchObject({ method: "POST", cache: "no-store", credentials: "same-origin" });
    expect(() => approveHost(id, { ...input, expectedInventoryRevision: 0 })).toThrow();
    expect(fetcher).toHaveBeenCalledTimes(1);
    fetcher.mockResolvedValue(Response.json({ code: "revision_conflict", message: "Revision changed",
      requestId: id, details: null }, { status: 409 }));
    await expect(approveHost(id, input)).rejects.toMatchObject({ code: "revision_conflict" });
    expect(fetcher).toHaveBeenCalledTimes(2);
  });

  it("issues once with a checked retry key and no caching or automatic retry", async () => {
    const fetcher = mockFetch(created, 201);
    const controller = new AbortController();
    await expect(issueEnrollment({ targetIds: [] }, "issue-once", controller.signal)).resolves.toEqual(created);
    expect(fetcher).toHaveBeenCalledTimes(1);
    const [path, options] = firstCall(fetcher);
    expect(path).toBe("/api/v1/fleet/enrollments");
    expect(options).toMatchObject({ method: "POST", cache: "no-store", credentials: "same-origin", redirect: "error", signal: controller.signal });
    expect(options.headers.get("Idempotency-Key")).toBe("issue-once");
    expect(options.headers.has("Authorization")).toBe(false);
    expect(JSON.parse(options.body)).toEqual({ targetIds: [] });
    fetcher.mockRejectedValue(new TypeError("Network unavailable"));
    await expect(issueEnrollment({ targetIds: [] }, "issue-once")).rejects.toThrow("Network unavailable");
    expect(fetcher).toHaveBeenCalledTimes(2);
  });

  it("parses metadata replay and status without recovering a code", async () => {
    mockFetch({ status: "replay", enrollment: metadata });
    await expect(issueEnrollment({ targetIds: [] }, "same-key")).resolves.toEqual({ status: "replay", enrollment: metadata });
    const fetcher = mockFetch(metadata);
    await expect(getEnrollment(id)).resolves.toEqual(metadata);
    expect(firstCall(fetcher)[1].method).toBe("GET");
    expect(firstCall(fetcher)[1].headers.has("Idempotency-Key")).toBe(false);
    mockFetch({ ...metadata, credential: "secret-sentinel" });
    await expect(getEnrollment(id)).rejects.toMatchObject({ code: "invalid_response" });
  });

  it("binds recovery and rotation to a parsed host and expected counter", async () => {
    let fetcher = mockFetch({ ...created, hostId: id }, 201);
    await recoverHost(id, 2, "recover");
    expect(firstCall(fetcher)[0]).toBe(`/api/v1/fleet/hosts/${id}/recover`);
    expect(JSON.parse(firstCall(fetcher)[1].body)).toEqual({ expectedRevision: 2 });
    const accepted = { status: "accepted", operationId: id, statusUrl: `/api/v1/operations/${id}` };
    fetcher = mockFetch(accepted, 202);
    await expect(rotateHostCredential(id, 3, "rotate")).resolves.toEqual(accepted);
    expect(JSON.parse(firstCall(fetcher)[1].body)).toEqual({ expectedGeneration: 3 });
    const operation = { operationId: id, status: "queued", reason: null, updatedAt: stamp };
    fetcher = mockFetch(operation);
    await expect(getFleetOperation(id)).resolves.toEqual(operation);
    expect(firstCall(fetcher)[0]).toBe(`/api/v1/operations/${id}`);
  });

  it("accepts empty revoke success and rejects untrusted path/key input before fetching", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    vi.stubGlobal("fetch", fetcher);
    await expect(revokeEnrollment(id)).resolves.toBeUndefined();
    expect(firstCall(fetcher)[0]).toBe(`/api/v1/fleet/enrollments/${id}/revoke`);
    expect(() => getEnrollment("../../agent/enroll")).toThrow();
    expect(() => recoverHost(id, 0, "key")).toThrow();
    await expect(issueEnrollment({ targetIds: [] }, "bad key")).rejects.toThrow();
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("preserves structured failures while discarding malformed/secret-bearing bodies", async () => {
    const failure = { code: "revision_conflict", message: "Revision changed", requestId: id, details: null };
    mockFetch(failure, 409);
    await expect(getEnrollment(id)).rejects.toMatchObject({ status: 409, code: "revision_conflict", requestId: id });
    mockFetch({ ...failure, credential: "secret-sentinel" }, 503);
    const rejected = await getEnrollment(id).catch((error: unknown) => error);
    expect(rejected).toBeInstanceOf(FleetRequestError);
    expect(JSON.stringify(rejected)).not.toContain("secret-sentinel");
    expect(String(rejected)).not.toContain("secret-sentinel");
    mockFetch({ ...created, credential: "secret-sentinel" }, 201);
    await expect(issueEnrollment({ targetIds: [] }, "key")).rejects.toMatchObject({ code: "invalid_response" });
  });
});
