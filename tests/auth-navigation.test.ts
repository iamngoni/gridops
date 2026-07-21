import { describe, expect, it } from "vitest";

import { safeReturnTo } from "~/lib/auth-navigation";

describe("authentication navigation", () => {
  it("preserves local application destinations", () => {
    expect(safeReturnTo("/repositories?q=gridops&page=2")).toBe("/repositories?q=gridops&page=2");
    expect(safeReturnTo("/runner-pools/pool-1#capacity")).toBe("/runner-pools/pool-1#capacity");
  });

  it("rejects external and protocol-relative redirects", () => {
    expect(safeReturnTo("https://example.com")).toBe("/");
    expect(safeReturnTo("//example.com/path")).toBe("/");
  });

  it("rejects login, API, and authentication endpoints as destinations", () => {
    expect(safeReturnTo("/login?returnTo=/login")).toBe("/");
    expect(safeReturnTo("/auth/github")).toBe("/");
    expect(safeReturnTo("/api/v1/auth/me")).toBe("/");
  });

  it("falls back safely for missing and malformed values", () => {
    expect(safeReturnTo(undefined)).toBe("/");
    expect(safeReturnTo(42)).toBe("/");
    expect(safeReturnTo("repositories")).toBe("/");
  });
});
