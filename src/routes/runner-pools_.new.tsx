import { useServerFn } from "@tanstack/react-start";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ArrowLeft, Github, LoaderCircle, Server } from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";

import { AppShell } from "~/components/app-shell";
import { Button, buttonVariants } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Input } from "~/components/ui/input";
import { createRunnerPoolAction, getCreateRunnerPoolOptions } from "~/features/runner-pools/runner-pools.functions";
import { cn } from "~/lib/utils";

export const Route = createFileRoute("/runner-pools_/new")({
  loader: () => getCreateRunnerPoolOptions(),
  component: NewRunnerPoolPage,
});

function NewRunnerPoolPage() {
  const options = Route.useLoaderData();
  if (!options.authenticated || !options.defaults) {
    return (
      <AppShell>
        <Card className="mx-auto max-w-xl">
          <CardContent className="flex min-h-80 flex-col items-center justify-center p-8 text-center">
            <Github className="size-8 text-muted-foreground" />
            <h1 className="mt-4 text-xl font-semibold">Connect GitHub first</h1>
            <p className="mt-2 max-w-sm text-sm leading-6 text-muted-foreground">
              GridOps needs an authorized GitHub App installation before it can create a repository or organization runner pool.
            </p>
            <a className={cn(buttonVariants(), "mt-5")} href="/auth/github?returnTo=/runner-pools/new">
              <Github />
              Connect GitHub
            </a>
          </CardContent>
        </Card>
      </AppShell>
    );
  }

  return <RunnerPoolForm options={options} />;
}

type RunnerPoolFormOptions = {
  authenticated: true;
  installations: Array<{ id: number; accountLogin: string; accountType: string }>;
  repositories: Array<{
    id: number;
    installationId: number;
    fullName: string;
    private: boolean;
  }>;
  defaults: {
    image: string;
    labels: string[];
    cpuLimit: number;
    memoryLimitMb: number;
    desiredCount: number;
    minCount: number;
    maxCount: number;
    autoscalingEnabled: boolean;
    queueScaleFactor: number;
    idleTimeoutMinutes: number;
    runnerGroupId: number;
  };
};

function RunnerPoolForm({ options }: { options: RunnerPoolFormOptions }) {
  const createPool = useServerFn(createRunnerPoolAction);
  const navigate = useNavigate();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [scope, setScope] = useState<"repository" | "organization">("repository");
  const [installationId, setInstallationId] = useState(options.installations[0]?.id ?? 0);
  const repositories = useMemo(
    () => options.repositories.filter((repository) => repository.installationId === installationId),
    [installationId, options.repositories],
  );

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    const form = new FormData(event.currentTarget);

    try {
      await createPool({
        data: {
          installationId,
          repositoryId:
            scope === "repository" ? Number(form.get("repositoryId")) || null : null,
          name: String(form.get("name") ?? ""),
          scope,
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
          runnerGroupId: Number(form.get("runnerGroupId")),
        },
      });
      await navigate({ to: "/runner-pools" });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Runner pool creation failed.");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <AppShell>
      <div className="mx-auto max-w-4xl">
        <Link className="inline-flex items-center gap-2 text-sm text-muted-foreground hover:text-foreground" to="/runner-pools">
          <ArrowLeft className="size-4" />
          Runner pools
        </Link>
        <div className="mt-5">
          <h1 className="text-2xl font-semibold tracking-tight md:text-3xl">Create runner pool</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Define the GitHub scope, container image, capacity, and resource boundary.
          </p>
        </div>

        <form className="mt-6 space-y-4" onSubmit={submit}>
          <Card>
            <CardHeader><CardTitle>GitHub destination</CardTitle></CardHeader>
            <CardContent className="grid gap-4 md:grid-cols-2">
              <Field label="Installation">
                <select className="gridops-select" value={installationId} onChange={(event) => setInstallationId(Number(event.target.value))}>
                  {options.installations.map((installation) => (
                    <option key={installation.id} value={installation.id}>{installation.accountLogin} · {installation.accountType}</option>
                  ))}
                </select>
              </Field>
              <Field label="Scope">
                <select className="gridops-select" value={scope} onChange={(event) => setScope(event.target.value as typeof scope)}>
                  <option value="repository">Repository</option>
                  <option value="organization">Organization</option>
                </select>
              </Field>
              {scope === "repository" && (
                <Field className="md:col-span-2" label="Repository">
                  <select className="gridops-select" name="repositoryId" required>
                    <option value="">Choose repository…</option>
                    {repositories.map((repository) => <option key={repository.id} value={repository.id}>{repository.fullName}{repository.private ? " · Private" : ""}</option>)}
                  </select>
                </Field>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>Runner definition</CardTitle></CardHeader>
            <CardContent className="grid gap-4 md:grid-cols-2">
              <Field label="Pool name" hint="Used as a runner label.">
                <Input name="name" placeholder="linux-general" required pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" />
              </Field>
              <Field label="Mode">
                <select className="gridops-select" defaultValue="ephemeral" name="mode">
                  <option value="ephemeral">Ephemeral · one job per runner</option>
                  <option value="persistent">Persistent</option>
                </select>
              </Field>
              <Field className="md:col-span-2" label="Container image">
                <Input defaultValue={options.defaults.image} name="image" required />
              </Field>
              <Field className="md:col-span-2" label="Additional labels" hint="Comma-separated; the pool name is always included.">
                <Input defaultValue={options.defaults.labels.join(", ")} name="labels" placeholder="docker, x64" />
              </Field>
              <Field label="Runner group ID" hint="Use 1 for the default group.">
                <Input defaultValue={options.defaults.runnerGroupId} min="1" name="runnerGroupId" type="number" required />
              </Field>
            </CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>Capacity and limits</CardTitle></CardHeader>
            <CardContent className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
              <Field label="Desired"><Input defaultValue={options.defaults.desiredCount} min="0" max="50" name="desiredCount" type="number" required /></Field>
              <Field label="Minimum"><Input defaultValue={options.defaults.minCount} min="0" max="50" name="minCount" type="number" required /></Field>
              <Field label="Maximum"><Input defaultValue={options.defaults.maxCount} min="1" max="100" name="maxCount" type="number" required /></Field>
              <Field label="CPU cores"><Input defaultValue={options.defaults.cpuLimit} min="0.25" max="64" step="0.25" name="cpuLimit" type="number" required /></Field>
              <Field label="Memory MB"><Input defaultValue={options.defaults.memoryLimitMb} min="256" step="256" name="memoryLimitMb" type="number" required /></Field>
              <label className="flex items-start gap-3 rounded-md border border-border p-3 sm:col-span-2 lg:col-span-5">
                <input className="mt-0.5 size-4 accent-emerald-500" defaultChecked={options.defaults.autoscalingEnabled} name="autoscalingEnabled" type="checkbox" />
                <span><span className="block text-xs font-medium">Autoscale from queued jobs</span><span className="mt-1 block text-[11px] text-muted-foreground">Webhook demand raises desired capacity up to the maximum, then drains idle capacity back to the minimum.</span></span>
              </label>
              <Field label="Runners per queued job"><Input defaultValue={options.defaults.queueScaleFactor} min="1" max="20" name="queueScaleFactor" type="number" required /></Field>
              <Field label="Idle scale-down delay"><Input defaultValue={options.defaults.idleTimeoutMinutes} min="1" max="1440" name="idleTimeoutMinutes" type="number" required /></Field>
            </CardContent>
          </Card>

          {error && <p role="alert" className="rounded-md border border-red-500/25 bg-red-500/10 px-4 py-3 text-sm text-red-300">{error}</p>}

          <div className="flex justify-end gap-2">
            <Link className={buttonVariants({ variant: "ghost" })} to="/runner-pools">Cancel</Link>
            <Button disabled={submitting || options.installations.length === 0} type="submit">
              {submitting ? <LoaderCircle className="animate-spin" /> : <Server />}
              {submitting ? "Creating pool…" : "Create runner pool"}
            </Button>
          </div>
        </form>
      </div>
    </AppShell>
  );
}

function Field({ label, hint, className, children }: { label: string; hint?: string; className?: string; children: React.ReactNode }) {
  return (
    <label className={cn("space-y-2", className)}>
      <span className="block text-xs font-medium">{label}</span>
      {children}
      {hint && <span className="block text-[11px] leading-4 text-muted-foreground">{hint}</span>}
    </label>
  );
}
