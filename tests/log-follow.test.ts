import { describe, expect, it } from "vitest";

import { advanceFollowedSteps, isNearLogEnd } from "../src/lib/log-follow";

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

describe("live log step following", () => {
  it("collapses the completed step and opens the newly running step", () => {
    const expanded = advanceFollowedSteps([
      { conclusion: "success", number: 1, status: "completed" },
      { conclusion: null, number: 2, status: "in_progress" },
      { conclusion: null, number: 3, status: "queued" },
    ], new Set([1]));

    expect([...expanded]).toEqual([2]);
  });

  it("opens the next queued step during the transition between steps", () => {
    const expanded = advanceFollowedSteps([
      { conclusion: "success", number: 1, status: "completed" },
      { conclusion: null, number: 2, status: "queued" },
    ], new Set([1]));

    expect([...expanded]).toEqual([2]);
  });

  it("keeps failed steps visible while advancing to cleanup", () => {
    const expanded = advanceFollowedSteps([
      { conclusion: "failure", number: 4, status: "completed" },
      { conclusion: null, number: 5, status: "in_progress" },
    ], new Set([4]));

    expect([...expanded]).toEqual([4, 5]);
  });
});
