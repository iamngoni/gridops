import { Link, getRouteApi, useRouterState } from "@tanstack/react-router";
import {
  Activity,
  Bell,
  Boxes,
  ChevronDown,
  CircleGauge,
  FileClock,
  Github,
  GitPullRequestArrow,
  Menu,
  PackageSearch,
  Radio,
  Search,
  Settings,
  Webhook,
  X,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { GridLogo } from "./grid-logo";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { cn } from "~/lib/utils";
import { api } from "~/lib/api";
import { searchAction } from "~/features/operations/operations.functions";

const navigation = [
  {
    label: "Operate",
    items: [
      { label: "Overview", to: "/", icon: CircleGauge },
      { label: "Repositories", to: "/repositories", icon: PackageSearch },
      { label: "Runner pools", to: "/runner-pools", icon: Boxes },
      { label: "Runners", to: "/runners", icon: Activity },
      { label: "Workflow runs", to: "/workflow-runs", icon: GitPullRequestArrow },
      { label: "Live logs", to: "/live-logs", icon: Radio },
    ],
  },
  {
    label: "Observe",
    items: [
      { label: "Webhooks", to: "/webhooks", icon: Webhook },
      { label: "Audit log", to: "/audit-log", icon: FileClock },
    ],
  },
  {
    label: "System",
    items: [{ label: "Settings", to: "/settings", icon: Settings }],
  },
] as const;

export function AppShell({ children }: { children: React.ReactNode }) {
  const [mobileOpen, setMobileOpen] = useState(false);
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const viewer = getRouteApi("__root__").useLoaderData();
  const search = searchAction;
  const searchRoot = useRef<HTMLLabelElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const normalizedQuery = query.trim();
  const [searchResult, setSearchResult] = useState<{
    query: string;
    items: Array<{ kind: string; id: string; title: string; subtitle: string; href: string }>;
  }>({ query: "", items: [] });
  const [searchOpen, setSearchOpen] = useState(false);
  const results = searchResult.query === normalizedQuery ? searchResult.items : [];
  const searchPending = Boolean(viewer && normalizedQuery.length >= 2 && searchResult.query !== normalizedQuery);

  useEffect(() => {
    function shortcut(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        searchInput.current?.focus();
      }
    }
    window.addEventListener("keydown", shortcut);
    return () => window.removeEventListener("keydown", shortcut);
  }, []);

  useEffect(() => {
    if (!viewer || normalizedQuery.length < 2) {
      return;
    }
    let cancelled = false;
    const timeout = window.setTimeout(() => {
      void search({ data: { query: normalizedQuery } }).then((items) => {
        if (!cancelled) {
          setSearchResult({ query: normalizedQuery, items });
          setSearchOpen(true);
        }
      }).catch(() => {
        if (!cancelled) setSearchResult({ query: normalizedQuery, items: [] });
      });
    }, 180);
    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
    };
  }, [normalizedQuery, search, viewer]);

  useEffect(() => {
    function closeSearch(event: PointerEvent) {
      if (!searchRoot.current?.contains(event.target as Node)) setSearchOpen(false);
    }
    document.addEventListener("pointerdown", closeSearch);
    return () => document.removeEventListener("pointerdown", closeSearch);
  }, []);

  const alertCount = viewer
    ? viewer.alerts.failedRunners + viewer.alerts.failedWebhooks + viewer.alerts.queuedJobs + viewer.alerts.deferredRunnerCleanup
    : 0;
  const currentSection = navigation
    .map((group) => group.items.find((item) => item.to === "/" ? pathname === "/" : pathname.startsWith(item.to)))
    .find((item) => item !== undefined);

  return (
    <div className="app-shell min-h-screen text-foreground">
      <aside
        className={cn(
          "fixed inset-y-0 left-0 z-50 flex w-64 -translate-x-full flex-col border-r border-border/60 bg-sidebar/95 shadow-[12px_0_40px_hsl(160_70%_2%/0.16)] backdrop-blur transition-transform lg:translate-x-0",
          mobileOpen && "translate-x-0",
        )}
      >
        <div className="flex h-16 items-center justify-between border-b border-border px-5">
          <GridLogo />
          <Button
            aria-label="Close navigation"
            className="lg:hidden"
            onClick={() => setMobileOpen(false)}
            size="icon"
            variant="ghost"
          >
            <X />
          </Button>
        </div>

        <nav className="flex-1 space-y-5 overflow-y-auto px-3 py-5" aria-label="Main navigation">
          {navigation.map((group) => (
            <div key={group.label}>
              <p className="px-3 text-[10px] font-semibold uppercase tracking-[0.12em] text-muted-foreground/55">{group.label}</p>
              <div className="mt-1.5 space-y-1">
                {group.items.map((item) => {
                  const active = item.to === "/" ? pathname === "/" : pathname.startsWith(item.to);
                  const Icon = item.icon;
                  return (
                    <Link
                      key={item.to}
                      to={item.to}
                      onClick={() => setMobileOpen(false)}
                      className={cn(
                        "relative flex h-10 items-center gap-3 rounded-lg px-3 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent/80 hover:text-foreground",
                        active && "bg-primary/[0.09] text-primary shadow-[0_1px_0_hsl(150_70%_90%/0.03)_inset]",
                      )}
                    >
                      {active && <span className="absolute inset-y-2 left-0 w-0.5 rounded-full bg-primary" />}
                      <span className={cn("grid size-6 place-items-center rounded-md", active && "bg-primary/10")}><Icon className="size-4" /></span>
                      {item.label}
                    </Link>
                  );
                })}
              </div>
            </div>
          ))}
        </nav>

        <div className="border-t border-border p-4">
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <span className="size-2 rounded-full bg-emerald-400 shadow-[0_0_0_3px_rgba(52,211,153,0.1)]" />
            Control plane online
          </div>
          <p className="mt-2 text-[11px] text-muted-foreground/65">GridOps v0.1.0</p>
        </div>
      </aside>

      {mobileOpen && (
        <button
          aria-label="Close navigation overlay"
          className="fixed inset-0 z-40 bg-black/60 lg:hidden"
          onClick={() => setMobileOpen(false)}
          type="button"
        />
      )}

      <div className="lg:pl-64">
        <header className="sticky top-0 z-30 flex h-16 items-center gap-3 border-b border-border/60 bg-background/80 px-4 backdrop-blur-xl md:px-6">
          <Button
            aria-label="Open navigation"
            className="lg:hidden"
            onClick={() => setMobileOpen(true)}
            size="icon"
            variant="ghost"
          >
            <Menu />
          </Button>

          <div className="hidden min-w-0 items-center gap-2 text-sm md:flex">
            <span className="font-medium">GridOps</span>
            <span className="text-muted-foreground">/</span>
            <span className="truncate text-muted-foreground">{currentSection?.label ?? "Resource detail"}</span>
          </div>

          <div className="ml-auto flex items-center gap-2">
            <label className="relative hidden w-72 xl:block" ref={searchRoot}>
              <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
              <Input autoComplete="off" className="pl-9 pr-14" onChange={(event) => setQuery(event.target.value)} onFocus={() => setSearchOpen(true)} onKeyDown={(event) => { if (event.key === "Escape") setSearchOpen(false); }} placeholder="Search GridOps…" ref={searchInput} value={query} />
              <kbd className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 rounded border border-border bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                ⌘ K
              </kbd>
              {searchOpen && normalizedQuery.length >= 2 ? (
                <div className="absolute right-0 top-11 z-50 w-[420px] overflow-hidden rounded-xl border border-border/80 bg-popover p-1.5 shadow-2xl">
                  {searchPending ? <div className="px-3 py-6 text-center text-xs text-muted-foreground">Searching GridOps…</div> : results.length ? results.map((result) => (
                    <Link className="flex items-center gap-3 rounded-sm px-3 py-2 hover:bg-accent" key={`${result.kind}-${result.id}`} onClick={() => setSearchOpen(false)} to={result.href}>
                      <Search className="size-3.5 text-muted-foreground" />
                      <span className="min-w-0 flex-1"><span className="block truncate text-xs font-medium">{result.title}</span><span className="mt-0.5 block truncate text-[11px] text-muted-foreground">{result.subtitle}</span></span>
                      <span className="text-[10px] uppercase tracking-wide text-muted-foreground">{result.kind}</span>
                    </Link>
                  )) : <div className="px-3 py-6 text-center text-xs text-muted-foreground">No GridOps resources match “{query}”.</div>}
                </div>
              ) : null}
            </label>
            <details className="group relative">
              <summary className="relative inline-flex size-9 cursor-pointer list-none items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"><Bell className="size-4" />{alertCount > 0 ? <span className="absolute right-1.5 top-1.5 size-1.5 rounded-full bg-red-400" /> : null}<span className="sr-only">Notifications</span></summary>
              <div className="absolute right-0 top-11 z-50 w-72 rounded-md border border-border bg-popover p-3 shadow-2xl">
                <div className="text-xs font-medium">Operational notifications</div>
                {viewer ? <div className="mt-3 space-y-2 text-xs"><AlertRow href="/runners" label="Failed runners" value={viewer.alerts.failedRunners} /><AlertRow href="/webhooks" label="Failed webhooks" value={viewer.alerts.failedWebhooks} /><AlertRow href="/workflow-runs" label="Queued jobs" value={viewer.alerts.queuedJobs} /><AlertRow href="/audit-log" label="Deferred GitHub cleanup" value={viewer.alerts.deferredRunnerCleanup} /></div> : <p className="mt-2 text-xs text-muted-foreground">Connect GitHub to see operational alerts.</p>}
              </div>
            </details>
            {viewer ? (
              <div className="flex h-9 items-center gap-2 rounded-md border border-border bg-background px-2.5 text-sm font-medium">
                {viewer.avatarUrl ? (
                  <img className="size-5 rounded-full" src={viewer.avatarUrl} alt="" />
                ) : (
                  <Github className="size-4" />
                )}
                <span className="hidden sm:inline">{viewer.login}</span>
              </div>
            ) : (
              <a
                href="/auth/github"
                className="inline-flex h-9 items-center gap-2 rounded-md border border-border bg-background px-3 text-sm font-medium transition-colors hover:bg-accent"
              >
                <Github className="size-4" />
                <span className="hidden sm:inline">Connect GitHub</span>
              </a>
            )}
            <details className="relative">
              <summary className="inline-flex size-9 cursor-pointer list-none items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"><ChevronDown className="size-4" /><span className="sr-only">Account menu</span></summary>
              <div className="absolute right-0 top-11 z-50 w-48 rounded-md border border-border bg-popover p-1 shadow-2xl">
                <Link className="block rounded-sm px-3 py-2 text-xs hover:bg-accent" to="/settings">Settings</Link>
                {viewer ? <button className="block w-full rounded-sm px-3 py-2 text-left text-xs text-red-300 hover:bg-accent" type="button" onClick={() => void api("/auth/logout", { method: "POST" }).then(() => { window.location.href = "/login"; })}>Sign out</button> : <a className="block rounded-sm px-3 py-2 text-xs hover:bg-accent" href="/auth/github">Connect GitHub</a>}
              </div>
            </details>
          </div>
        </header>

        <main className="mx-auto max-w-[1600px] p-4 md:p-6 xl:p-8">{children}</main>
      </div>
    </div>
  );
}

function AlertRow({ href, label, value }: { href: string; label: string; value: number }) {
  return <Link className="flex items-center justify-between rounded-sm border border-border px-3 py-2 hover:bg-accent" to={href}><span className="text-muted-foreground">{label}</span><span className={value > 0 ? "font-medium text-foreground" : "text-muted-foreground"}>{value}</span></Link>;
}
