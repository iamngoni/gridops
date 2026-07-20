import { createFileRoute } from "@tanstack/react-router";
import { useServerFn } from "@tanstack/react-start";
import { LoaderCircle, Pause, Play, Radio, RefreshCw, Terminal } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { ResourcePage } from "~/components/resource-page";
import { StatusBadge } from "~/components/status-badge";
import { Button } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { getLiveLogsPage, runnerLogsAction } from "~/features/operations/operations.functions";

export const Route = createFileRoute("/live-logs")({
  loader: () => getLiveLogsPage(),
  component: LiveLogsPage,
});

function LiveLogsPage() {
  const data = Route.useLoaderData();
  const getLogs = useServerFn(runnerLogsAction);
  const [runnerId, setRunnerId] = useState(data.items[0]?.id ?? "");
  const [logs, setLogs] = useState("");
  const [streaming, setStreaming] = useState(true);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!runnerId) return;
    setLoading(true);
    try {
      const response = await getLogs({ data: { runnerId } });
      setLogs(response.logs || "No container output yet.");
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not load runner logs.");
    } finally {
      setLoading(false);
    }
  }, [getLogs, runnerId]);

  useEffect(() => {
    void refresh();
    if (!streaming) return;
    const interval = window.setInterval(() => void refresh(), 2_000);
    return () => window.clearInterval(interval);
  }, [refresh, streaming]);

  const selected = data.items.find((item) => item.id === runnerId);

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
              <div><CardTitle className="flex items-center gap-2"><Terminal className="size-4" />{selected?.name ?? "Runner output"}</CardTitle><p className="mt-1 text-[11px] text-muted-foreground">Last 500 Docker log lines · refreshes every 2 seconds</p></div>
              <div className="flex gap-1">
                <Button onClick={() => setStreaming((value) => !value)} size="sm" variant="outline">{streaming ? <Pause /> : <Play />}{streaming ? "Pause stream" : "Resume stream"}</Button>
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
