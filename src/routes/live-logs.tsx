import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { Activity, ArrowDown, LoaderCircle, Pause, Play, Radio, RefreshCw, Terminal } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { ListPagination } from "~/components/list-pagination";
import { ResourcePage } from "~/components/resource-page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusBadge } from "~/components/status-badge";
import { Button } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { archivedLogsAction, getLiveLogsPage, runnerLogsAction } from "~/features/operations/operations.functions";
import { isNearLogEnd } from "~/lib/log-follow";
import { parsePage } from "~/lib/pagination";

export const Route = createFileRoute("/live-logs")({
  validateSearch: (search: Record<string, unknown>): { target?: string; page?: number } => {
    const target = typeof search.target === "string" ? search.target : undefined;
    const page = parsePage(search.page);
    return { ...(target ? { target } : {}), ...(page > 1 ? { page } : {}) };
  },
  loaderDeps: ({ search }) => ({ target: search.target, page: search.page ?? 1 }),
  loader: ({ deps }) => getLiveLogsPage({ page: deps.page, target: deps.target }),
  pendingComponent: () => (
    <ResourcePageLoading
      title="Live logs"
      description="Stream output from active runners and inspect archived runner logs."
      icon={Radio}
    />
  ),
  component: LiveLogsRoutePage,
});

function LiveLogsRoutePage() {
  const search = Route.useSearch();
  return <LiveLogsPage key={`${search.page ?? 1}:${search.target ?? ""}`} />;
}

function LiveLogsPage() {
  const data = Route.useLoaderData();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const getLogs = runnerLogsAction;
  const getArchive = archivedLogsAction;
  const [targetPage, setTargetPage] = useState(data);
  const targets = targetPage.items;
  const initialTarget = data.items.find((item) => item.id === search.target || item.runnerId === search.target);
  const [runnerId, setRunnerId] = useState(initialTarget?.id ?? data.items[0]?.id ?? "");
  const [logs, setLogs] = useState("");
  const [streaming, setStreaming] = useState(true);
  const [following, setFollowing] = useState(true);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const logViewport = useRef<HTMLPreElement>(null);
  const selected = targets.find((item) => item.id === runnerId) ?? targets[0];
  const selectedId = selected?.id;
  const selectedKind = selected?.kind;

  useEffect(() => {
    if (!data.authenticated) return undefined;
    let cancelled = false;
    let refreshing = false;

    async function refreshTargets() {
      if (cancelled || refreshing || document.visibilityState === "hidden") return;
      refreshing = true;
      try {
        const page = await getLiveLogsPage({ page: search.page, target: search.target });
        if (!cancelled) {
          setTargetPage(page);
          setRunnerId((current) => page.items.some((item) => item.id === current)
            ? current
            : (page.items[0]?.id ?? ""));
        }
      } catch {
        // Keep the last known target list; the active stream has its own error state.
      } finally {
        refreshing = false;
      }
    }

    function handleVisibilityChange() {
      if (document.visibilityState === "visible") void refreshTargets();
    }

    const interval = window.setInterval(() => void refreshTargets(), 5_000);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [data.authenticated, search.page, search.target]);

  const refresh = useCallback(async () => {
    if (!selectedId || !selectedKind) return;
    setLoading(true);
    try {
      const response = selectedKind === "archive"
        ? await getArchive({ data: { streamId: selectedId } })
        : await getLogs({ data: { runnerId: selectedId } });
      setLogs(response.logs || "No container output yet.");
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not load runner logs.");
    } finally {
      setLoading(false);
    }
  }, [getArchive, getLogs, selectedId, selectedKind]);

  useEffect(() => {
    if (!selectedId || !selectedKind) return undefined;
    if (selectedKind === "archive" || !streaming) {
      const initial = window.setTimeout(() => void refresh(), 0);
      return () => window.clearTimeout(initial);
    }

    let cancelled = false;
    let controller: AbortController | undefined;
    void (async () => {
      await delay(0);
      if (cancelled) return;
      setLogs("");
      setError(null);
      setLoading(true);
      let tail = "500";
      while (!cancelled) {
        controller = new AbortController();
        try {
          const response = await fetch(`/api/v1/runners/${encodeURIComponent(selectedId)}/logs/stream?tail=${tail}`, {
            credentials: "same-origin",
            signal: controller.signal,
          });
          if (!response.ok) {
            const body = await response.json().catch(() => null) as { error?: string } | null;
            throw new Error(body?.error ?? `Log stream failed (${response.status}).`);
          }
          const reader = response.body?.getReader();
          if (!reader) throw new Error("This browser cannot read streaming responses.");
          const decoder = new TextDecoder();
          setLoading(false);
          while (!cancelled) {
            const { done, value } = await reader.read();
            if (done) break;
            const chunk = decoder.decode(value, { stream: true });
            if (chunk) setLogs((current) => trimLog(`${current}${chunk}`));
          }
          tail = "0";
          if (!cancelled) await delay(250);
        } catch (cause) {
          if (cancelled || controller.signal.aborted) break;
          setLoading(false);
          setError(cause instanceof Error ? cause.message : "Could not stream runner logs.");
          await delay(1_000);
        }
      }
    })();
    return () => {
      cancelled = true;
      controller?.abort();
    };
  }, [refresh, selectedId, selectedKind, streaming]);

  useLayoutEffect(() => {
    const viewport = logViewport.current;
    if (!viewport || !following || selectedKind !== "live" || !streaming) return;
    viewport.scrollTop = viewport.scrollHeight;
  }, [following, logs, selectedId, selectedKind, streaming]);

  function selectTarget(targetId: string) {
    setRunnerId(targetId);
    setFollowing(true);
    void navigate({ replace: true, search: { page: search.page, target: targetId } });
  }

  function toggleStreaming() {
    const nextStreaming = !streaming;
    setStreaming(nextStreaming);
    if (nextStreaming) setFollowing(true);
  }

  function handleLogScroll() {
    const viewport = logViewport.current;
    if (!viewport || selectedKind !== "live" || !streaming) return;
    setFollowing(isNearLogEnd(viewport));
  }

  function jumpToLatest() {
    const viewport = logViewport.current;
    setFollowing(true);
    viewport?.scrollTo({ behavior: "smooth", top: viewport.scrollHeight });
  }

  return (
    <ResourcePage
      title="Live logs"
      description="Follow job output from active runners and inspect retained logs after a runner is removed."
      icon={Radio}
      emptyTitle="No active log streams"
      emptyDescription="A stream becomes available as soon as a managed runner container is created."
      action="Manage runners"
      actionHref="/runners"
      actionIcon={Activity}
    >
      {targets.length > 0 ? (
        <div className="grid items-start gap-5 lg:grid-cols-[320px_minmax(0,1fr)]">
          <Card><CardHeader><div><CardTitle>Runner streams</CardTitle><p className="mt-1 text-xs text-muted-foreground">Choose a live runner or retained archive.</p></div></CardHeader><CardContent className="p-0">
            <div className="max-h-[28rem] space-y-2 overflow-y-auto p-3 lg:max-h-[36rem]">
              {targets.map((runner) => (
                <button aria-pressed={runner.id === selectedId} className={`w-full rounded-lg p-3 text-left transition-colors ${runner.id === selectedId ? "bg-primary/[0.09] shadow-[inset_3px_0_0_hsl(153_64%_52%)]" : "hover:bg-muted/50"}`} key={runner.id} onClick={() => selectTarget(runner.id)} type="button">
                  <div className="flex items-center justify-between gap-2"><span className="truncate font-mono text-xs font-medium">{runner.name}</span><div className="flex items-center gap-1.5"><span className="text-[10px] uppercase tracking-wide text-muted-foreground">{runner.kind}</span><StatusBadge status={runner.busy ? "busy" : String(runner.status)} /></div></div>
                  <div className="mt-1.5 truncate text-[11px] leading-5 text-muted-foreground">{runner.poolName} · {runner.repository ?? "organization"}</div>
                </button>
              ))}
            </div>
            <ListPagination itemCount={targets.length} noun="log streams" onPageChange={(page) => void navigate({ search: { page, target: undefined } })} page={targetPage.page} perPage={targetPage.perPage} total={targetPage.total} />
          </CardContent></Card>
          <Card className="min-w-0 overflow-hidden">
            <CardHeader>
              <div><CardTitle className="flex items-center gap-2"><Terminal className="size-4 text-primary" />{selected?.name ?? "Runner output"}</CardTitle><p className="mt-1 text-xs text-muted-foreground">{selected?.kind === "archive" ? `Retained runner output · ${formatBytes(selected.sizeBytes ?? 0)}` : "Live job output · reconnects automatically"}</p></div>
              <div className="flex flex-wrap items-center justify-end gap-1">
                {selected?.kind === "live" && streaming ? <span className="mr-1 inline-flex items-center gap-1.5 text-[11px] text-muted-foreground"><span className={`size-1.5 rounded-full ${following ? "bg-emerald-400" : "bg-amber-400"}`} />{following ? "Following output" : "Follow paused"}</span> : null}
                {selected?.kind === "live" ? <Button onClick={toggleStreaming} size="sm" variant="outline">{streaming ? <Pause /> : <Play />}{streaming ? "Pause stream" : "Resume stream"}</Button> : null}
                <Button disabled={loading} onClick={() => void refresh()} size="icon" title="Refresh logs" variant="outline">{loading ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}<span className="sr-only">Refresh logs</span></Button>
              </div>
            </CardHeader>
            <CardContent className="relative p-0">
              {error ? <div className="border-t border-red-500/20 bg-red-500/5 px-4 py-3 text-xs text-red-300">{error}</div> : null}
              <pre aria-live="polite" className="h-[60vh] min-h-[360px] max-h-[640px] overflow-auto border-t border-border/60 bg-[hsl(162_28%_4%)] p-5 font-mono text-[12px] leading-5 text-emerald-100/85 shadow-[inset_0_12px_28px_hsl(160_80%_2%/0.28)]" onScroll={handleLogScroll} ref={logViewport}><code>{logs || (loading ? "Loading runner output…" : "No output yet.")}</code></pre>
              {selected?.kind === "live" && streaming && !following ? (
                <Button className="absolute bottom-4 right-4 shadow-lg" onClick={jumpToLatest} size="sm" variant="secondary">
                  <ArrowDown />Jump to latest
                </Button>
              ) : null}
            </CardContent>
          </Card>
        </div>
      ) : undefined}
    </ResourcePage>
  );
}

const MAX_LOG_CHARACTERS = 1_000_000;

function trimLog(value: string) {
  return value.length > MAX_LOG_CHARACTERS ? value.slice(-MAX_LOG_CHARACTERS) : value;
}

function delay(milliseconds: number) {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

function formatBytes(bytes: number) {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KB`;
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}
