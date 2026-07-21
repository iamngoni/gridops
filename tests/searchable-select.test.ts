import { describe, expect, it } from "vitest";

import { filterSearchableOptions } from "~/components/ui/searchable-select";

const options = [
  { value: 1, label: "iamngoni/gridops", description: "Private repository", keywords: ["runner control"] },
  { value: 2, label: "iamngoni/website", description: "Public repository", keywords: ["portfolio"] },
];

describe("searchable select filtering", () => {
  it("returns all options for an empty search", () => {
    expect(filterSearchableOptions(options, "  ")).toEqual(options);
  });

  it("matches labels without case sensitivity", () => {
    expect(filterSearchableOptions(options, "GRIDOPS")).toEqual([options[0]]);
  });

  it("matches descriptions and additional keywords", () => {
    expect(filterSearchableOptions(options, "public")).toEqual([options[1]]);
    expect(filterSearchableOptions(options, "runner control")).toEqual([options[0]]);
  });

  it("returns an empty list when nothing matches", () => {
    expect(filterSearchableOptions(options, "missing")).toEqual([]);
  });
});
