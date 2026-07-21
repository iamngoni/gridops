import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ArrowLeft, Github, LoaderCircle, Server } from "lucide-react";
import { type FormEvent, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";

import { AppShell } from "~/components/app-shell";
import { Button, buttonVariants } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Input } from "~/components/ui/input";
import { SearchableSelect } from "~/components/ui/searchable-select";
import { createRunnerPoolAction, getCreateRunnerPoolOptions } from "~/features/runner-pools/runner-pools.functions";
import { cn } from "~/lib/utils";

export const Route = createFileRoute("/runner-pools_/new")({
  loader: () => getCreateRunnerPoolOptions(),
  component: NewRunnerPoolPage,
});

function NewRunnerPoolPage() {
  const options = Route.useLoaderData();

  useEffect(() => {
    const search = new URLSearchParams(window.location.search);
    if (search.get("appCreated") === "1") {
      toast.success("GitHub App created and authorized. Install it on an account to continue.");
    }
    if (search.get("installationUpdated") === "1") {
      toast.success("GitHub App installation synchronized.");
    }
    if (search.has("appCreated") || search.has("installationUpdated")) {
      window.history.replaceState({}, "", window.location.pathname);
    }
  }, []);

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

  if (options.installations.length === 0) {
    return (
      <AppShell>
        <Card className="mx-auto max-w-xl">
          <CardContent className="flex min-h-80 flex-col items-center justify-center p-8 text-center">
            <Server className="size-8 text-muted-foreground" />
            <h1 className="mt-4 text-xl font-semibold">Install the GitHub App</h1>
            <p className="mt-2 max-w-sm text-sm leading-6 text-muted-foreground">
              Choose the account and repositories GridOps may operate, then return here and sync GitHub.
            </p>
            <a className={cn(buttonVariants(), "mt-5")} href={options.installUrl}>
              <Github />Install GridOps on GitHub
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
  runnerGroups: Array<{
    installationId: number;
    id: number;
    name: string;
    visibility: string;
    isDefault: boolean;
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
  installUrl: string;
};

function RunnerPoolForm({ options }: { options: RunnerPoolFormOptions }) {
  const createPool = createRunnerPoolAction;
  const navigate = useNavigate();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [scope, setScope] = useState<"repository" | "organization">("repository");
  const [installationId, setInstallationId] = useState(options.installations[0]?.id ?? 0);
  const [repositoryId, setRepositoryId] = useState(0);
  const [mode, setMode] = useState<"ephemeral" | "persistent">("ephemeral");
  const repositories = useMemo(
    () => options.repositories.filter((repository) => repository.installationId === installationId),
    [installationId, options.repositories],
  );
  const runnerGroups = useMemo(
    () => options.runnerGroups.filter((group) => group.installationId === installationId),
    [installationId, options.runnerGroups],
  );
  const defaultRunnerGroup = runnerGroups.find((group) => group.isDefault) ?? runnerGroups[0];
  const [runnerGroupId, setRunnerGroupId] = useState(
    options.runnerGroups.find((group) => group.installationId === installationId && group.isDefault)?.id
      ?? options.runnerGroups.find((group) => group.installationId === installationId)?.id
      ?? options.defaults.runnerGroupId,
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
            scope === "repository" ? repositoryId || null : null,
          name: String(form.get("name") ?? ""),
          scope,
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
          runnerGroupId: scope === "organization"
            ? runnerGroupId || defaultRunnerGroup?.id || 1
            : 1,
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
                <SearchableSelect
                  ariaLabel="GitHub installation"
                  onValueChange={(nextInstallationId) => {
                    const nextId = nextInstallationId ?? 0;
                    const nextRunnerGroups = options.runnerGroups.filter((group) => group.installationId === nextId);
                    setInstallationId(nextId);
                    setRepositoryId(0);
                    setRunnerGroupId(
                      nextRunnerGroups.find((group) => group.isDefault)?.id
                        ?? nextRunnerGroups[0]?.id
                        ?? options.defaults.runnerGroupId,
                    );
                  }}
                  options={options.installations.map((installation) => ({
                    value: installation.id,
                    label: installation.accountLogin,
                    description: `${installation.accountType} installation`,
                  }))}
                  placeholder="Choose installation…"
                  searchPlaceholder="Search installations…"
                  value={installationId}
                />
              </Field>
              <Field label="Scope">
                <SearchableSelect
                  ariaLabel="Runner pool scope"
                  onValueChange={(nextScope) => setScope(nextScope ?? "repository")}
                  options={[
                    { value: "repository", label: "Repository", description: "Runners dedicated to one repository" },
                    { value: "organization", label: "Organization", description: "Shared runners across an organization" },
                  ]}
                  searchable={false}
                  value={scope}
                />
              </Field>
              {scope === "repository" && (
                <Field className="md:col-span-2" label="Repository" hint={`${repositories.length} repositories available to this installation`}>
                  <SearchableSelect
                    ariaLabel="Repository"
                    emptyMessage="No repositories match this search"
                    onValueChange={(nextRepositoryId) => setRepositoryId(nextRepositoryId ?? 0)}
                    options={repositories.map((repository) => ({
                      value: repository.id,
                      label: repository.fullName,
                      description: repository.private ? "Private repository" : "Public repository",
                      keywords: [repository.private ? "private" : "public"],
                    }))}
                    placeholder="Choose repository…"
                    searchPlaceholder="Search by owner or repository name…"
                    value={repositoryId || null}
                  />
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
                <Input defaultValue={options.defaults.image} name="image" required />
              </Field>
              <Field className="md:col-span-2" label="Additional labels" hint="Comma-separated; the pool name is always included.">
                <Input defaultValue={options.defaults.labels.join(", ")} name="labels" placeholder="docker, x64" />
              </Field>
              {scope === "organization" ? (
                <Field label="Runner group" hint={runnerGroups.length ? "Groups available to this GitHub App installation." : "GridOps could not discover groups; enter the GitHub runner group ID."}>
                  {runnerGroups.length ? (
                    <SearchableSelect
                      ariaLabel="GitHub runner group"
                      onValueChange={(nextRunnerGroupId) => setRunnerGroupId(nextRunnerGroupId ?? defaultRunnerGroup?.id ?? 1)}
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
                    <Input defaultValue={options.defaults.runnerGroupId} min="1" name="runnerGroupId" type="number" required />
                  )}
                </Field>
              ) : null}
            </CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>Capacity and limits</CardTitle></CardHeader>
            <CardContent className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
              <Field label="Desired" hint="Starts at one runner by default; idle autoscaling may return it to the minimum."><Input defaultValue={options.defaults.desiredCount} min="0" max="100" name="desiredCount" type="number" required /></Field>
              <Field label="Minimum"><Input defaultValue={options.defaults.minCount} min="0" max="100" name="minCount" type="number" required /></Field>
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
