import { describe, expect, it } from "vitest";

import { isNearLogEnd } from "../src/lib/log-follow";

describe("live log follow detection", () => {
  it("follows at the bottom of the viewport", () => {
    expect(isNearLogEnd({ clientHeight: 400, scrollHeight: 1_000, scrollTop: 600 })).toBe(true);
  });

  it("keeps following within the end threshold", () => {
    expect(isNearLogEnd({ clientHeight: 400, scrollHeight: 1_000, scrollTop: 552 })).toBe(true);
  });

  it("stops following when the operator scrolls up", () => {
    expect(isNearLogEnd({ clientHeight: 400, scrollHeight: 1_000, scrollTop: 551 })).toBe(false);
  });

  it("treats content shorter than the viewport as already at the end", () => {
    expect(isNearLogEnd({ clientHeight: 400, scrollHeight: 250, scrollTop: 0 })).toBe(true);
  });
});
