import { Moon, Sun } from "lucide-react";

import { useTheme } from "./theme-provider";
import { Button } from "./ui/button";
import { cn } from "~/lib/utils";

export function ThemeToggle({ className }: { className?: string }) {
  const { theme, toggleTheme } = useTheme();
  const target = theme === "dark" ? "light" : "dark";

  return (
    <Button
      aria-label={`Switch to ${target} mode`}
      className={cn("relative", className)}
      onClick={toggleTheme}
      size="icon"
      title={`Switch to ${target} mode`}
      variant="ghost"
    >
      {theme === "dark" ? <Sun /> : <Moon />}
    </Button>
  );
}
