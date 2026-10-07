import { createFileRoute, useRouter } from "@tanstack/react-router";
import { ExternalLink, KeyRound, LoaderCircle, LogIn } from "lucide-react";
import { type FormEvent, type KeyboardEvent, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { Callout, InlineError } from "~/components/page";
import { SettingsLayout, SettingsRow, SettingsSection, SettingsValue, UnitField } from "~/components/settings-ui";
import { StatusDot } from "~/components/status-icon";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import { SearchableSelect } from "~/components/ui/searchable-select";
import {
  DAILY_LIMIT_MAX,
  DAILY_LIMIT_MIN,
  connectionAccount,
  connectionStatusLabel,
  connectionTone,
  effortForModel,
  isApiKeyProvider,
  isSubscriptionProvider,
  modelChoices,
  parseDailyLimit,
  sentenceCase,
} from "~/features/agent/agent-state";
import {
  type AgentGitHubPermissions,
  type AgentTriggerMode,
  type AiApiKeyProviderId,
  type AiConnection,
  type AiModel,
  type AiOAuthAttempt,
  type AiProviderOption,
  type AiSettings,
  type AiSubscriptionProviderId,
  completeAiOAuth,
  connectAiApiKey,
  getAiConnectionModels,
  getAiSettings,
  removeAiConnection,
  saveAgentSettings,
  startAiOAuth,
} from "~/features/agent/agent.functions";
import { useAction } from "~/lib/use-action";
import { cn, formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/settings/ai")({
  loader: () => getAiSettings(),
  component: AiAgentSettings,
});

const TRIGGERS: Array<{ value: AgentTriggerMode; label: string; description: string }> = [
  { value: "manual", label: "Button only", description: "An admin starts a fix from a failed job." },
  { value: "automatic", label: "Automatically on failure", description: "GridOps starts a fix for each failed job, up to the daily limit." },
];

const SIGN_IN: Record<AiSubscriptionProviderId, { action: string; field: string; placeholder: string }> = {
  codex: { action: "Sign in with ChatGPT", field: "Redirected URL", placeholder: "http://localhost:1455/auth/callback?code=…" },
  claude_code: { action: "Sign in with Claude", field: "Authorization code", placeholder: "Paste the code shown after signing in" },
};

function AiAgentSettings() {
  const settings = Route.useLoaderData();
  const { canManage, connections, agent, configured, github } = settings;
  const ready = connections.filter((connection) => connection.status === "ready");
  // Re-seed the agent form whenever the saved settings or usable providers change.
  const formKey = JSON.stringify([agent, ready.map((connection) => connection.id)]);

  return (
    <SettingsLayout
      description="An agent diagnoses a failed GitHub Actions job in an isolated sandbox, then opens a draft pull request with a fix, or explains the failure when it isn’t a code problem."
      title="AI agent"
    >
      {!github.ready ? <GitHubPermissionsCallout github={github} /> : null}
      {!canManage ? <Callout title="Read-only access">A GridOps administrator connects AI providers and configures the fix agent.</Callout> : null}
      {!configured ? <Callout title="Fix with agent is hidden">The button stays off failed jobs until a provider and model are saved below.</Callout> : null}

      <SettingsSection title={`Providers${connections.length ? ` · ${connections.length}` : ""}`}>
        {connections.length ? connections.map((connection) => (
          <ConnectionRow canManage={canManage} connection={connection} inUse={agent.connectionId === connection.id} key={connection.id} />
        )) : <div className="px-4 py-6 text-center text-sm text-muted-foreground">No AI providers are connected yet.</div>}
      </SettingsSection>

      {canManage ? <ConnectProviderSection providers={settings.providers} /> : null}

      {canManage ? <AgentSettingsForm key={formKey} settings={settings} /> : <AgentSettingsSummary settings={settings} />}
    </SettingsLayout>
  );
}

function GitHubPermissionsCallout({ github }: { github: AgentGitHubPermissions }) {
  return (
    <Callout title="The GitHub App can’t open fixes yet" tone="warning">
      <p>
        To open pull requests, the GridOps GitHub App needs <span className="font-medium text-foreground">Contents: read and write</span> and{" "}
        <span className="font-medium text-foreground">Pull requests: read and write</span>.
      </p>
      {github.missing.length ? (
        <ul className="mt-1.5 space-y-0.5">
          {github.missing.map((installation) => (
            <li key={installation.installationId}>
              <span className="font-medium text-foreground">{installation.account}</span>
              {installation.permissions.length ? <> is missing {installation.permissions.map(sentenceCase).join(", ")}</> : null}
            </li>
          ))}
        </ul>
      ) : null}
      {github.appSettingsUrl ? (
        <p className="mt-1.5">
          <a className="inline-flex items-center gap-1 font-medium text-foreground underline-offset-2 hover:underline" href={github.appSettingsUrl} rel="noreferrer" target="_blank">
            Update the App’s permissions on GitHub<ExternalLink className="size-3" />
          </a>, then approve the change for each installation.
        </p>
      ) : null}
    </Callout>
  );
}

function ConnectionRow({ connection, inUse, canManage }: { connection: AiConnection; inUse: boolean; canManage: boolean }) {
  const run = useAction();
  const notReady = connection.status !== "ready";
  return (
    <div className="flex items-center gap-3 px-4 py-3">
      <StatusDot tone={connectionTone(connection.status)} />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm font-medium">{connection.providerName}</span>
          {inUse ? <Badge variant="default">Fix agent</Badge> : null}
        </div>
        <div className="truncate text-xs text-muted-foreground">
          {connection.authMethod === "api_key" ? "API key" : "Subscription"} ·{" "}
          <span className={cn(!connection.accountLabel && connection.fingerprint && "font-mono")}>{connectionAccount(connection)}</span>
          {" "}· connected {formatRelativeTime(connection.createdAt)}
        </div>
        {notReady ? (
          <div className={cn("mt-0.5 text-xs leading-5", connection.status === "error" ? "text-danger" : "text-warning")}>
            {connectionStatusLabel(connection.status)}{connection.statusDetail ? <span className="text-muted-foreground"> · {connection.statusDetail}</span> : null}
          </div>
        ) : null}
      </div>
      {canManage ? (
        <Button
          onClick={() => void run({
            action: () => removeAiConnection(connection.id),
            confirm: `Remove ${connection.providerName}? ${inUse ? "The fix agent uses it, so Fix with agent is hidden until you choose another provider." : "GridOps deletes its stored credentials."}`,
            success: `${connection.providerName} removed.`,
          })}
          size="sm"
          variant="ghost"
        >
          Remove
        </Button>
      ) : null}
    </div>
  );
}

function ConnectProviderSection({ providers }: { providers: AiProviderOption[] }) {
  const subscriptions = providers.filter((provider) => provider.authMethod === "subscription_oauth");
  const apiKeys = providers.filter((provider) => provider.authMethod === "api_key");
  return (
    <SettingsSection
      description="Sign in with a ChatGPT or Claude subscription, or use an API key. Credentials are encrypted at rest and never returned to the browser."
      title="Connect a provider"
    >
      {subscriptions.map((provider) => isSubscriptionProvider(provider.id) ? <SubscriptionConnect key={provider.id} provider={provider} providerId={provider.id} /> : null)}
      {apiKeys.length ? <ApiKeyConnect providers={apiKeys} /> : null}
    </SettingsSection>
  );
}

/** Start → sign in on the provider's page in a new tab → paste the result back → complete. */
function SubscriptionConnect({ provider, providerId }: { provider: AiProviderOption; providerId: AiSubscriptionProviderId }) {
  const router = useRouter();
  const [attempt, setAttempt] = useState<AiOAuthAttempt | null>(null);
  const [callbackValue, setCallbackValue] = useState("");
  const [pending, setPending] = useState<"start" | "complete" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const copy = SIGN_IN[providerId];
  const fieldId = `ai-oauth-${providerId}`;

  async function start() {
    // Open the tab during the click: browsers block a window opened after an await.
    const tab = window.open("", "_blank");
    setPending("start");
    setError(null);
    try {
      const next = await startAiOAuth({ provider: providerId });
      setAttempt(next);
      setCallbackValue("");
      if (tab) {
        tab.opener = null;
        tab.location.href = next.authorizeUrl;
      }
    } catch (cause) {
      tab?.close();
      setError(cause instanceof Error ? cause.message : `Could not start signing in to ${provider.name}.`);
    } finally {
      setPending(null);
    }
  }

  async function complete(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!attempt) return;
    setPending("complete");
    setError(null);
    try {
      const connection = await completeAiOAuth({ attemptId: attempt.attemptId, callbackValue: callbackValue.trim() });
      setAttempt(null);
      setCallbackValue("");
      toast.success(`${connection.providerName} connected.`);
      await router.invalidate();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : `Could not finish signing in to ${provider.name}.`);
    } finally {
      setPending(null);
    }
  }

  function cancel() {
    setAttempt(null);
    setCallbackValue("");
    setError(null);
  }

  return (
    <div>
      <SettingsRow description={provider.description} label={provider.name}>
        {attempt ? null : (
          <Button disabled={pending === "start"} onClick={() => void start()} variant="outline">
            {pending === "start" ? <LoaderCircle className="animate-spin" /> : <LogIn />}{copy.action}
          </Button>
        )}
      </SettingsRow>
      {attempt ? (
        <form className="space-y-3 px-4 pb-4" onSubmit={complete}>
          <div className="rounded-md border border-border bg-panel-subtle px-3 py-2.5">
            <p className="whitespace-pre-wrap text-xs leading-5 text-secondary-foreground">{attempt.instructions}</p>
            <p className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-2xs text-muted-foreground">
              <a className="inline-flex items-center gap-1 font-medium text-foreground underline-offset-2 hover:underline" href={attempt.authorizeUrl} rel="noreferrer" target="_blank">
                Open the sign-in page<ExternalLink className="size-3" />
              </a>
              <span>Expires {new Date(attempt.expiresAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>
            </p>
          </div>
          <div>
            <label className="mb-1.5 block text-xs font-medium text-foreground" htmlFor={fieldId}>{copy.field}</label>
            <Input
              autoCapitalize="none"
              autoComplete="off"
              autoCorrect="off"
              className="font-mono text-xs"
              id={fieldId}
              onChange={(event) => setCallbackValue(event.target.value)}
              placeholder={copy.placeholder}
              required
              spellCheck={false}
              value={callbackValue}
            />
          </div>
          {error ? <Callout title="Could not connect" tone="danger">{error}</Callout> : null}
          <div className="flex justify-end gap-2">
            <Button onClick={cancel} variant="ghost">Cancel</Button>
            <Button disabled={pending === "complete" || !callbackValue.trim()} type="submit">
              {pending === "complete" ? <LoaderCircle className="animate-spin" /> : null}{pending === "complete" ? "Connecting…" : "Finish sign-in"}
            </Button>
          </div>
        </form>
      ) : error ? <div className="px-4 pb-3"><Callout title="Could not start signing in" tone="danger">{error}</Callout></div> : null}
    </div>
  );
}

function ApiKeyConnect({ providers }: { providers: AiProviderOption[] }) {
  const router = useRouter();
  const options = providers.flatMap((provider) => isApiKeyProvider(provider.id) ? [{ ...provider, id: provider.id }] : []);
  const [providerId, setProviderId] = useState<AiApiKeyProviderId | null>(options[0]?.id ?? null);
  const [apiKey, setApiKey] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const selected = options.find((option) => option.id === providerId);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!providerId) return;
    setSubmitting(true);
    setError(null);
    try {
      const connection = await connectAiApiKey({ provider: providerId, apiKey: apiKey.trim() });
      setApiKey("");
      toast.success(`${connection.providerName} connected.`);
      await router.invalidate();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The API key could not be saved.");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <form className="divide-y divide-border" onSubmit={submit}>
      <SettingsRow description={selected?.description ?? "Pay per use with a provider API key."} label="API key provider">
        <SearchableSelect
          ariaLabel="API key provider"
          onValueChange={(next) => {
            setProviderId(next ?? options[0]?.id ?? null);
            setError(null);
          }}
          options={options.map((option) => ({ value: option.id, label: option.name }))}
          searchable={false}
          value={providerId}
        />
      </SettingsRow>
      <SettingsRow description="GridOps checks the key with the provider before saving it." htmlFor="ai-api-key" label="API key">
        <Input autoComplete="off" id="ai-api-key" onChange={(event) => setApiKey(event.target.value)} placeholder={selected?.keyHint ?? "Paste an API key"} required spellCheck={false} type="password" value={apiKey} />
      </SettingsRow>
      {error ? <div className="px-4 py-3"><Callout title="Could not connect" tone="danger">{error}</Callout></div> : null}
      <div className="flex justify-end px-4 py-3">
        <Button disabled={submitting || !providerId || !apiKey.trim()} type="submit">
          {submitting ? <LoaderCircle className="animate-spin" /> : <KeyRound />}{submitting ? "Verifying…" : "Verify and save"}
        </Button>
      </div>
    </form>
  );
}

type ModelLoad =
  | { status: "idle"; items: AiModel[]; error: null }
  | { status: "loading"; items: AiModel[]; error: null }
  | { status: "ready"; items: AiModel[]; error: null }
  | { status: "error"; items: AiModel[]; error: string };

function AgentSettingsForm({ settings }: { settings: AiSettings }) {
  const router = useRouter();
  const { agent, connections } = settings;
  const ready = connections.filter((connection) => connection.status === "ready");
  const savedConnection = connections.find((connection) => connection.id === agent.connectionId);
  const savedUsable = savedConnection?.status === "ready";
  const [connectionId, setConnectionId] = useState<string | null>(savedUsable ? savedConnection.id : agent.connectionId ? null : ready[0]?.id ?? null);
  // The saved model belongs to the saved provider; any other choice starts fresh.
  const [choice, setChoice] = useState<{ model: string | null; effort: string | null }>(
    connectionId && connectionId === agent.connectionId ? { model: agent.model, effort: agent.reasoningEffort } : { model: null, effort: null },
  );
  const [triggerMode, setTriggerMode] = useState<AgentTriggerMode>(agent.triggerMode);
  const [dailyLimit, setDailyLimit] = useState(String(agent.dailyLimit));
  const [modelLoad, setModelLoad] = useState<ModelLoad>(connectionId ? { status: "loading", items: [], error: null } : { status: "idle", items: [], error: null });
  const [modelAttempt, setModelAttempt] = useState(0);
  const [saving, setSaving] = useState(false);
  const triggerButtons = useRef<Array<HTMLButtonElement | null>>([]);
  const selectedConnection = ready.find((connection) => connection.id === connectionId);
  const models = modelChoices(modelLoad.items, choice.model);
  const selectedModel = models.find((model) => model.id === choice.model);
  const efforts = selectedModel?.reasoningEfforts ?? [];

  useEffect(() => {
    if (!connectionId) return undefined;
    const controller = new AbortController();
    void getAiConnectionModels(connectionId, controller.signal)
      .then(({ models: items }) => {
        setModelLoad({ status: "ready", items, error: null });
        // Pick the first model for a fresh provider; settle the effort the chosen model supports.
        setChoice((current) => {
          const model = current.model ?? items[0]?.id ?? null;
          return { model, effort: effortForModel(items.find((item) => item.id === model), current.effort) };
        });
      })
      .catch((cause: unknown) => {
        if (cause instanceof DOMException && cause.name === "AbortError") return;
        setModelLoad({ status: "error", items: [], error: cause instanceof Error ? cause.message : "Models could not be loaded." });
      });
    return () => controller.abort();
  }, [connectionId, modelAttempt]);

  function chooseConnection(next: string | null) {
    setConnectionId(next);
    setChoice(next && next === agent.connectionId ? { model: agent.model, effort: agent.reasoningEffort } : { model: null, effort: null });
    setModelLoad(next ? { status: "loading", items: [], error: null } : { status: "idle", items: [], error: null });
  }

  function chooseModel(next: string | null) {
    const listed = modelLoad.items.find((model) => model.id === next);
    // A saved model the provider didn't list keeps its saved effort.
    const fallback = !listed && next === agent.model && connectionId === agent.connectionId ? agent.reasoningEffort : null;
    setChoice((current) => ({ model: next, effort: listed ? effortForModel(listed, current.effort) : fallback }));
  }

  function moveTrigger(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) return;
    event.preventDefault();
    const step = event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 1;
    const nextIndex = (index + step + TRIGGERS.length) % TRIGGERS.length;
    const next = TRIGGERS[nextIndex];
    if (!next) return;
    setTriggerMode(next.value);
    triggerButtons.current[nextIndex]?.focus();
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const limit = parseDailyLimit(dailyLimit);
    if (triggerMode === "automatic" && limit === null) {
      toast.error(`Daily limit must be a whole number from ${DAILY_LIMIT_MIN} to ${DAILY_LIMIT_MAX}.`);
      return;
    }
    setSaving(true);
    try {
      await saveAgentSettings({
        connectionId,
        model: connectionId ? choice.model : null,
        reasoningEffort: connectionId && choice.model ? choice.effort : null,
        triggerMode,
        dailyLimit: limit ?? agent.dailyLimit,
      });
      toast.success("Fix agent settings saved.");
      await router.invalidate();
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : "Could not save the fix agent settings.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <form onSubmit={submit}>
      <SettingsSection description="How the agent runs when a job fails." title="Fix agent">
        <SettingsRow
          description={savedConnection && !savedUsable && !connectionId
            ? `${savedConnection.providerName} needs reconnecting. Choose another provider or sign in again.`
            : ready.length ? "The provider the agent signs in with." : "Connect a provider above first."}
          label="Provider"
        >
          <SearchableSelect
            ariaLabel="AI provider"
            disabled={!ready.length}
            onValueChange={chooseConnection}
            options={ready.map((connection) => ({ value: connection.id, label: connection.providerName, description: connectionAccount(connection) }))}
            placeholder={ready.length ? "Choose a provider…" : "No providers connected"}
            searchable={false}
            value={connectionId}
          />
        </SettingsRow>
        <SettingsRow
          description={modelLoad.status === "loading" ? `Loading models from ${selectedConnection?.providerName ?? "the provider"}…` : "The model the agent uses to investigate and write the fix."}
          label="Model"
        >
          <SearchableSelect
            ariaLabel="Model"
            disabled={!connectionId}
            emptyMessage="This provider lists no models"
            loading={modelLoad.status === "loading"}
            onValueChange={chooseModel}
            options={models.map((model) => ({ value: model.id, label: model.name, description: model.name === model.id ? undefined : model.id }))}
            placeholder={connectionId ? "Choose a model…" : "Choose a provider first"}
            searchPlaceholder="Search models…"
            searchable={models.length > 8}
            value={choice.model}
          />
          {modelLoad.status === "error" ? (
            <InlineError
              onRetry={() => {
                setModelLoad({ status: "loading", items: [], error: null });
                setModelAttempt((attempt) => attempt + 1);
              }}
              title="Couldn’t load models"
            >
              {modelLoad.error}
            </InlineError>
          ) : null}
        </SettingsRow>
        {efforts.length ? (
          <SettingsRow description="Higher effort thinks longer before acting." label="Reasoning effort">
            <SearchableSelect
              ariaLabel="Reasoning effort"
              onValueChange={(next) => setChoice((current) => ({ ...current, effort: next }))}
              options={efforts.map((effort) => ({ value: effort, label: sentenceCase(effort), description: effort === selectedModel?.defaultEffort ? "Default" : undefined }))}
              searchable={false}
              value={choice.effort}
            />
          </SettingsRow>
        ) : null}
        <SettingsRow label="Trigger" stacked>
          <div aria-label="Trigger" className="grid gap-2 sm:grid-cols-2" role="radiogroup">
            {TRIGGERS.map((option, index) => {
              const checked = triggerMode === option.value;
              return (
                <button
                  aria-checked={checked}
                  className={cn(
                    "flex flex-col rounded-lg border px-3 py-2.5 text-left transition-[border-color,box-shadow] duration-150 ease-out-strong",
                    checked ? "border-primary ring-2 ring-primary/25" : "border-border-strong hover:border-faint",
                  )}
                  key={option.value}
                  onClick={() => setTriggerMode(option.value)}
                  onKeyDown={(event) => moveTrigger(event, index)}
                  ref={(element) => {
                    triggerButtons.current[index] = element;
                  }}
                  role="radio"
                  tabIndex={checked ? 0 : -1}
                  type="button"
                >
                  <span className="flex items-center justify-between gap-2 text-sm font-medium text-foreground">
                    {option.label}
                    {checked ? <span aria-hidden="true" className="size-2 shrink-0 rounded-full bg-primary" /> : null}
                  </span>
                  <span className="mt-0.5 block text-xs leading-5 text-muted-foreground">{option.description}</span>
                </button>
              );
            })}
          </div>
        </SettingsRow>
        {triggerMode === "automatic" ? (
          <SettingsRow description="The most fixes GridOps starts on its own in a day." htmlFor="agent-daily-limit" label="Daily limit">
            <UnitField unit="runs a day">
              <Input
                className="pr-24"
                id="agent-daily-limit"
                max={DAILY_LIMIT_MAX}
                min={DAILY_LIMIT_MIN}
                onChange={(event) => setDailyLimit(event.target.value)}
                required
                step={1}
                type="number"
                value={dailyLimit}
              />
            </UnitField>
          </SettingsRow>
        ) : null}
        <div className="flex justify-end px-4 py-3">
          <Button disabled={saving || modelLoad.status === "loading"} type="submit">{saving ? <LoaderCircle className="animate-spin" /> : null}{saving ? "Saving…" : "Save changes"}</Button>
        </div>
      </SettingsSection>
    </form>
  );
}

function AgentSettingsSummary({ settings }: { settings: AiSettings }) {
  const { agent, connections } = settings;
  const connection = connections.find((candidate) => candidate.id === agent.connectionId);
  return (
    <SettingsSection description="How the agent runs when a job fails." title="Fix agent">
      <SettingsValue label="Provider" value={connection?.providerName ?? "—"} />
      <SettingsValue label="Model" mono={Boolean(agent.model)} value={agent.model ?? "—"} />
      {agent.reasoningEffort ? <SettingsValue label="Reasoning effort" value={sentenceCase(agent.reasoningEffort)} /> : null}
      <SettingsValue label="Trigger" value={TRIGGERS.find((option) => option.value === agent.triggerMode)?.label ?? sentenceCase(agent.triggerMode)} />
      {agent.triggerMode === "automatic" ? <SettingsValue label="Daily limit" value={`${agent.dailyLimit} ${agent.dailyLimit === 1 ? "run" : "runs"} a day`} /> : null}
    </SettingsSection>
  );
}
