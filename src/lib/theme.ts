export const THEME_STORAGE_KEY = "gridops-theme";

export type Theme = "light" | "dark";

export function resolveTheme(value: unknown): Theme {
  return value === "light" ? "light" : "dark";
}
