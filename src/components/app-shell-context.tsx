import { Menu } from "lucide-react";
import { createContext, useContext } from "react";

import { Button } from "./ui/button";

type ShellContextValue = {
  openNavigation: () => void;
  openCommandMenu: () => void;
};

export const ShellContext = createContext<ShellContextValue | null>(null);

export function useShell() {
  return useContext(ShellContext);
}

/** Opens the sidebar drawer on narrow screens; hidden once the sidebar is docked. */
export function MobileNavButton() {
  const shell = useShell();
  if (!shell) return null;
  return (
    <Button aria-label="Open navigation" className="-ml-1 lg:hidden" onClick={shell.openNavigation} size="icon-sm" variant="ghost">
      <Menu />
    </Button>
  );
}
