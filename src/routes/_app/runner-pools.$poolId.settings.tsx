import { Link, createFileRoute, getRouteApi, useNavigate } from "@tanstack/react-router";
import { LoaderCircle } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { Callout } from "~/components/page";
import { PoolPlanPreview } from "~/components/pool-plan-preview";
import { SettingsLayout, SettingsRow, SettingsSection, SettingsValue, Switch, UnitField } from "~/components/settings-ui";
import { Badge } from "~/components/ui/badge";
import { Button, buttonVariants } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import { SearchableMultiSelect } from "~/components/ui/searchable-multi-select";
import { SearchableSelect } from "~/components/ui/searchable-select";
import { providerLabel } from "~/features/runner-pools/pool-actions";
import { formatResourceNumber, hostResourceWarning } from "~/features/runner-pools/resource-risk";
import {
  type RepositoryOption,
  type RunnerGroupOption,
  type RunnerPoolDetail,
  getInstallationRunnerGroups,
  getRunnerPoolRepositories,
  updateRunnerPoolAction,
} from "~/features/runner-pools/runner-pools.functions";

export const Route = createFileRoute("/_app/runner-pools/$poolId/settings")({
  component: PoolSettingsTab,
});

const poolRoute = getRouteApi("/_app/runner-pools/$poolId");

function PoolSettingsTab() {
  const pool = poolRoute.useLoaderData();
  return pool.canManage ? <RunnerPoolEditor key={`${pool.id}:${pool.configurationVersion}`} pool={pool} /> : <ReadOnlyPool pool={pool} />;
}

type LoadState<T> =
  | { status: "idle" | "loading"; items: T[]; error: null }
  | { status: "ready"; items: T[]; error: null }
  | { status: "error"; items: T[]; error: string };

function RunnerPoolEditor({ pool }: { pool: RunnerPoolDetail }) {
  const navigate = useNavigate();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<"ephemeral" | "persistent">(pool.mode);
  const [providers, setProviders] = useState<Array<"docker" | "tart">>(pool.providers?.length ? pool.providers : [pool.provider]);
  const [dockerImage, setDockerImage] = useState(pool.dockerImage);
  const [tartImage, setTartImage] = useState(pool.tartImage);
  const [macosRuntime, setMacosRuntime] = useState<"vm" | "native">(pool.macosRuntime ?? "vm");
  const [repositoryIds, setRepositoryIds] = useState(pool.repositoryIds);
  const [maxCount, setMaxCount] = useState(pool.maxCount);
  const [desiredCount, setDesiredCount] = useState(pool.desiredCount);
  const [cpuLimit, setCpuLimit] = useState(pool.cpuLimit);
  const [memoryLimitMb, setMemoryLimitMb] = useState(pool.memoryLimitMb);
  const [poolName, setPoolName] = useState(pool.name);
  const [labels, setLabels] = useState(pool.labels.join(", "));
  const [runnerGroupId, setRunnerGroupId] = useState(pool.runnerGroupId);
  const shouldLoadRepositories = pool.scope === "repository";
  const [repositoryLoad, setRepositoryLoad] = useState<LoadState<RepositoryOption>>(
    shouldLoadRepositories ? { status: "loading", items: pool.repositories, error: null } : { status: "idle", items: [], error: null },
  );
  const shouldLoadRunnerGroups = pool.scope === "organization";
  const [runnerGroupLoad, setRunnerGroupLoad] = useState<LoadState<RunnerGroupOption>>(
    shouldLoadRunnerGroups ? { status: "loading", items: [], error: null } : { status: "idle", items: [], error: null },
  );
  const primaryProvider = providers[0] ?? "docker";
  const includesTart = providers.includes("tart");
  const resourceWarning = hostResourceWarning({
    runnerCount: desiredCount,
    cpuLimit,
    memoryLimitMb,
    cpuBudget: pool.maxCpuLimit ?? 0,
    memoryBudgetMb: pool.maxMemoryLimitMb ?? 0,
  });
  const repositorySeed = pool.repositories;

  useEffect(() => {
    if (!shouldLoadRunnerGroups) return;
    const controller = new AbortController();
    void getInstallationRunnerGroups(pool.installationId, controller.signal)
      .then(({ items }) => setRunnerGroupLoad({ status: "ready", items, error: null }))
      .catch((cause: unknown) => {
        if (cause instanceof DOMException && cause.name === "AbortError") return;
        setRunnerGroupLoad({ status: "error", items: [], error: cause instanceof Error ? cause.message : "Runner groups could not be loaded." });
      });
    return () => controller.abort();
  }, [pool.installationId, shouldLoadRunnerGroups]);

  useEffect(() => {
    if (!shouldLoadRepositories) return;
    const controller = new AbortController();
    void getRunnerPoolRepositories(controller.signal)
      .then(({ items }) => setRepositoryLoad({ status: "ready", items, error: null }))
      .catch((cause: unknown) => {
        if (cause instanceof DOMException && cause.name === "AbortError") return;
        setRepositoryLoad({ status: "error", items: repositorySeed, error: cause instanceof Error ? cause.message : "Repositories could not be loaded." });
      });
    return () => controller.abort();
  }, [repositorySeed, shouldLoadRepositories]);

  const runnerGroups = runnerGroupLoad.items;
  const selectedRepositories = repositoryLoad.items.filter((repository) => repositoryIds.includes(repository.id));
  const selectedAccounts = [...new Set(selectedRepositories.map((repository) => repository.accountLogin))];
  const labelList = labels.split(",").map((label) => label.trim()).filter(Boolean);

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
          name: poolName,
          mode,
          provider: primaryProvider,
          providers,
          labels: labelList,
          image: primaryProvider === "tart" ? tartImage : dockerImage,
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
          runnerGroupId: pool.scope === "organization" ? runnerGroupId || pool.runnerGroupId : 1,
        },
      });
      await navigate({ to: "/runner-pools/$poolId", params: { poolId: pool.id } });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Runner pool update failed.");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <form className="contents" onSubmit={submit}>
      <SettingsLayout
        description={<>Generation {pool.configurationVersion}. Changing the runtime starts a rolling replacement: busy runners finish their jobs, and idle runners are replaced one at a time.</>}
        title="Configuration"
      >
        <SettingsSection description="Where this pool’s runners register." title="Destination">
          <SettingsRow label={pool.scope === "repository" ? "GitHub accounts" : "Installation"}>
            <span className="text-sm text-foreground">{pool.scope === "repository" ? selectedAccounts.join(", ") || pool.accountLogin : pool.accountLogin}</span>
          </SettingsRow>
          {pool.scope === "repository" ? (
            <SettingsRow description={`${repositoryIds.length} selected · the maximum runner count must cover every repository.`} label="Repositories" stacked>
              <SearchableMultiSelect
                ariaLabel="Pool repositories"
                emptyMessage="No repositories match this search"
                loading={repositoryLoad.status === "loading"}
                maxSelected={maxCount}
                onValueChange={setRepositoryIds}
                options={repositoryLoad.items.map((repository) => ({
                  value: repository.id,
                  label: repository.fullName,
                  description: `${repository.accountLogin} · ${repository.private ? "Private" : "Public"}`,
                  keywords: [repository.accountLogin, repository.accountType],
                }))}
                placeholder="Choose one or more repositories…"
                searchPlaceholder="Search by owner or repository name…"
                selectedNoun="repositories"
                values={repositoryIds}
              />
              {repositoryLoad.status === "error" ? <p className="mt-1.5 text-xs text-danger">{repositoryLoad.error}</p> : null}
            </SettingsRow>
          ) : (
            <SettingsRow
              description={runnerGroupLoad.status === "loading" ? "Loading runner groups from GitHub…" : runnerGroups.length ? "Repository access is controlled by this GitHub runner group." : "Enter the GitHub runner group ID."}
              label="Runner group"
            >
              {runnerGroupLoad.status === "loading" ? (
                <div className="flex h-8 items-center gap-2 text-sm text-muted-foreground" role="status"><LoaderCircle className="size-4 animate-spin" />Loading…</div>
              ) : runnerGroups.length ? (
                <SearchableSelect
                  ariaLabel="GitHub runner group"
                  onValueChange={(next) => setRunnerGroupId(next ?? pool.runnerGroupId)}
                  options={runnerGroups.map((group) => ({ value: group.id, label: group.name, description: group.isDefault ? "Default runner group" : `${group.visibility} visibility` }))}
                  placeholder="Choose runner group…"
                  searchPlaceholder="Search runner groups…"
                  value={runnerGroupId}
                />
              ) : (
                <>
                  <Input min="1" name="runnerGroupId" onChange={(event) => setRunnerGroupId(Number(event.target.value))} required type="number" value={runnerGroupId} />
                  {runnerGroupLoad.status === "error" ? <p className="mt-1.5 text-left text-xs text-danger">{runnerGroupLoad.error}</p> : null}
                </>
              )}
            </SettingsRow>
          )}
        </SettingsSection>

        <SettingsSection description="What each runner is and how jobs find it." title="Runner definition">
          <SettingsRow description="Also registered as a runner label." htmlFor="pool-name" label="Pool name">
            <Input id="pool-name" name="name" onChange={(event) => setPoolName(event.target.value)} pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" required value={poolName} />
          </SettingsRow>
          <SettingsRow description="Jobs route to the first compatible provider; the first one also takes jobs that only ask for self-hosted." label="Providers">
            <SearchableMultiSelect
              ariaLabel="Runner providers"
              maxSelected={2}
              onValueChange={(values) => {
                const selected = values.filter((value): value is "docker" | "tart" => value === "docker" || value === "tart");
                if (!selected.length) return;
                setProviders(selected);
                if (selected.includes("tart")) setMode("ephemeral");
              }}
              options={[
                { value: "docker", label: "Linux · Docker", description: "Fast Linux containers" },
                { value: "tart", label: "macOS · Tart", description: "macOS VMs or native host execution" },
              ]}
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
              <Input className="font-mono text-xs" id="docker-image" onChange={(event) => setDockerImage(event.target.value)} required value={dockerImage} />
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
            <SettingsRow description="A stopped, prepared Tart VM that each runner clones. xcodebuild needs an Xcode-ready VM." htmlFor="tart-image" label="Tart base VM">
              <Input className="font-mono text-xs" id="tart-image" onChange={(event) => setTartImage(event.target.value)} required value={tartImage} />
            </SettingsRow>
          ) : null}
          <SettingsRow description="Comma-separated. self-hosted, OS, architecture, and the pool name are added automatically." htmlFor="pool-labels" label="Additional labels">
            <Input id="pool-labels" name="labels" onChange={(event) => setLabels(event.target.value)} placeholder="gpu, large" value={labels} />
          </SettingsRow>
        </SettingsSection>

        <SettingsSection description="How many runners GridOps keeps and what each one may use." title="Capacity">
          <SettingsRow description="Autoscaling moves the target between the minimum and maximum." label="Runner counts" stacked>
            <div className="grid grid-cols-3 gap-3">
              <NumberInput label="Target" onChange={setDesiredCount} name="desiredCount" value={desiredCount} />
              <NumberInput defaultValue={pool.minCount} label="Minimum" name="minCount" />
              <NumberInput label="Maximum" min={Math.max(1, repositoryIds.length)} name="maxCount" onChange={setMaxCount} value={maxCount} />
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
            <PoolPlanPreview desiredCount={desiredCount} labels={labelList} maxCount={maxCount} name={poolName} providers={providers} repositoryCount={repositoryIds.length} scope={pool.scope} />
          </div>
        </SettingsSection>

        <SettingsSection description="Grow from the queue, shrink when idle." title="Autoscaling">
          <SettingsRow description="Queued workflow jobs raise the target up to the maximum." label="Autoscale from queued jobs">
            <Switch aria-label="Autoscale from queued jobs" defaultChecked={pool.autoscalingEnabled} name="autoscalingEnabled" />
          </SettingsRow>
          <SettingsRow description="Runner slots requested per queued job, capped by the maximum." htmlFor="queue-factor" label="Runners per queued job">
            <Input defaultValue={pool.queueScaleFactor} id="queue-factor" max="20" min="1" name="queueScaleFactor" required type="number" />
          </SettingsRow>
          <SettingsRow description="Once every runner is idle and nothing is queued for this long, the target returns to the minimum." htmlFor="idle-timeout" label="Idle scale-down delay">
            <UnitField unit="minutes"><Input className="pr-16" defaultValue={pool.idleTimeoutMinutes} id="idle-timeout" max="1440" min="1" name="idleTimeoutMinutes" required type="number" /></UnitField>
          </SettingsRow>
        </SettingsSection>

        {error ? <Callout title="Could not save" tone="danger">{error}</Callout> : null}
      </SettingsLayout>
      <div className="sticky bottom-0 z-20 border-t border-border bg-panel/90 backdrop-blur">
        <div className="mx-auto flex max-w-[720px] items-center justify-end gap-2 px-5 py-3 sm:px-8">
          <Link className={buttonVariants({ variant: "ghost" })} params={{ poolId: pool.id }} to="/runner-pools/$poolId">Cancel</Link>
          <Button disabled={submitting} type="submit">{submitting ? <LoaderCircle className="animate-spin" /> : null}{submitting ? "Saving…" : "Save changes"}</Button>
        </div>
      </div>
    </form>
  );
}

function NumberInput({ label, name, value, defaultValue, onChange, min = 0 }: { label: string; name: string; value?: number; defaultValue?: number; onChange?: (value: number) => void; min?: number }) {
  return (
    <label className="space-y-1.5">
      <span className="block text-xs text-muted-foreground">{label}</span>
      <Input
        defaultValue={defaultValue}
        max="100"
        min={min}
        name={name}
        onChange={onChange ? (event) => onChange(Number(event.target.value)) : undefined}
        required
        type="number"
        value={value}
      />
    </label>
  );
}

function ReadOnlyPool({ pool }: { pool: RunnerPoolDetail }) {
  const providers = pool.providers?.length ? pool.providers : [pool.provider];
  return (
    <SettingsLayout actions={<Badge variant="outline">Read only</Badge>} description="An installation administrator manages this pool." title="Configuration">
      <SettingsSection title="Runner definition">
        <SettingsValue label="Destination" value={pool.scope === "repository" ? `${pool.repositoryIds.length} repositories` : pool.accountLogin} />
        <SettingsValue label="Providers" value={providers.map((provider) => providerLabel(provider, pool.macosRuntime)).join(" + ")} />
        <SettingsValue label="Mode" value={pool.mode} />
        {providers.includes("docker") ? <SettingsValue label="Docker image" mono value={pool.dockerImage} /> : null}
        {providers.includes("tart") && pool.macosRuntime !== "native" ? <SettingsValue label="Tart base VM" mono value={pool.tartImage} /> : null}
      </SettingsSection>
      <SettingsSection title="Capacity">
        <SettingsValue label="Runners" value={`${pool.desiredCount} target · ${pool.minCount}–${pool.maxCount}`} />
        <SettingsValue label="Per runner" value={`${pool.cpuLimit} cores · ${pool.memoryLimitMb} MB`} />
        <SettingsValue label="Autoscaling" value={pool.autoscalingEnabled ? `On · ${pool.queueScaleFactor} per queued job · idle after ${pool.idleTimeoutMinutes}m` : "Off"} />
      </SettingsSection>
    </SettingsLayout>
  );
}
