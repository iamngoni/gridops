export const THEME_STORAGE_KEY = "gridops-theme";

export type Theme = "light" | "dark";

export function resolveTheme(value: unknown): Theme {
  return value === "light" ? "light" : "dark";
}

export function readThemePreference(storage: Pick<Storage, "getItem">): Theme {
  try {
    return resolveTheme(storage.getItem(THEME_STORAGE_KEY));
  } catch {
    return "dark";
  }
}

export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  root.classList.toggle("dark", theme === "dark");
  root.dataset.theme = theme;

  document.querySelector('meta[name="color-scheme"]')?.setAttribute("content", theme);
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute("content", theme === "dark" ? "#0d120f" : "#f7faf8");
}
