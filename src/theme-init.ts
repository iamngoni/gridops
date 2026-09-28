import { applyTheme, effectiveTheme, readThemePreference, systemPrefersDark } from "~/lib/theme";

applyTheme(effectiveTheme(readThemePreference(window.localStorage), systemPrefersDark()));
