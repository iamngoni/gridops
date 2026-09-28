import { useNavigate } from "@tanstack/react-router";
import { Command } from "cmdk";
import { Activity, ArrowRight, Boxes, LoaderCircle, LogOut, Moon, PackageSearch, Plus, RefreshCw, Search, Sun, type LucideIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { RunStatusIcon } from "./status-icon";
import { useTheme } from "./theme-provider";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";
import { Shortcut } from "./ui/kbd";
import { searchAction, syncGitHubAction } from "~/features/operations/operations.functions";
import { api } from "~/lib/api";
import { allNavItems } from "~/lib/navigation";

type SearchResult = { kind: string; id: string; title: string; subtitle: string; href: string; state?: string | null };

const kindIcons: Record<string, LucideIcon> = { repository: PackageSearch, "runner pool": Boxes, runner: Activity };

function ResultIcon({ result }: { result: SearchResult }) {
  if (result.kind === "workflow run") return <RunStatusIcon status={result.state} />;
  const Icon = kindIcons[result.kind] ?? Search;
  return <Icon />;
}

const itemClassName =
  "flex h-10 cursor-default select-none items-center gap-3 rounded-md px-3 text-sm text-secondary-foreground outline-none data-[selected=true]:bg-popover-hover data-[selected=true]:text-foreground [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-muted-foreground";

export function CommandMenu({ open, onOpenChange, signedIn }: { open: boolean; onOpenChange: (open: boolean) => void; signedIn: boolean }) {
  const navigate = useNavigate();
  const { theme, toggleTheme } = useTheme();
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<{ query: string; items: SearchResult[] }>({ query: "", items: [] });
  const normalized = query.trim();
  const searching = signedIn && normalized.length >= 2 && results.query !== normalized;

  useEffect(() => {
    if (!open || !signedIn || normalized.length < 2) return undefined;
    let cancelled = false;
    const timeout = window.setTimeout(() => {
      void searchAction({ data: { query: normalized } })
        .then((items) => { if (!cancelled) setResults({ query: normalized, items }); })
        .catch(() => { if (!cancelled) setResults({ query: normalized, items: [] }); });
    }, 160);
    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
    };
  }, [normalized, open, signedIn]);

  function run(action: () => void) {
    onOpenChange(false);
    setQuery("");
    action();
  }

  const serverResults = results.query === normalized ? results.items : [];

  return (
    <Dialog onOpenChange={(next) => { onOpenChange(next); if (!next) setQuery(""); }} open={open}>
      <DialogContent aria-describedby={undefined} className="max-w-[640px]" instant>
        <DialogTitle className="sr-only">Command menu</DialogTitle>
        <DialogDescription className="sr-only">Search GridOps or run a command.</DialogDescription>
        <Command className="flex min-h-0 flex-col" label="Command menu" loop>
          <div className="flex h-12 shrink-0 items-center gap-3 border-b border-border px-4">
            <Search className="size-4 text-muted-foreground" />
            <Command.Input
              autoFocus
              className="h-full min-w-0 flex-1 bg-transparent text-base text-foreground outline-none placeholder:text-faint"
              onValueChange={setQuery}
              placeholder="Type a command or search pools, runners, runs…"
              value={query}
            />
            {searching ? <LoaderCircle className="size-4 animate-spin text-muted-foreground" /> : null}
          </div>
          <Command.List className="max-h-[min(60vh,440px)] overflow-y-auto overscroll-contain p-2 [&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:pb-1.5 [&_[cmdk-group-heading]]:pt-2.5 [&_[cmdk-group-heading]]:text-2xs [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:text-faint">
            <Command.Empty className="px-3 py-10 text-center text-sm text-muted-foreground">
              {searching ? "Searching GridOps…" : `No results for “${normalized}”.`}
            </Command.Empty>

            {serverResults.length ? (
              <Command.Group heading="Results">
                {serverResults.map((result) => (
                  <Command.Item
                    className={itemClassName}
                    key={`${result.kind}-${result.id}`}
                    keywords={[normalized, result.subtitle, result.kind]}
                    onSelect={() => run(() => void navigate({ href: result.href }))}
                    value={`${result.kind} ${result.title} ${result.id}`}
                  >
                    <ResultIcon result={result} />
                    <span className="min-w-0 flex-1 truncate">
                      <span className="text-foreground">{result.title}</span>
                      <span className="ml-2 text-xs text-muted-foreground">{result.subtitle}</span>
                    </span>
                    <span className="shrink-0 text-2xs text-faint">{result.kind.charAt(0).toUpperCase() + result.kind.slice(1)}</span>
                  </Command.Item>
                ))}
              </Command.Group>
            ) : null}

            <Command.Group heading="Navigation">
              {allNavItems.map((item) => (
                <Command.Item className={itemClassName} key={item.to} keywords={["go", "open", "view"]} onSelect={() => run(() => void navigate({ to: item.to }))} value={`Go to ${item.label}`}>
                  <item.icon />
                  <span className="flex-1">Go to {item.label}</span>
                  <Shortcut keys={["G", item.shortcut.toUpperCase()]} />
                </Command.Item>
              ))}
            </Command.Group>

            <Command.Group heading="Actions">
              <Command.Item className={itemClassName} keywords={["new", "provision", "add"]} onSelect={() => run(() => void navigate({ to: "/runner-pools/new" }))} value="Create runner pool">
                <Plus /><span className="flex-1">Create runner pool</span><Shortcut keys={["C"]} />
              </Command.Item>
              {signedIn ? (
                <Command.Item
                  className={itemClassName}
                  keywords={["sync", "installations", "repositories"]}
                  onSelect={() => run(() => {
                    const pending = syncGitHubAction();
                    toast.promise(pending, { loading: "Refreshing GitHub access…", success: "GitHub installation access refreshed.", error: (error: unknown) => error instanceof Error ? error.message : "GitHub refresh failed." });
                  })}
                  value="Refresh GitHub access"
                >
                  <RefreshCw /><span className="flex-1">Refresh GitHub access</span><ArrowRight />
                </Command.Item>
              ) : null}
              <Command.Item className={itemClassName} keywords={["theme", "appearance", "dark", "light"]} onSelect={() => run(toggleTheme)} value="Switch theme">
                {theme === "dark" ? <Sun /> : <Moon />}<span className="flex-1">Switch to {theme === "dark" ? "light" : "dark"} theme</span>
              </Command.Item>
              {signedIn ? (
                <Command.Item
                  className={itemClassName}
                  keywords={["logout", "log out"]}
                  onSelect={() => run(() => void api("/auth/logout", { method: "POST" }).then(() => { window.location.href = "/login"; }))}
                  value="Sign out"
                >
                  <LogOut /><span className="flex-1">Sign out</span>
                </Command.Item>
              ) : null}
            </Command.Group>
          </Command.List>
          <div className="flex h-9 shrink-0 items-center gap-4 border-t border-border px-4 text-2xs text-faint">
            <span className="inline-flex items-center gap-1.5"><Shortcut keys={["↑"]} /><Shortcut keys={["↓"]} />to navigate</span>
            <span className="inline-flex items-center gap-1.5"><Shortcut keys={["↵"]} />to select</span>
            <span className="inline-flex items-center gap-1.5"><Shortcut keys={["esc"]} />to close</span>
          </div>
        </Command>
      </DialogContent>
    </Dialog>
  );
}
