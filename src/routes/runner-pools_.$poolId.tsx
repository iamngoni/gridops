import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ArrowLeft, LoaderCircle, Save, Server, Settings2 } from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";

import { AppShell } from "~/components/app-shell";
import { StatusBadge } from "~/components/status-badge";
import { Badge } from "~/components/ui/badge";
import { Button, buttonVariants } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Input } from "~/components/ui/input";
import {
  getCreateRunnerPoolOptions,
  getRunnerPoolAction,
  updateRunnerPoolAction,
} from "~/features/runner-pools/runner-pools.functions";
import { cn } from "~/lib/utils";

export const Route = createFileRoute("/runner-pools_/$poolId")({
  loader: ({ params }) => Promise.all([
    getRunnerPoolAction({ data: { poolId: params.poolId } }),
    getCreateRunnerPoolOptions(),
  ]),
  component: EditRunnerPoolPage,
});

function EditRunnerPoolPage() {
  const [pool, options] = Route.useLoaderData();
  const navigate = useNavigate();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const runnerGroups = useMemo(
    () => options.runnerGroups.filter((group) => group.installationId === pool.installationId),
    [options.runnerGroups, pool.installationId],
  );

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    const form = new FormData(event.currentTarget);
    try {
      await updateRunnerPoolAction({
        data: {
          poolId: pool.id,
          name: String(form.get("name") ?? ""),
          mode: String(form.get("mode")) as "ephemeral" | "persistent",
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
            ? Number(form.get("runnerGroupId")) || pool.runnerGroupId
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
              <ReadOnly label="Scope" value={pool.scope === "repository" ? pool.repository ?? "Repository" : "Organization"} />
              <p className="text-[11px] leading-5 text-muted-foreground sm:col-span-2">
                A pool destination is immutable because GitHub registrations belong to that target. Create a new pool to move workloads to another repository or organization.
              </p>
            </CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>Runner definition</CardTitle></CardHeader>
            <CardContent className="grid gap-4 md:grid-cols-2">
              <Field label="Pool name" hint="Also used as a runner label.">
                <Input defaultValue={pool.name} name="name" pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" required />
              </Field>
              <Field label="Mode">
                <select className="gridops-select" defaultValue={pool.mode} name="mode">
                  <option value="ephemeral">Ephemeral · one job per runner</option>
                  <option value="persistent">Persistent</option>
                </select>
              </Field>
              <Field className="md:col-span-2" label="Container image">
                <Input defaultValue={pool.image} name="image" required />
              </Field>
              <Field className="md:col-span-2" label="Additional labels" hint="Comma-separated; the current pool name is included automatically.">
                <Input defaultValue={pool.labels.join(", ")} name="labels" />
              </Field>
              {pool.scope === "organization" ? (
                <Field label="Runner group" hint={runnerGroups.length ? "GitHub runner groups available to this installation." : "Enter the GitHub runner group ID."}>
                  {runnerGroups.length ? (
                    <select className="gridops-select" defaultValue={pool.runnerGroupId} name="runnerGroupId">
                      {runnerGroups.map((group) => <option key={group.id} value={group.id}>{group.name}{group.isDefault ? " · Default" : ` · ${group.visibility}`}</option>)}
                    </select>
                  ) : (
                    <Input defaultValue={pool.runnerGroupId} min="1" name="runnerGroupId" required type="number" />
                  )}
                </Field>
              ) : null}
              <div className="rounded-md border border-amber-500/20 bg-amber-500/5 p-3 text-[11px] leading-5 text-amber-100 md:col-span-2">
                Changing the name, mode, image, labels, runner group, CPU, or memory starts a rolling replacement. Busy runners finish their jobs; GridOps replaces idle runners one at a time.
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>Capacity and limits</CardTitle></CardHeader>
            <CardContent className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
              <Field label="Desired"><Input defaultValue={pool.desiredCount} max="100" min="0" name="desiredCount" required type="number" /></Field>
              <Field label="Minimum"><Input defaultValue={pool.minCount} max="100" min="0" name="minCount" required type="number" /></Field>
              <Field label="Maximum"><Input defaultValue={pool.maxCount} max="100" min="1" name="maxCount" required type="number" /></Field>
              <Field label="CPU cores"><Input defaultValue={pool.cpuLimit} max="64" min="0.25" name="cpuLimit" required step="0.25" type="number" /></Field>
              <Field label="Memory MB"><Input defaultValue={pool.memoryLimitMb} max="262144" min="256" name="memoryLimitMb" required step="256" type="number" /></Field>
              <label className="flex items-start gap-3 rounded-md border border-border p-3 sm:col-span-2 lg:col-span-5">
                <input className="mt-0.5 size-4 accent-emerald-500" defaultChecked={pool.autoscalingEnabled} name="autoscalingEnabled" type="checkbox" />
                <span><span className="block text-xs font-medium">Autoscale from queued jobs</span><span className="mt-1 block text-[11px] text-muted-foreground">Increase desired capacity from workflow demand and return to minimum after the idle delay.</span></span>
              </label>
              <Field label="Runners per queued job"><Input defaultValue={pool.queueScaleFactor} max="20" min="1" name="queueScaleFactor" required type="number" /></Field>
              <Field label="Idle scale-down delay"><Input defaultValue={pool.idleTimeoutMinutes} max="1440" min="1" name="idleTimeoutMinutes" required type="number" /></Field>
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
        </form> : <Card className="mt-6"><CardHeader><div><CardTitle>Read-only runner pool</CardTitle><p className="mt-1 text-xs text-muted-foreground">An installation administrator manages this pool.</p></div><Badge variant="outline">read only</Badge></CardHeader><CardContent className="grid gap-3 sm:grid-cols-2"><ReadOnly label="Destination" value={pool.repository ?? pool.accountLogin} /><ReadOnly label="Capacity" value={`${pool.desiredCount} desired · ${pool.minCount}-${pool.maxCount}`} /><ReadOnly label="Runner image" value={pool.image} /><ReadOnly label="Resources" value={`${pool.cpuLimit} CPU · ${pool.memoryLimitMb} MB`} /></CardContent></Card>}
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
