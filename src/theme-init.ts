import { applyTheme, readThemePreference } from "~/lib/theme";

applyTheme(readThemePreference(window.localStorage));
