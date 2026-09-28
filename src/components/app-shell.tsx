import * as DialogPrimitive from "@radix-ui/react-dialog";
import { Link, getRouteApi, useNavigate, useRouterState } from "@tanstack/react-router";
import { ChevronDown, ChevronRight, Command as CommandIcon, LogOut, Moon, Search, Settings, SquarePen, Sun } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { ShellContext } from "./app-shell-context";
import { CommandMenu } from "./command-menu";
import { GridMark } from "./grid-logo";
import { StatusDot } from "./status-icon";
import { useTheme } from "./theme-provider";
import { Avatar } from "./ui/avatar";
import { Button, buttonVariants } from "./ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { Tooltip } from "./ui/tooltip";
import { api, type Viewer } from "~/lib/api";
import { allNavItems, isActivePath, navigation, settingsNavItem, type NavGroup, type NavItem } from "~/lib/navigation";
import { useGlobalHotkeys } from "~/lib/use-hotkeys";
import { cn } from "~/lib/utils";

const COLLAPSED_KEY = "gridops-sidebar-collapsed";

function readCollapsed(): string[] {
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(COLLAPSED_KEY) ?? "[]");
    return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
  } catch {
    return [];
  }
}

export function AppShell({ children, sidebar }: { children: React.ReactNode; sidebar?: React.ReactNode }) {
  const [mobileOpen, setMobileOpen] = useState(false);
  const [commandOpen, setCommandOpen] = useState(false);
  const navigate = useNavigate();
  const viewer = getRouteApi("__root__").useLoaderData();

  // The drawer is modal, so it must not outlive the layout it belongs to: once
  // the docked sidebar takes over, a hidden open drawer would still block clicks.
  useEffect(() => {
    if (!mobileOpen || typeof window.matchMedia !== "function") return undefined;
    const docked = window.matchMedia("(min-width: 64rem)");
    const close = () => { if (docked.matches) setMobileOpen(false); };
    docked.addEventListener("change", close);
    return () => docked.removeEventListener("change", close);
  }, [mobileOpen]);

  const openCommandMenu = useCallback(() => setCommandOpen(true), []);
  const openNavigation = useCallback(() => setMobileOpen(true), []);
  const shell = useMemo(() => ({ openCommandMenu, openNavigation }), [openCommandMenu, openNavigation]);

  useGlobalHotkeys({
    onCommandMenu: () => setCommandOpen((open) => !open),
    onGo: (key) => {
      const item = allNavItems.find((candidate) => candidate.shortcut === key);
      if (!item) return false;
      void navigate({ to: item.to });
      return true;
    },
    onKey: (key) => {
      if (key === "c" || key === "C") {
        void navigate({ to: "/runner-pools/new" });
        return true;
      }
      if (key === "?") {
        setCommandOpen(true);
        return true;
      }
      if (key === "j" || key === "k") return moveRowFocus(key === "j" ? 1 : -1);
      return false;
    },
  });

  return (
    <ShellContext.Provider value={shell}>
      <div className="fixed inset-0 flex overflow-hidden bg-background text-foreground">
        <a className="sr-only z-[100] rounded-md bg-popover px-3 py-2 text-sm font-medium shadow-popover focus:not-sr-only focus:fixed focus:left-3 focus:top-3" href="#main">Skip to content</a>
        <div className="hidden lg:flex">{sidebar ?? <Sidebar onSearch={openCommandMenu} viewer={viewer} />}</div>
        {/* A modal drawer on narrow screens: Escape and the backdrop close it, and focus stays inside while open. */}
        <DialogPrimitive.Root onOpenChange={setMobileOpen} open={mobileOpen}>
          <DialogPrimitive.Portal>
            <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-black/50 lg:hidden data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:duration-200 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:duration-150" />
            <DialogPrimitive.Content
              aria-describedby={undefined}
              className="ease-drawer fixed inset-y-0 left-0 z-50 flex border-r border-border bg-background shadow-popover outline-none lg:hidden data-[state=open]:animate-in data-[state=open]:slide-in-from-left-8 data-[state=open]:fade-in-0 data-[state=open]:duration-300 data-[state=closed]:animate-out data-[state=closed]:slide-out-to-left-8 data-[state=closed]:fade-out-0 data-[state=closed]:duration-200"
            >
              <DialogPrimitive.Title className="sr-only">Navigation</DialogPrimitive.Title>
              {sidebar ? <div onClickCapture={(event) => { if ((event.target as HTMLElement).closest("a")) setMobileOpen(false); }}>{sidebar}</div> : <Sidebar onNavigate={() => setMobileOpen(false)} onSearch={() => { setMobileOpen(false); openCommandMenu(); }} viewer={viewer} />}
            </DialogPrimitive.Content>
          </DialogPrimitive.Portal>
        </DialogPrimitive.Root>
        <main className="flex min-w-0 flex-1 flex-col overflow-hidden bg-panel outline-none lg:my-2 lg:mr-2 lg:rounded-lg lg:border lg:border-border lg:shadow-panel" id="main" tabIndex={-1}>
          {children}
        </main>
      </div>
      <CommandMenu onOpenChange={setCommandOpen} open={commandOpen} signedIn={Boolean(viewer)} />
    </ShellContext.Provider>
  );
}

/** Linear's J/K list navigation: focus the next or previous row link; Enter opens it. */
function moveRowFocus(direction: 1 | -1) {
  const rows = [...document.querySelectorAll<HTMLElement>("main [data-list-row]")].filter((row) => !row.hasAttribute("disabled"));
  if (!rows.length) return false;
  const current = rows.indexOf(document.activeElement as HTMLElement);
  const next = current === -1 ? (direction === 1 ? 0 : rows.length - 1) : Math.min(rows.length - 1, Math.max(0, current + direction));
  rows[next]?.focus({ preventScroll: true });
  rows[next]?.scrollIntoView({ block: "nearest" });
  return true;
}

function Sidebar({ viewer, onSearch, onNavigate }: { viewer: Viewer | null; onSearch: () => void; onNavigate?: () => void }) {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const [collapsed, setCollapsed] = useState<string[]>(readCollapsed);

  function toggleGroup(label: string) {
    setCollapsed((current) => {
      const next = current.includes(label) ? current.filter((item) => item !== label) : [...current, label];
      try {
        window.localStorage.setItem(COLLAPSED_KEY, JSON.stringify(next));
      } catch {
        // Collapsing still works for this visit when storage is unavailable.
      }
      return next;
    });
  }

  return (
    <aside className="flex h-full w-[244px] shrink-0 flex-col gap-3 px-2 py-2">
      <div className="flex h-9 items-center gap-1">
        <WorkspaceMenu viewer={viewer} />
        <Tooltip content="Search and commands" shortcut={["⌘", "K"]}>
          <Button aria-label="Search and commands" onClick={onSearch} size="icon-sm" variant="ghost"><Search /></Button>
        </Tooltip>
        <Tooltip content="Create runner pool" shortcut={["C"]}>
          <Link aria-label="Create runner pool" className={cn(buttonVariants({ size: "icon-sm", variant: "outline" }), "rounded-md")} onClick={onNavigate} to="/runner-pools/new"><SquarePen /></Link>
        </Tooltip>
      </div>

      <nav aria-label="Main navigation" className="-mx-1 min-h-0 flex-1 space-y-4 overflow-y-auto px-1">
        {navigation.map((group, index) => (
          <NavSection
            collapsed={group.label ? collapsed.includes(group.label) : false}
            group={group}
            key={group.label ?? `group-${index}`}
            onNavigate={onNavigate}
            onToggle={group.label ? () => toggleGroup(group.label as string) : undefined}
            pathname={pathname}
            viewer={viewer}
          />
        ))}
      </nav>

      <div className="space-y-1">
        <NavLinkItem active={isActivePath(pathname, settingsNavItem.to)} item={settingsNavItem} onNavigate={onNavigate} />
        <div className="flex h-7 items-center gap-2 px-2 text-2xs text-faint">
          <StatusDot tone="success" />
          <span className="flex-1">Control plane online</span>
          <button className="inline-flex items-center gap-1 rounded px-1 hover:text-muted-foreground" onClick={onSearch} type="button"><CommandIcon className="size-3" />K</button>
        </div>
      </div>
    </aside>
  );
}

function NavSection({
  group,
  pathname,
  viewer,
  collapsed,
  onToggle,
  onNavigate,
}: {
  group: NavGroup;
  pathname: string;
  viewer: Viewer | null;
  collapsed: boolean;
  onToggle?: () => void;
  onNavigate?: () => void;
}) {
  return (
    <div>
      {group.label ? (
        <button
          aria-expanded={!collapsed}
          className="group/section mb-0.5 flex h-7 w-full items-center gap-1 rounded-md px-2 text-xs font-medium text-faint hover:bg-hover hover:text-muted-foreground"
          onClick={onToggle}
          type="button"
        >
          {group.label}
          {collapsed ? <ChevronRight className="size-3" /> : <ChevronDown className="size-3 opacity-0 transition-opacity group-hover/section:opacity-100" />}
        </button>
      ) : null}
      {collapsed ? null : (
        <div className="space-y-px">
          {group.items.map((item) => (
            <NavLinkItem
              active={isActivePath(pathname, item.to)}
              count={item.alert && viewer ? viewer.alerts[item.alert] : undefined}
              item={item}
              key={item.to}
              onNavigate={onNavigate}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function NavLinkItem({ item, active, count, onNavigate }: { item: NavItem; active: boolean; count?: number; onNavigate?: () => void }) {
  const Icon = item.icon;
  return (
    <Link
      aria-current={active ? "page" : undefined}
      className={cn(
        "flex h-7 items-center gap-2 rounded-md px-2 text-sm font-medium transition-colors",
        active ? "bg-selected text-foreground" : "text-secondary-foreground/85 hover:bg-hover hover:text-foreground",
      )}
      onClick={onNavigate}
      to={item.to}
    >
      <Icon className={cn("size-4 shrink-0", active ? "text-foreground" : "text-muted-foreground")} />
      <span className="min-w-0 flex-1 truncate">{item.label}</span>
      {count ? <span className="tabular text-xs text-faint">{count}</span> : null}
    </Link>
  );
}

function WorkspaceMenu({ viewer }: { viewer: Viewer | null }) {
  const { theme, toggleTheme } = useTheme();
  const navigate = useNavigate();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-1.5 text-left outline-none hover:bg-hover data-[state=open]:bg-hover" type="button">
          <GridMark size={20} />
          <span className="min-w-0 truncate text-sm font-semibold text-foreground">GridOps</span>
          <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-60">
        {viewer ? (
          <>
            <div className="flex items-center gap-2.5 px-2 py-2">
              <Avatar name={viewer.login} size={28} src={viewer.avatarUrl} />
              <div className="min-w-0">
                <div className="truncate text-sm font-medium">{viewer.name ?? viewer.login}</div>
                <div className="truncate text-2xs text-muted-foreground">@{viewer.login} · {viewer.role === "admin" ? "Administrator" : "Member"}</div>
              </div>
            </div>
            <DropdownMenuSeparator />
          </>
        ) : null}
        <DropdownMenuItem icon={<Settings />} onSelect={() => void navigate({ to: "/settings" })} shortcut={["G", "S"]}>Settings</DropdownMenuItem>
        <DropdownMenuItem icon={theme === "dark" ? <Sun /> : <Moon />} onSelect={toggleTheme}>Switch to {theme === "dark" ? "light" : "dark"} theme</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuLabel>GridOps v0.1.0</DropdownMenuLabel>
        {viewer ? (
          <DropdownMenuItem icon={<LogOut />} onSelect={() => void api("/auth/logout", { method: "POST" }).then(() => { window.location.href = "/login"; })}>Sign out</DropdownMenuItem>
        ) : (
          <DropdownMenuItem onSelect={() => { window.location.href = "/auth/github"; }}>Connect GitHub</DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
