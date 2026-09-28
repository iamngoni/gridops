import { Moon, Sun } from "lucide-react";

import { useTheme } from "./theme-provider";
import { Button } from "./ui/button";
import { Tooltip } from "./ui/tooltip";

export function ThemeToggle({ className }: { className?: string }) {
  const { theme, toggleTheme } = useTheme();
  const target = theme === "dark" ? "light" : "dark";

  return (
    <Tooltip content={`Switch to ${target} theme`}>
      <Button aria-label={`Switch to ${target} theme`} className={className} onClick={toggleTheme} size="icon-sm" variant="ghost">
        {theme === "dark" ? <Sun /> : <Moon />}
      </Button>
    </Tooltip>
  );
}
