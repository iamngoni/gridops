import { describe, expect, it } from "vitest";

import { pageNumbers, parsePage, validatePageSearch } from "~/lib/pagination";

describe("list pagination", () => {
  it("normalizes page search parameters", () => {
    expect(parsePage(undefined)).toBe(1);
    expect(parsePage("4")).toBe(4);
    expect(parsePage(-2)).toBe(1);
    expect(parsePage("invalid")).toBe(1);
    expect(validatePageSearch({})).toEqual({});
    expect(validatePageSearch({ page: "3" })).toEqual({ page: 3 });
  });

  it("keeps a compact five-page window around the current page", () => {
    expect(pageNumbers(1, 10)).toEqual([1, 2, 3, 4, 5]);
    expect(pageNumbers(6, 10)).toEqual([4, 5, 6, 7, 8]);
    expect(pageNumbers(10, 10)).toEqual([6, 7, 8, 9, 10]);
    expect(pageNumbers(1, 2)).toEqual([1, 2]);
  });

  it("keeps list summaries separate from the non-wrapping control row", async () => {
    const source = await import("node:fs/promises").then((fs) => fs.readFile("src/components/list-pagination.tsx", "utf8"));
    expect(source).toContain("flex max-w-full flex-nowrap");
    expect(source).not.toContain("sm:flex-row sm:items-center sm:justify-between");
  });
});
