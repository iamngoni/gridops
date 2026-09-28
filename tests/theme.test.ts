import { describe, expect, it } from "vitest";

import { effectiveTheme, readThemePreference, resolvePreference, resolveTheme, THEME_STORAGE_KEY } from "~/lib/theme";

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

  it("falls back safely when browser storage is unavailable", () => {
    expect(readThemePreference({ getItem: () => { throw new Error("blocked"); } })).toBe("dark");
  });

  it("remembers a system preference and resolves it from the operating system", () => {
    expect(resolvePreference("system")).toBe("system");
    expect(resolvePreference("sepia")).toBe("dark");
    expect(readThemePreference({ getItem: () => "system" })).toBe("system");
    expect(effectiveTheme("system", true)).toBe("dark");
    expect(effectiveTheme("system", false)).toBe("light");
    expect(effectiveTheme("light", true)).toBe("light");
  });
});
