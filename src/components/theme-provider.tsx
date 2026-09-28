import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";

import {
  applyTheme,
  effectiveTheme,
  readThemePreference,
  SYSTEM_DARK_QUERY,
  systemPrefersDark,
  THEME_STORAGE_KEY,
  type Theme,
  type ThemePreference,
} from "~/lib/theme";

type ThemeContextValue = {
  /** The palette on screen. */
  theme: Theme;
  preference: ThemePreference;
  setPreference: (preference: ThemePreference) => void;
  toggleTheme: () => void;
};

const ThemeContext = createContext<ThemeContextValue | null>(null);

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [preference, setPreference] = useState<ThemePreference>(() => readThemePreference(window.localStorage));
  const [systemDark, setSystemDark] = useState(systemPrefersDark);
  const theme = effectiveTheme(preference, systemDark);

  useEffect(() => {
    if (preference !== "system" || typeof window.matchMedia !== "function") return undefined;
    const query = window.matchMedia(SYSTEM_DARK_QUERY);
    const follow = () => setSystemDark(query.matches);
    follow();
    query.addEventListener("change", follow);
    return () => query.removeEventListener("change", follow);
  }, [preference]);

  useEffect(() => {
    // The boot script already painted the initial theme; only react to changes.
    if (document.documentElement.dataset.theme !== theme) applyTheme(theme);
  }, [theme]);

  useEffect(() => {
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, preference);
    } catch {
      // Theme switching still works when storage is unavailable.
    }
  }, [preference]);

  const toggleTheme = useCallback(() => {
    setPreference(theme === "dark" ? "light" : "dark");
  }, [theme]);
  const value = useMemo(() => ({ theme, preference, setPreference, toggleTheme }), [theme, preference, toggleTheme]);

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme() {
  const context = useContext(ThemeContext);
  if (!context) throw new Error("useTheme must be used within ThemeProvider.");
  return context;
}
