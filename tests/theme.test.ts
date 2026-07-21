import { describe, expect, it } from "vitest";

import { resolveTheme, THEME_STORAGE_KEY } from "~/lib/theme";

describe("theme preference", () => {
  it("restores an explicit light preference", () => {
    expect(resolveTheme("light")).toBe("light");
  });

  it("keeps dark mode as the safe default", () => {
    expect(resolveTheme("dark")).toBe("dark");
    expect(resolveTheme(undefined)).toBe("dark");
    expect(resolveTheme("system")).toBe("dark");
  });

  it("uses a GridOps-specific storage key", () => {
    expect(THEME_STORAGE_KEY).toBe("gridops-theme");
  });
});
