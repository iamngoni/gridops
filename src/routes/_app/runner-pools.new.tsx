import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { Boxes, Github, LoaderCircle, Plus } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";
import { toast } from "sonner";

import { Callout, EmptyState, PageBody, PageHeader } from "~/components/page";
import { PoolPlanPreview } from "~/components/pool-plan-preview";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { SettingsLayout, SettingsRow, SettingsSection, Switch, UnitField } from "~/components/settings-ui";
import { Button, buttonVariants } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import { SearchableMultiSelect } from "~/components/ui/searchable-multi-select";
import { SearchableSelect } from "~/components/ui/searchable-select";
import {
  type RepositoryOption,
  type RunnerGroupOption,
  createRunnerPoolAction,
  getCreateRunnerPoolOptions,
  getInstallationRunnerGroups,
  getRunnerPoolRepositories,
} from "~/features/runner-pools/runner-pools.functions";
import { formatResourceNumber, hostResourceWarning } from "~/features/runner-pools/resource-risk";

export const Route = createFileRoute("/_app/runner-pools/new")({
  loader: () => getCreateRunnerPoolOptions(),
  pendingComponent: () => <ResourcePageLoading icon={Boxes} title="New runner pool" />,
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

  const header = <PageHeader breadcrumbs={[{ label: "Runner pools", to: "/runner-pools" }]} title="New pool" />;

  if (!options.authenticated || !options.defaults) {
    return (
      <>
        {header}
        <PageBody>
          <EmptyState description="GridOps needs an authorized GitHub App installation before it can create a repository or organization runner pool." icon={Github} title="Connect GitHub first">
            <a className={buttonVariants()} href="/auth/github?returnTo=/runner-pools/new"><Github />Connect GitHub</a>
          </EmptyState>
        </PageBody>
      </>
    );
  }

  if (options.installations.length === 0) {
    return (
      <>
        {header}
        <PageBody>
          <EmptyState description="Choose the account and repositories GridOps may operate, then come back here and refresh GitHub access." icon={Github} title="Install the GitHub App">
            <a className={buttonVariants()} href={options.installUrl}><Github />Install GridOps on GitHub</a>
          </EmptyState>
        </PageBody>
      </>
    );
  }

  return (
    <>
      {header}
      <PageBody>
        <RunnerPoolForm options={options} />
      </PageBody>
    </>
  );
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
  bitbucketConnections: Array<{ id: string; name: string; workspace: string; workspaceUuid: string }>;
  defaults: {
    provider: "docker" | "tart";
    providers: Array<"docker" | "tart">;
    image: string;
    dockerImage: string;
    tartImage: string;
    macosRuntime?: "vm" | "native";
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
    maxCpuLimit: number;
    maxMemoryLimitMb: number;
  };
  installUrl: string;
};

type RunnerProvider = "docker" | "tart";

type AsyncOptions<T> =
  | { status: "loading"; items: T[]; error: null }
  | { status: "ready"; items: T[]; error: null }
  | { status: "error"; items: T[]; error: string };

function RunnerPoolForm({ options }: { options: RunnerPoolFormOptions }) {
  const createPool = createRunnerPoolAction;
  const navigate = useNavigate();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [scope, setScope] = useState<"repository" | "organization">("repository");
  const [installationId, setInstallationId] = useState(options.installations[0]?.id ?? 0);
  const [repositoryIds, setRepositoryIds] = useState<number[]>([]);
  const [bitbucketConnectionIds, setBitbucketConnectionIds] = useState<string[]>([]);
  const [mode, setMode] = useState<"ephemeral" | "persistent">("ephemeral");
  const [providers, setProviders] = useState<RunnerProvider[]>(options.defaults.providers);
  const provider = providers[0] ?? options.defaults.provider;
  const includesTart = providers.includes("tart");
  const [dockerImage, setDockerImage] = useState(options.defaults.dockerImage);
  const [tartImage, setTartImage] = useState(options.defaults.tartImage);
  const [macosRuntime, setMacosRuntime] = useState<"vm" | "native">(options.defaults.macosRuntime ?? "vm");
  const [maxCount, setMaxCount] = useState(options.defaults.maxCount);
  const [desiredCount, setDesiredCount] = useState(options.defaults.desiredCount);
  const [cpuLimit, setCpuLimit] = useState(options.defaults.cpuLimit);
  const [memoryLimitMb, setMemoryLimitMb] = useState(options.defaults.memoryLimitMb);
  const [poolName, setPoolName] = useState("");
  const [labels, setLabels] = useState(options.defaults.labels.join(", "));
  const initialInstallation = options.installations.find((installation) => installation.id === installationId);
  const [repositoryLoad, setRepositoryLoad] = useState<AsyncOptions<RepositoryOption>>(
    { status: "loading", items: [], error: null },
  );
  const [runnerGroupLoad, setRunnerGroupLoad] = useState<AsyncOptions<RunnerGroupOption>>(
    initialInstallation?.accountType === "Organization"
      ? { status: "loading", items: [], error: null }
      : { status: "ready", items: [], error: null },
  );
  const repositories = repositoryLoad.items;
  const selectedRepositories = repositories.filter((repository) => repositoryIds.includes(repository.id));
  const selectedAccounts = [...new Set(selectedRepositories.map((repository) => repository.accountLogin))];
  const runnerGroups = runnerGroupLoad.items;
  const defaultRunnerGroup = runnerGroups.find((group) => group.isDefault) ?? runnerGroups[0];
  const [runnerGroupId, setRunnerGroupId] = useState(options.defaults.runnerGroupId);
  const resourceWarning = hostResourceWarning({
    runnerCount: desiredCount,
    cpuLimit,
    memoryLimitMb,
    cpuBudget: options.defaults.maxCpuLimit,
    memoryBudgetMb: options.defaults.maxMemoryLimitMb,
  });

  useEffect(() => {
    const controller = new AbortController();
    void getRunnerPoolRepositories(controller.signal)
      .then(({ items }) => setRepositoryLoad({ status: "ready", items, error: null }))
      .catch((cause: unknown) => {
        if (cause instanceof DOMException && cause.name === "AbortError") return;
        setRepositoryLoad({
          status: "error",
          items: [],
          error: cause instanceof Error ? cause.message : "Repositories could not be loaded.",
        });
      });
    return () => controller.abort();
  }, []);

  useEffect(() => {
    if (!installationId || scope !== "organization") return;
    const controller = new AbortController();
    const installation = options.installations.find((candidate) => candidate.id === installationId);
    if (installation?.accountType === "Organization") {
      void getInstallationRunnerGroups(installationId, controller.signal)
        .then(({ items }) => {
          setRunnerGroupLoad({ status: "ready", items, error: null });
          setRunnerGroupId(items.find((group) => group.isDefault)?.id ?? items[0]?.id ?? options.defaults.runnerGroupId);
        })
        .catch((cause: unknown) => {
          if (cause instanceof DOMException && cause.name === "AbortError") return;
          setRunnerGroupLoad({
            status: "error",
            items: [],
            error: cause instanceof Error ? cause.message : "Runner groups could not be loaded.",
          });
        });
    }
    return () => controller.abort();
  }, [installationId, options.defaults.runnerGroupId, options.installations, scope]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    const form = new FormData(event.currentTarget);

    try {
      await createPool({
        data: {
          installationId: scope === "repository"
            ? selectedRepositories[0]?.installationId ?? 0
            : installationId,
          repositoryIds: scope === "repository" ? repositoryIds : [],
          bitbucketConnectionIds,
          name: poolName,
          scope,
          mode,
          provider,
          providers,
          labels: labels
            .split(",")
            .map((label) => label.trim())
            .filter(Boolean),
          image: provider === "tart" ? tartImage : dockerImage,
          dockerImage,
          tartImage,
          macosRuntime,
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

  const labelList = labels.split(",").map((label) => label.trim()).filter(Boolean);

  return (
    <form className="contents" onSubmit={submit}>
      <SettingsLayout description="Choose where runners register, what they run on, and how far the pool may scale." title="New runner pool">
        <SettingsSection description="Where this pool’s runners register." title="Destination">
          <SettingsRow description={scope === "repository" ? "Capacity shared across repositories, even from different accounts." : "Runners shared across one organization through a runner group."} label="Scope">
            <SearchableSelect
              ariaLabel="Runner pool scope"
              onValueChange={(nextScope) => setScope(nextScope ?? "repository")}
              options={[
                { value: "repository", label: "Repositories", description: "Shared capacity across repositories and accounts" },
                { value: "organization", label: "Organization", description: "Shared runners across one organization" },
              ]}
              searchable={false}
              value={scope}
            />
          </SettingsRow>
          {scope === "organization" ? (
            <SettingsRow label="Installation">
              <SearchableSelect
                ariaLabel="GitHub installation"
                onValueChange={(nextInstallationId) => {
                  const nextId = nextInstallationId ?? 0;
                  const nextInstallation = options.installations.find((installation) => installation.id === nextId);
                  setInstallationId(nextId);
                  setRunnerGroupLoad(
                    nextInstallation?.accountType === "Organization"
                      ? { status: "loading", items: [], error: null }
                      : { status: "ready", items: [], error: null },
                  );
                  setRunnerGroupId(options.defaults.runnerGroupId);
                }}
                options={options.installations.map((installation) => ({ value: installation.id, label: installation.accountLogin, description: `${installation.accountType} installation` }))}
                placeholder="Choose installation…"
                searchPlaceholder="Search installations…"
                value={installationId}
              />
            </SettingsRow>
          ) : (
            <SettingsRow
              description={`${repositoryIds.length} selected across ${selectedAccounts.length} ${selectedAccounts.length === 1 ? "account" : "accounts"} · ${repositories.length} available · the maximum runner count must cover every repository.`}
              label="Repositories"
              stacked
            >
              <SearchableMultiSelect
                ariaLabel="Repositories"
                emptyMessage="No repositories match this search"
                loading={repositoryLoad.status === "loading"}
                maxSelected={maxCount}
                onValueChange={setRepositoryIds}
                options={repositories.map((repository) => ({
                  value: repository.id,
                  label: repository.fullName,
                  description: `${repository.accountLogin} · ${repository.private ? "Private" : "Public"}`,
                  keywords: [repository.accountLogin, repository.accountType, repository.private ? "private" : "public"],
                }))}
                placeholder="Choose one or more repositories…"
                searchPlaceholder="Search by owner or repository name…"
                selectedNoun="repositories"
                values={repositoryIds}
              />
              {repositoryLoad.status === "error" ? <p className="mt-1.5 text-xs text-danger">{repositoryLoad.error}</p> : null}
            </SettingsRow>
          )}
          {scope === "organization" ? (
            <SettingsRow description={runnerGroupLoad.status === "loading" ? "Loading runner groups from GitHub…" : runnerGroups.length ? "Groups available to this installation." : "Enter the GitHub runner group ID."} label="Runner group">
              {runnerGroupLoad.status === "loading" ? (
                <div className="flex h-8 items-center gap-2 text-sm text-muted-foreground" role="status"><LoaderCircle className="size-4 animate-spin" />Loading…</div>
              ) : runnerGroups.length ? (
                <SearchableSelect
                  ariaLabel="GitHub runner group"
                  onValueChange={(nextRunnerGroupId) => setRunnerGroupId(nextRunnerGroupId ?? defaultRunnerGroup?.id ?? 1)}
                  options={runnerGroups.map((group) => ({ value: group.id, label: group.name, description: group.isDefault ? "Default runner group" : `${group.visibility} visibility` }))}
                  placeholder="Choose runner group…"
                  searchPlaceholder="Search runner groups…"
                  value={runnerGroupId}
                />
              ) : (
                <Input min="1" name="runnerGroupId" onChange={(event) => setRunnerGroupId(Number(event.target.value))} required type="number" value={runnerGroupId} />
              )}
              {runnerGroupLoad.status === "error" ? <p className="mt-1.5 text-left text-xs text-danger">{runnerGroupLoad.error}</p> : null}
            </SettingsRow>
          ) : null}
          <SettingsRow
            description={options.bitbucketConnections.length
              ? "Optional. Each workspace shares this pool’s capacity with a managed macOS runner for Bitbucket Pipelines."
              : <>Optional. Connect a workspace first in <Link className="text-foreground underline-offset-2 hover:underline" to="/settings/integrations">Settings → Integrations</Link>.</>}
            label="Bitbucket workspaces"
            stacked
          >
            <SearchableMultiSelect
              ariaLabel="Bitbucket Cloud workspaces"
              emptyMessage="No Bitbucket connections are configured"
              maxSelected={maxCount}
              onValueChange={(nextConnections) => {
                setBitbucketConnectionIds(nextConnections);
                if (nextConnections.length && !providers.includes("tart")) setProviders((current) => [...current, "tart"]);
              }}
              options={options.bitbucketConnections.map((connection) => ({ value: connection.id, label: connection.name, description: `${connection.workspace} workspace`, keywords: [connection.workspace, "bitbucket", "workspace"] }))}
              placeholder="Choose connected Bitbucket workspaces…"
              searchPlaceholder="Search Bitbucket workspaces…"
              selectedNoun="workspaces"
              values={bitbucketConnectionIds}
            />
          </SettingsRow>
        </SettingsSection>

        <SettingsSection description="What each runner is and how jobs find it." title="Runner definition">
          <SettingsRow description="Lowercase letters, numbers, and hyphens. Also registered as a runner label." htmlFor="pool-name" label="Pool name">
            <Input id="pool-name" name="name" onChange={(event) => setPoolName(event.target.value)} pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" placeholder={providers.length > 1 ? "cross-platform" : provider === "tart" ? "macos-arm64" : "linux-general"} required value={poolName} />
          </SettingsRow>
          <SettingsRow description="Jobs route to the first provider whose system labels match. Capacity limits are shared." label="Providers">
            <SearchableMultiSelect
              ariaLabel="Runner providers"
              maxSelected={2}
              onValueChange={(nextProviders) => {
                if (nextProviders.length === 0) return;
                setProviders(nextProviders);
                if (nextProviders.includes("tart")) setMode("ephemeral");
              }}
              options={[
                { value: "docker", label: "Linux · Docker", description: "Fast Linux containers" },
                { value: "tart", label: "macOS · Tart", description: "macOS VMs or native host execution" },
              ]}
              placeholder="Choose providers…"
              searchPlaceholder="Search providers…"
              selectedNoun="providers"
              values={providers}
            />
          </SettingsRow>
          <SettingsRow description={includesTart ? "macOS runners are always ephemeral." : "Ephemeral runners take one job each; persistent runners are reused."} label="Mode">
            <SearchableSelect
              ariaLabel="Runner mode"
              onValueChange={(nextMode) => setMode(nextMode ?? "ephemeral")}
              options={includesTart
                ? [{ value: "ephemeral", label: "Ephemeral", description: "One clean runner per job" }]
                : [
                  { value: "ephemeral", label: "Ephemeral", description: "One clean runner per job" },
                  { value: "persistent", label: "Persistent", description: "Reuse the runner across jobs" },
                ]}
              searchable={false}
              value={mode}
            />
          </SettingsRow>
          {providers.includes("docker") ? (
            <SettingsRow description="OCI image for each Linux runner." htmlFor="docker-image" label="Docker image">
              <Input className="font-mono text-xs" id="docker-image" name="dockerImage" onChange={(event) => setDockerImage(event.target.value)} required value={dockerImage} />
            </SettingsRow>
          ) : null}
          {includesTart ? (
            <SettingsRow description="Native runs jobs directly on the macOS agent host: no VM image, TCC-gated APIs available, but jobs share the host." label="macOS execution">
              <SearchableSelect
                ariaLabel="macOS execution"
                onValueChange={(next) => setMacosRuntime(next === "native" ? "native" : "vm")}
                options={[
                  { value: "vm", label: "Virtual machine", description: "One copy-on-write Tart VM per job" },
                  { value: "native", label: "Native on host", description: "Run jobs directly on the agent host" },
                ]}
                searchable={false}
                value={macosRuntime}
              />
            </SettingsRow>
          ) : null}
          {includesTart && macosRuntime === "vm" ? (
            <SettingsRow description={bitbucketConnectionIds.length ? "Use the prepared Bitbucket runner image for this pool." : "A stopped, prepared Tart VM that each runner clones. xcodebuild needs an Xcode-ready VM."} htmlFor="tart-image" label="Tart base VM">
              <Input className="font-mono text-xs" id="tart-image" name="tartImage" onChange={(event) => setTartImage(event.target.value)} required value={tartImage} />
            </SettingsRow>
          ) : null}
          <SettingsRow description={bitbucketConnectionIds.length ? "Comma-separated. Bitbucket labels may use lowercase letters, numbers, and dots." : "Comma-separated. OS and architecture labels are added automatically."} htmlFor="pool-labels" label="Additional labels">
            <Input id="pool-labels" name="labels" onChange={(event) => setLabels(event.target.value)} placeholder={bitbucketConnectionIds.length ? "build, release.ios" : providers.length > 1 ? "build, release" : provider === "tart" ? "xcode, apple-silicon" : "docker, build"} value={labels} />
          </SettingsRow>
        </SettingsSection>

        <SettingsSection description="How many runners GridOps keeps and what each one may use." title="Capacity">
          <SettingsRow description="Autoscaling moves the target between the minimum and maximum." label="Runner counts" stacked>
            <div className="grid grid-cols-3 gap-3">
              <CountInput label="Target" name="desiredCount" onChange={setDesiredCount} value={desiredCount} />
              <CountInput defaultValue={options.defaults.minCount} label="Minimum" name="minCount" />
              <CountInput label="Maximum" min={Math.max(1, repositoryIds.length)} name="maxCount" onChange={setMaxCount} value={maxCount} />
            </div>
          </SettingsRow>
          <SettingsRow description={includesTart ? "Whole cores, because this pool includes macOS." : "Applied to each Docker runner."} htmlFor="cpu-limit" label="CPU per runner">
            <UnitField unit="cores"><Input className="pr-12" id="cpu-limit" name="cpuLimit" onChange={(event) => setCpuLimit(Number(event.target.value))} required step={includesTart ? "1" : "0.25"} type="number" value={cpuLimit} /></UnitField>
          </SettingsRow>
          <SettingsRow description="Checked against the host budget when each runner starts." htmlFor="memory-limit" label="Memory per runner">
            <UnitField unit="MB"><Input className="pr-10" id="memory-limit" name="memoryLimitMb" onChange={(event) => setMemoryLimitMb(Number(event.target.value))} required step="256" type="number" value={memoryLimitMb} /></UnitField>
          </SettingsRow>
          {resourceWarning ? (
            <div className="px-4 py-3">
              <Callout title="Target exceeds this host’s safe capacity" tone="warning">
                {resourceWarning.runnerCount} runners would request {formatResourceNumber(resourceWarning.cpuRequested)} cores and {resourceWarning.memoryRequestedMb.toLocaleString()} MB; the host safely offers {formatResourceNumber(resourceWarning.cpuBudget)} cores and {resourceWarning.memoryBudgetMb.toLocaleString()} MB. You can save, but GridOps won’t start runners beyond the budget.
              </Callout>
            </div>
          ) : null}
          <div className="px-4 py-3.5">
            <PoolPlanPreview desiredCount={desiredCount} labels={labelList} maxCount={maxCount} name={poolName} providers={providers} repositoryCount={repositoryIds.length} scope={scope} />
          </div>
        </SettingsSection>

        <SettingsSection description="Grow from the queue, shrink when idle." title="Autoscaling">
          <SettingsRow description="Queued workflow jobs raise the target up to the maximum." label="Autoscale from queued jobs">
            <Switch aria-label="Autoscale from queued jobs" defaultChecked={options.defaults.autoscalingEnabled} name="autoscalingEnabled" />
          </SettingsRow>
          <SettingsRow description="Runner slots requested per queued job, capped by the maximum." htmlFor="queue-factor" label="Runners per queued job">
            <Input defaultValue={options.defaults.queueScaleFactor} id="queue-factor" max="20" min="1" name="queueScaleFactor" required type="number" />
          </SettingsRow>
          <SettingsRow description="Once every runner is idle and nothing is queued for this long, the target returns to the minimum." htmlFor="idle-timeout" label="Idle scale-down delay">
            <UnitField unit="minutes"><Input className="pr-16" defaultValue={options.defaults.idleTimeoutMinutes} id="idle-timeout" max="1440" min="1" name="idleTimeoutMinutes" required type="number" /></UnitField>
          </SettingsRow>
        </SettingsSection>

        {error ? <Callout title="Could not create the pool" tone="danger">{error}</Callout> : null}
      </SettingsLayout>
      <div className="sticky bottom-0 z-20 border-t border-border bg-panel/90 backdrop-blur">
        <div className="mx-auto flex max-w-[720px] items-center justify-end gap-2 px-5 py-3 sm:px-8">
          <Link className={buttonVariants({ variant: "ghost" })} to="/runner-pools">Cancel</Link>
          <Button disabled={submitting || options.installations.length === 0} type="submit">
            {submitting ? <LoaderCircle className="animate-spin" /> : <Plus />}
            {submitting ? "Creating…" : "Create pool"}
          </Button>
        </div>
      </div>
    </form>
  );
}

function CountInput({ label, name, value, defaultValue, onChange, min = 0 }: { label: string; name: string; value?: number; defaultValue?: number; onChange?: (value: number) => void; min?: number }) {
  return (
    <label className="space-y-1.5">
      <span className="block text-xs text-muted-foreground">{label}</span>
      <Input defaultValue={defaultValue} max="100" min={min} name={name} onChange={onChange ? (event) => onChange(Number(event.target.value)) : undefined} required type="number" value={value} />
    </label>
  );
}
