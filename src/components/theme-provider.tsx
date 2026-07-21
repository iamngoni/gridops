import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";

import { applyTheme, readThemePreference, resolveTheme, THEME_STORAGE_KEY, type Theme } from "~/lib/theme";

type ThemeContextValue = {
  theme: Theme;
  toggleTheme: () => void;
};

const ThemeContext = createContext<ThemeContextValue | null>(null);

function initialTheme(): Theme {
  if (document.documentElement.dataset.theme) {
    return resolveTheme(document.documentElement.dataset.theme);
  }
  return readThemePreference(window.localStorage);
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [theme, setTheme] = useState<Theme>(initialTheme);

  useEffect(() => {
    applyTheme(theme);
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, theme);
    } catch {
      // Theme switching still works when storage is unavailable.
    }
  }, [theme]);

  const toggleTheme = useCallback(() => {
    setTheme((current) => current === "dark" ? "light" : "dark");
  }, []);
  const value = useMemo(() => ({ theme, toggleTheme }), [theme, toggleTheme]);

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme() {
  const context = useContext(ThemeContext);
  if (!context) throw new Error("useTheme must be used within ThemeProvider.");
  return context;
}
