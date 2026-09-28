export const THEME_STORAGE_KEY = "gridops-theme";

/** The palette on screen. */
export type Theme = "light" | "dark";
/** What the person chose; "system" follows the operating system. */
export type ThemePreference = Theme | "system";

export const SYSTEM_DARK_QUERY = "(prefers-color-scheme: dark)";

export function resolveTheme(value: unknown): Theme {
  return value === "light" ? "light" : "dark";
}

export function resolvePreference(value: unknown): ThemePreference {
  return value === "light" || value === "system" ? value : "dark";
}

export function readThemePreference(storage: Pick<Storage, "getItem">): ThemePreference {
  try {
    return resolvePreference(storage.getItem(THEME_STORAGE_KEY));
  } catch {
    return "dark";
  }
}

export function effectiveTheme(preference: ThemePreference, systemPrefersDark: boolean): Theme {
  if (preference === "system") return systemPrefersDark ? "dark" : "light";
  return preference;
}

export function systemPrefersDark() {
  return typeof window.matchMedia === "function" && window.matchMedia(SYSTEM_DARK_QUERY).matches;
}

export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  // Swap the palette in one frame: per-element color transitions would
  // otherwise ripple across the page as each hover style catches up.
  root.classList.add("theme-switching");
  root.classList.toggle("dark", theme === "dark");
  root.dataset.theme = theme;

  document.querySelector('meta[name="color-scheme"]')?.setAttribute("content", theme);
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute("content", theme === "dark" ? "#08090a" : "#f4f4f6");
  requestAnimationFrame(() => requestAnimationFrame(() => root.classList.remove("theme-switching")));
}
