import { createFileRoute } from "@tanstack/react-router";
import { LoaderCircle, Pause, Play, Radio, RefreshCw, Terminal } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { ResourcePage } from "~/components/resource-page";
import { StatusBadge } from "~/components/status-badge";
import { Button } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { archivedLogsAction, getLiveLogsPage, runnerLogsAction } from "~/features/operations/operations.functions";

export const Route = createFileRoute("/live-logs")({
  validateSearch: (search: Record<string, unknown>) => ({
    target: typeof search.target === "string" ? search.target : undefined,
  }),
  loader: () => getLiveLogsPage(),
  component: LiveLogsPage,
});

function LiveLogsPage() {
  const data = Route.useLoaderData();
  const search = Route.useSearch();
  const getLogs = runnerLogsAction;
  const getArchive = archivedLogsAction;
  const [runnerId, setRunnerId] = useState(
    data.items.some((item) => item.id === search.target) ? String(search.target) : (data.items[0]?.id ?? ""),
  );
  const [logs, setLogs] = useState("");
  const [streaming, setStreaming] = useState(true);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const selected = data.items.find((item) => item.id === runnerId);

  const refresh = useCallback(async () => {
    if (!selected) return;
    setLoading(true);
    try {
      const response = selected.kind === "archive"
        ? await getArchive({ data: { streamId: selected.id } })
        : await getLogs({ data: { runnerId: selected.id } });
      setLogs(response.logs || "No container output yet.");
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not load runner logs.");
    } finally {
      setLoading(false);
    }
  }, [getArchive, getLogs, selected]);

  useEffect(() => {
    if (!selected) return undefined;
    if (selected.kind === "archive" || !streaming) {
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
          const response = await fetch(`/api/v1/runners/${encodeURIComponent(selected.id)}/logs/stream?tail=${tail}`, {
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
  }, [refresh, selected, streaming]);

  return (
    <ResourcePage
      title="Live logs"
      description="Stream active runner output from the isolated Docker manager."
      icon={Radio}
      emptyTitle="No active log streams"
      emptyDescription="A stream becomes available as soon as a managed runner container is created."
      action="Manage runners"
      actionHref="/runners"
    >
      {data.items.length > 0 ? (
        <div className="grid gap-4 lg:grid-cols-[300px_minmax(0,1fr)]">
          <Card><CardHeader><CardTitle>Managed streams</CardTitle></CardHeader><CardContent className="space-y-2">
            {data.items.map((runner) => (
              <button className={`w-full rounded-md border p-3 text-left transition-colors ${runner.id === runnerId ? "border-primary/40 bg-primary/5" : "border-border hover:bg-muted/40"}`} key={runner.id} onClick={() => setRunnerId(runner.id)} type="button">
                <div className="flex items-center justify-between gap-2"><span className="truncate font-mono text-xs font-medium">{runner.name}</span><StatusBadge status={runner.busy ? "busy" : String(runner.status)} /></div>
                <div className="mt-2 truncate text-[11px] text-muted-foreground">{runner.poolName} · {runner.repository ?? "organization"}</div>
              </button>
            ))}
          </CardContent></Card>
          <Card className="min-w-0 overflow-hidden">
            <CardHeader>
              <div><CardTitle className="flex items-center gap-2"><Terminal className="size-4" />{selected?.name ?? "Runner output"}</CardTitle><p className="mt-1 text-[11px] text-muted-foreground">{selected?.kind === "archive" ? `Retained Docker log · ${formatBytes(selected.sizeBytes ?? 0)}` : "Streaming Docker output · reconnects automatically"}</p></div>
              <div className="flex gap-1">
                {selected?.kind === "live" ? <Button onClick={() => setStreaming((value) => !value)} size="sm" variant="outline">{streaming ? <Pause /> : <Play />}{streaming ? "Pause stream" : "Resume stream"}</Button> : null}
                <Button disabled={loading} onClick={() => void refresh()} size="icon" variant="outline">{loading ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}<span className="sr-only">Refresh logs</span></Button>
              </div>
            </CardHeader>
            <CardContent className="p-0">
              {error ? <div className="border-t border-red-500/20 bg-red-500/5 px-4 py-3 text-xs text-red-300">{error}</div> : null}
              <pre aria-live="polite" className="min-h-[540px] max-h-[68vh] overflow-auto border-t border-border bg-[#070b0b] p-4 font-mono text-[11px] leading-5 text-emerald-100/80"><code>{logs || (loading ? "Loading runner output…" : "No output yet.")}</code></pre>
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
