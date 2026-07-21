import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ArrowLeft, LoaderCircle, Save, Server, Settings2 } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { AppShell } from "~/components/app-shell";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusBadge } from "~/components/status-badge";
import { Badge } from "~/components/ui/badge";
import { Button, buttonVariants } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Input } from "~/components/ui/input";
import { SearchableMultiSelect } from "~/components/ui/searchable-multi-select";
import { SearchableSelect } from "~/components/ui/searchable-select";
import {
  type RepositoryOption,
  type RunnerGroupOption,
  type RunnerPoolDetail,
  getInstallationRepositories,
  getInstallationRunnerGroups,
  getRunnerPoolAction,
  updateRunnerPoolAction,
} from "~/features/runner-pools/runner-pools.functions";
import { cn } from "~/lib/utils";

export const Route = createFileRoute("/runner-pools_/$poolId")({
  loader: ({ params }) => getRunnerPoolAction({ data: { poolId: params.poolId } }),
  pendingComponent: () => (
    <ResourcePageLoading
      title="Runner pool"
      description="Loading the pool configuration while the page remains available."
      icon={Settings2}
    />
  ),
  component: EditRunnerPoolPage,
});

function EditRunnerPoolPage() {
  const pool = Route.useLoaderData();
  return <RunnerPoolEditor key={pool.id} pool={pool} />;
}

type RunnerGroupLoadState =
  | { status: "idle" | "loading"; items: RunnerGroupOption[]; error: null }
  | { status: "ready"; items: RunnerGroupOption[]; error: null }
  | { status: "error"; items: RunnerGroupOption[]; error: string };

type RepositoryLoadState =
  | { status: "idle" | "loading"; items: RepositoryOption[]; error: null }
  | { status: "ready"; items: RepositoryOption[]; error: null }
  | { status: "error"; items: RepositoryOption[]; error: string };

function RunnerPoolEditor({ pool }: { pool: RunnerPoolDetail }) {
  const navigate = useNavigate();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<"ephemeral" | "persistent">(pool.mode);
  const [repositoryIds, setRepositoryIds] = useState(pool.repositoryIds);
  const [maxCount, setMaxCount] = useState(pool.maxCount);
  const [runnerGroupId, setRunnerGroupId] = useState(pool.runnerGroupId);
  const shouldLoadRepositories = pool.canManage && pool.scope === "repository";
  const [repositoryLoad, setRepositoryLoad] = useState<RepositoryLoadState>(
    shouldLoadRepositories
      ? { status: "loading", items: pool.repositories, error: null }
      : { status: "idle", items: [], error: null },
  );
  const shouldLoadRunnerGroups = pool.canManage && pool.scope === "organization";
  const [runnerGroupLoad, setRunnerGroupLoad] = useState<RunnerGroupLoadState>(
    shouldLoadRunnerGroups
      ? { status: "loading", items: [], error: null }
      : { status: "idle", items: [], error: null },
  );

  useEffect(() => {
    if (!shouldLoadRunnerGroups) return;
    const controller = new AbortController();
    void getInstallationRunnerGroups(pool.installationId, controller.signal)
      .then(({ items }) => setRunnerGroupLoad({ status: "ready", items, error: null }))
      .catch((cause: unknown) => {
        if (cause instanceof DOMException && cause.name === "AbortError") return;
        setRunnerGroupLoad({
          status: "error",
          items: [],
          error: cause instanceof Error ? cause.message : "Runner groups could not be loaded.",
        });
      });
    return () => controller.abort();
  }, [pool.installationId, shouldLoadRunnerGroups]);

  useEffect(() => {
    if (!shouldLoadRepositories) return;
    const controller = new AbortController();
    void getInstallationRepositories(pool.installationId, controller.signal)
      .then(({ items }) => setRepositoryLoad({ status: "ready", items, error: null }))
      .catch((cause: unknown) => {
        if (cause instanceof DOMException && cause.name === "AbortError") return;
        setRepositoryLoad({
          status: "error",
          items: pool.repositories,
          error: cause instanceof Error ? cause.message : "Repositories could not be loaded.",
        });
      });
    return () => controller.abort();
  }, [pool.installationId, pool.repositories, shouldLoadRepositories]);

  const runnerGroups = runnerGroupLoad.items;

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    const form = new FormData(event.currentTarget);
    try {
      await updateRunnerPoolAction({
        data: {
          poolId: pool.id,
          repositoryIds: pool.scope === "repository" ? repositoryIds : undefined,
          name: String(form.get("name") ?? ""),
          mode,
          labels: String(form.get("labels") ?? "")
            .split(",")
            .map((label) => label.trim())
            .filter(Boolean),
          image: String(form.get("image") ?? ""),
          desiredCount: Number(form.get("desiredCount")),
          minCount: Number(form.get("minCount")),
          maxCount: Number(form.get("maxCount")),
          autoscalingEnabled: form.get("autoscalingEnabled") === "on",
          queueScaleFactor: Number(form.get("queueScaleFactor")),
          idleTimeoutMinutes: Number(form.get("idleTimeoutMinutes")),
          cpuLimit: Number(form.get("cpuLimit")),
          memoryLimitMb: Number(form.get("memoryLimitMb")),
          runnerGroupId: pool.scope === "organization"
            ? runnerGroupId || pool.runnerGroupId
            : 1,
        },
      });
      await navigate({ to: "/runner-pools" });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Runner pool update failed.");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <AppShell>
      <div className="mx-auto max-w-4xl">
        <Link className="inline-flex items-center gap-2 text-sm text-muted-foreground hover:text-foreground" to="/runner-pools">
          <ArrowLeft className="size-4" />Runner pools
        </Link>
        <div className="mt-5 flex flex-col justify-between gap-3 sm:flex-row sm:items-end">
          <div>
            <div className="flex items-center gap-2">
              <Settings2 className="size-5 text-primary" />
              <h1 className="text-2xl font-semibold tracking-tight md:text-3xl">Edit {pool.name}</h1>
              <StatusBadge status={pool.paused ? "paused" : pool.state} />
            </div>
            <p className="mt-1 text-sm text-muted-foreground">
              Generation {pool.configurationVersion} · runtime changes roll through idle runners safely.
            </p>
          </div>
          <Badge variant="outline">{pool.mode}</Badge>
        </div>

        {pool.canManage ? <form className="mt-6 space-y-4" onSubmit={submit}>
          <Card>
            <CardHeader><CardTitle>GitHub destination</CardTitle></CardHeader>
            <CardContent className="grid gap-4 sm:grid-cols-2">
              <ReadOnly label="Installation" value={pool.accountLogin} />
              <ReadOnly label="Scope" value={pool.scope === "repository" ? `${repositoryIds.length} repositories` : "Organization"} />
              {pool.scope === "repository" ? (
                <Field className="sm:col-span-2" label="Repositories" hint={`${repositoryIds.length} selected · maximum ${maxCount}`}>
                  <SearchableMultiSelect
                    ariaLabel="Pool repositories"
                    emptyMessage="No repositories match this search"
                    loading={repositoryLoad.status === "loading"}
                    maxSelected={maxCount}
                    onValueChange={setRepositoryIds}
                    options={repositoryLoad.items.map((repository) => ({
                      value: repository.id,
                      label: repository.fullName,
                      description: repository.private ? "Private repository" : "Public repository",
                    }))}
                    placeholder="Choose one or more repositories…"
                    searchPlaceholder="Search by owner or repository name…"
                    values={repositoryIds}
                  />
                  {repositoryLoad.status === "error" ? <span className="block text-[11px] text-destructive">{repositoryLoad.error}</span> : null}
                </Field>
              ) : (
                <p className="text-[11px] leading-5 text-muted-foreground sm:col-span-2">
                  Repository access is controlled by the selected GitHub runner group.
                </p>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>Runner definition</CardTitle></CardHeader>
            <CardContent className="grid gap-4 md:grid-cols-2">
              <Field label="Pool name" hint="Also used as a runner label.">
                <Input defaultValue={pool.name} name="name" pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" required />
              </Field>
              <Field label="Mode">
                <SearchableSelect
                  ariaLabel="Runner mode"
                  onValueChange={(nextMode) => setMode(nextMode ?? "ephemeral")}
                  options={[
                    { value: "ephemeral", label: "Ephemeral", description: "One clean runner per job" },
                    { value: "persistent", label: "Persistent", description: "Reuse the runner across jobs" },
                  ]}
                  searchable={false}
                  value={mode}
                />
              </Field>
              <Field className="md:col-span-2" label="Container image">
                <Input defaultValue={pool.image} name="image" required />
              </Field>
              <Field className="md:col-span-2" label="Additional labels" hint="Comma-separated; the current pool name is included automatically.">
                <Input defaultValue={pool.labels.join(", ")} name="labels" />
              </Field>
              {pool.scope === "organization" ? (
                <Field
                  label="Runner group"
                  hint={runnerGroupLoad.status === "loading"
                    ? "Loading runner groups from GitHub…"
                    : runnerGroups.length
                      ? "GitHub runner groups available to this installation."
                      : "Enter the GitHub runner group ID."}
                >
                  {runnerGroupLoad.status === "loading" ? (
                    <div className="flex h-9 items-center gap-2 rounded-md border border-input bg-background px-3 text-sm text-muted-foreground" role="status">
                      <LoaderCircle className="size-4 animate-spin" />
                      Loading runner groups…
                    </div>
                  ) : runnerGroups.length ? (
                    <SearchableSelect
                      ariaLabel="GitHub runner group"
                      onValueChange={(nextRunnerGroupId) => setRunnerGroupId(nextRunnerGroupId ?? pool.runnerGroupId)}
                      options={runnerGroups.map((group) => ({
                        value: group.id,
                        label: group.name,
                        description: group.isDefault ? "Default runner group" : `${group.visibility} visibility`,
                      }))}
                      placeholder="Choose runner group…"
                      searchPlaceholder="Search runner groups…"
                      value={runnerGroupId}
                    />
                  ) : (
                    <>
                      <Input
                        min="1"
                        name="runnerGroupId"
                        onChange={(event) => setRunnerGroupId(Number(event.target.value))}
                        required
                        type="number"
                        value={runnerGroupId}
                      />
                      {runnerGroupLoad.status === "error" ? (
                        <span className="block text-[11px] leading-4 text-destructive">
                          {runnerGroupLoad.error} Enter the group ID manually.
                        </span>
                      ) : null}
                    </>
                  )}
                </Field>
              ) : null}
              <div className="rounded-md border border-amber-500/20 bg-amber-500/5 p-3 text-[11px] leading-5 text-amber-100 md:col-span-2">
                Changing the name, mode, image, labels, runner group, CPU, or memory starts a rolling replacement. Busy runners finish their jobs; GridOps replaces idle runners one at a time.
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>Pool capacity and per-runner limits</CardTitle></CardHeader>
            <CardContent className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              <Field label="Target runners" hint="Runners GridOps should keep active now. Autoscaling may change this between the minimum and maximum."><Input defaultValue={pool.desiredCount} max="100" min="0" name="desiredCount" required type="number" /></Field>
              <Field label="Minimum runners" hint="Lowest pool target after idle scale-down. Set 0 to scale all the way down."><Input defaultValue={pool.minCount} max="100" min="0" name="minCount" required type="number" /></Field>
              <Field label="Maximum runners" hint={pool.scope === "repository" ? "Highest pool target during scale-up. Must be at least the number of selected repositories." : "Highest pool target autoscaling can request."}><Input max="100" min={Math.max(1, repositoryIds.length)} name="maxCount" onChange={(event) => setMaxCount(Number(event.target.value))} required type="number" value={maxCount} /></Field>
              <Field label="CPU cores per runner" hint="Docker CPU limit for each runner."><Input defaultValue={pool.cpuLimit} max="64" min="0.25" name="cpuLimit" required step="0.25" type="number" /></Field>
              <Field label="Memory per runner (MB)" hint="Docker memory limit for each runner, in megabytes."><Input defaultValue={pool.memoryLimitMb} max="262144" min="256" name="memoryLimitMb" required step="256" type="number" /></Field>
              <label className="flex items-start gap-3 rounded-md border border-border p-3 sm:col-span-2 xl:col-span-3">
                <input className="mt-0.5 size-4 accent-emerald-500" defaultChecked={pool.autoscalingEnabled} name="autoscalingEnabled" type="checkbox" />
                <span><span className="block text-xs font-medium">Autoscale from queued jobs</span><span className="mt-1 block text-[11px] text-muted-foreground">Queued workflow jobs raise the target up to Maximum runners. When every runner is idle, the target returns to Minimum runners after the delay below.</span></span>
              </label>
              <Field label="Extra runners per queued job" hint="Each queued job requests this many additional runner slots, capped by Maximum runners."><Input defaultValue={pool.queueScaleFactor} max="20" min="1" name="queueScaleFactor" required type="number" /></Field>
              <Field label="Idle scale-down delay (minutes)" hint="After all runners are idle and no jobs are queued for this long, the target returns to Minimum runners."><Input defaultValue={pool.idleTimeoutMinutes} max="1440" min="1" name="idleTimeoutMinutes" required type="number" /></Field>
            </CardContent>
          </Card>

          {error ? <p className="rounded-md border border-red-500/25 bg-red-500/10 px-4 py-3 text-sm text-red-300" role="alert">{error}</p> : null}
          <div className="flex justify-end gap-2">
            <Link className={buttonVariants({ variant: "ghost" })} to="/runner-pools">Cancel</Link>
            <Button disabled={submitting} type="submit">
              {submitting ? <LoaderCircle className="animate-spin" /> : <Save />}
              {submitting ? "Saving changes…" : "Save changes"}
            </Button>
          </div>
        </form> : <Card className="mt-6"><CardHeader><div><CardTitle>Read-only runner pool</CardTitle><p className="mt-1 text-xs text-muted-foreground">An installation administrator manages this pool.</p></div><Badge variant="outline">read only</Badge></CardHeader><CardContent className="grid gap-3 sm:grid-cols-2"><ReadOnly label="Destination" value={pool.scope === "repository" ? `${pool.repositoryIds.length} repositories` : pool.accountLogin} /><ReadOnly label="Runner capacity" value={`${pool.desiredCount} target · ${pool.minCount}-${pool.maxCount} runners`} /><ReadOnly label="Runner image" value={pool.image} /><ReadOnly label="Per-runner resources" value={`${pool.cpuLimit} CPU cores · ${pool.memoryLimitMb} MB memory`} /></CardContent></Card>}
      </div>
    </AppShell>
  );
}

function ReadOnly({ label, value }: { label: string; value: string }) {
  return <div className="rounded-md border border-border p-3"><div className="text-[11px] text-muted-foreground">{label}</div><div className="mt-1 flex items-center gap-2 text-sm font-medium"><Server className="size-3.5 text-primary" />{value}</div></div>;
}

function Field({ label, hint, className, children }: { label: string; hint?: string; className?: string; children: React.ReactNode }) {
  return <label className={cn("space-y-2", className)}><span className="block text-xs font-medium">{label}</span>{children}{hint ? <span className="block text-[11px] leading-4 text-muted-foreground">{hint}</span> : null}</label>;
}
