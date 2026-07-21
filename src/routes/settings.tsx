import { createFileRoute, useRouter } from "@tanstack/react-router";
import { CheckCircle2, CircleX, DatabaseBackup, Github, LoaderCircle, Save, Settings, ShieldCheck } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";
import { toast } from "sonner";

import { ResourcePage } from "~/components/resource-page";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Input } from "~/components/ui/input";
import { createGitHubAppManifestAction, getSettingsPage, saveSettingsAction } from "~/features/operations/operations.functions";

export const Route = createFileRoute("/settings")({
  loader: () => getSettingsPage(),
  component: SettingsPage,
});

function SettingsPage() {
  const page = Route.useLoaderData();
  if (!page.authenticated || !page.data) {
    return <ResourcePage title="Settings" description="Configure GitHub, runners, retention, and system policy." icon={Settings} emptyTitle="Connect GitHub to finish setup" emptyDescription="Authenticate before viewing operational configuration and host health." />;
  }

  return <AuthenticatedSettings data={page.data} />;
}

function AuthenticatedSettings({ data }: { data: NonNullable<Extract<ReturnType<typeof Route.useLoaderData>, { authenticated: true }>['data']> }) {
  const save = saveSettingsAction;
  const router = useRouter();
  const [pending, setPending] = useState(false);
  const [manifestPending, setManifestPending] = useState(false);
  const [appOwnerType, setAppOwnerType] = useState<"user" | "organization">("user");
  const [appOrganization, setAppOrganization] = useState("");
  const [appName, setAppName] = useState("GridOps Self-Hosted");

  useEffect(() => {
    const search = new URLSearchParams(window.location.search);
    const appError = search.get("appError");
    if (search.get("appCreated") === "1") toast.success("GitHub App credentials were encrypted and activated.");
    if (appError) toast.error(appError);
    if (search.has("appCreated") || search.has("appError")) {
      window.history.replaceState({}, "", window.location.pathname);
    }
  }, []);

  async function createGitHubApp() {
    setManifestPending(true);
    try {
      const setup = await createGitHubAppManifestAction({ data: {
        ownerType: appOwnerType,
        organization: appOwnerType === "organization" ? appOrganization.trim() : undefined,
        name: appName.trim() || undefined,
      } });
      const form = document.createElement("form");
      form.action = `${setup.action}?state=${encodeURIComponent(setup.state)}`;
      form.method = "post";
      const manifest = document.createElement("input");
      manifest.type = "hidden";
      manifest.name = "manifest";
      manifest.value = setup.manifest;
      form.append(manifest);
      document.body.append(form);
      form.submit();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Could not start GitHub App setup.");
      setManifestPending(false);
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    const form = new FormData(event.currentTarget);
    try {
      await save({ data: {
        logRetentionDays: Number(form.get("logRetentionDays")),
        webhookRetentionDays: Number(form.get("webhookRetentionDays")),
        auditRetentionDays: Number(form.get("auditRetentionDays")),
        reconcileIntervalSeconds: Number(form.get("reconcileIntervalSeconds")),
        autoUpdateImages: form.get("autoUpdateImages") === "on",
      } });
      toast.success("GridOps policy saved.");
      await router.invalidate();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Could not save settings.");
    } finally {
      setPending(false);
    }
  }

  const checks = [
    ["GitHub OAuth", data.configuration.githubOAuth, "Client ID and secret"],
    ["Runner control", data.configuration.githubAppControl, "GitHub runner API access"],
    ["Webhook verification", data.configuration.webhookVerification, "HMAC signature secret"],
    ["Encrypted storage", data.configuration.secureStorage, "Session and AES keys"],
    ["Manager authentication", data.configuration.runnerManager, "Internal bearer token"],
  ] as const;

  return (
    <ResourcePage title="Settings" description="Configure GitHub, runner defaults, retention, backups, and system policy." icon={Settings} emptyTitle="Settings unavailable" emptyDescription="Connect GitHub to manage GridOps.">
      <div className="grid gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader><div><CardTitle>Security and integrations</CardTitle><p className="mt-1 text-xs text-muted-foreground">Secrets come from the host environment or encrypted runtime storage and are never rendered here.</p></div><ShieldCheck className="size-5 text-emerald-400" /></CardHeader>
          <CardContent className="space-y-2">
            {checks.map(([label, ready, detail]) => (
              <div className="flex items-center gap-3 rounded-md border border-border p-3" key={label}>
                {ready ? <CheckCircle2 className="size-4 text-emerald-400" /> : <CircleX className="size-4 text-amber-400" />}
                <div className="min-w-0 flex-1"><div className="text-sm font-medium">{label}</div><div className="mt-0.5 text-[11px] text-muted-foreground">{detail}</div></div>
                <Badge variant={ready ? "success" : "warning"}>{ready ? "configured" : "required"}</Badge>
              </div>
            ))}
            <div className="mt-4 space-y-3 rounded-md bg-muted/25 p-3 text-xs">
              <CopyRow label="OAuth callback" value={data.configuration.callbackUrl} />
              <CopyRow label="Webhook URL" value={data.configuration.webhookUrl} />
            </div>
            {!data.configuration.githubAppControl || !data.configuration.webhookVerification ? (
              <div className="mt-4 rounded-md border border-amber-500/20 bg-amber-500/5 p-4">
                <div className="text-sm font-medium">Finish GitHub App setup</div>
                <p className="mt-1 text-xs leading-5 text-muted-foreground">
                  GridOps can create a private GitHub App with the runner, Actions, installation, and webhook permissions it needs. GitHub returns the private key and webhook secret directly to this instance, where they are encrypted at rest.
                </p>
                <div className="mt-3 grid gap-3 sm:grid-cols-2">
                  <label className="space-y-2"><span className="block text-[11px] font-medium">App owner</span><select className="gridops-select" value={appOwnerType} onChange={(event) => setAppOwnerType(event.target.value as typeof appOwnerType)}><option value="user">My GitHub account</option><option value="organization">An organization</option></select></label>
                  <label className="space-y-2"><span className="block text-[11px] font-medium">App name</span><Input maxLength={100} onChange={(event) => setAppName(event.target.value)} value={appName} /></label>
                  {appOwnerType === "organization" ? <label className="space-y-2 sm:col-span-2"><span className="block text-[11px] font-medium">Organization login</span><Input onChange={(event) => setAppOrganization(event.target.value)} placeholder="your-organization" required value={appOrganization} /></label> : null}
                </div>
                <Button className="mt-3" disabled={manifestPending || (appOwnerType === "organization" && !appOrganization.trim())} onClick={() => void createGitHubApp()} type="button">
                  {manifestPending ? <LoaderCircle className="animate-spin" /> : <Github />}
                  {manifestPending ? "Opening GitHub…" : "Create GitHub App"}
                </Button>
                {data.configuration.webhookUrl.startsWith("http://localhost") ? (
                  <p className="mt-2 text-[11px] leading-4 text-amber-300/80">The manifest will leave webhook delivery disabled while GridOps uses localhost. Set a public HTTPS base URL before enabling GitHub deliveries.</p>
                ) : null}
              </div>
            ) : null}
          </CardContent>
        </Card>

        <Card>
          <CardHeader><div><CardTitle>Runner host</CardTitle><p className="mt-1 text-xs text-muted-foreground">The web service cannot access Docker directly.</p></div><Badge variant={data.manager.ok ? "success" : "destructive"}>{data.manager.ok ? "healthy" : "offline"}</Badge></CardHeader>
          <CardContent className="space-y-3 text-sm">
            <InfoRow label="Manager" value={data.manager.ok ? "Authenticated and reachable" : "Unavailable"} />
            <InfoRow label="Docker Engine" value={data.manager.dockerVersion ?? "—"} />
            <InfoRow label="Docker API" value={data.manager.apiVersion ?? "—"} />
            <InfoRow label="GitHub control token" value={data.configuration.installationTokens ? "Installation token" : "Authorized user token fallback"} />
            <InfoRow label="Database" value="SQLite · WAL mode" />
            <InfoRow label="Signed in as" value={data.user.login} />
            {!data.manager.ok && data.manager.error ? <p className="rounded-md border border-red-500/20 bg-red-500/5 p-3 text-xs leading-5 text-red-300">{data.manager.error}</p> : null}
            <a className="inline-flex h-9 items-center justify-center gap-2 rounded-md border border-border px-3 text-xs font-medium hover:bg-accent" href="/api/backups/database"><DatabaseBackup className="size-4" />Download SQLite backup</a>
          </CardContent>
        </Card>
      </div>

      <form className="mt-4" onSubmit={submit}>
        <Card>
          <CardHeader><div><CardTitle>Retention and reconciliation</CardTitle><p className="mt-1 text-xs text-muted-foreground">Durable system policy stored in SQLite and included in backups.</p></div></CardHeader>
          <CardContent>
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
              <NumberField defaultValue={data.settings.logRetentionDays} label="Runner log retention" name="logRetentionDays" suffix="days" />
              <NumberField defaultValue={data.settings.webhookRetentionDays} label="Webhook retention" name="webhookRetentionDays" suffix="days" />
              <NumberField defaultValue={data.settings.auditRetentionDays} label="Audit retention" name="auditRetentionDays" suffix="days" />
              <NumberField defaultValue={data.settings.reconcileIntervalSeconds} label="Reconcile interval" name="reconcileIntervalSeconds" suffix="seconds" />
            </div>
            <label className="mt-5 flex items-start gap-3 rounded-md border border-border p-3">
              <input className="mt-0.5 size-4 accent-emerald-500" defaultChecked={data.settings.autoUpdateImages} name="autoUpdateImages" type="checkbox" />
              <span><span className="block text-sm font-medium">Automatically refresh runner images</span><span className="mt-1 block text-xs text-muted-foreground">Pull configured tags before provisioning replacement containers.</span></span>
            </label>
            <div className="mt-5 flex justify-end"><Button disabled={pending} type="submit">{pending ? <LoaderCircle className="animate-spin" /> : <Save />}{pending ? "Saving…" : "Save policy"}</Button></div>
          </CardContent>
        </Card>
      </form>
    </ResourcePage>
  );
}

function CopyRow({ label, value }: { label: string; value: string }) {
  return <div><div className="text-[11px] text-muted-foreground">{label}</div><code className="mt-1 block break-all text-foreground">{value}</code></div>;
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return <div className="flex items-center justify-between gap-4 border-b border-border pb-3 last:border-0 last:pb-0"><span className="text-muted-foreground">{label}</span><span className="text-right font-medium">{value}</span></div>;
}

function NumberField({ label, name, defaultValue, suffix }: { label: string; name: string; defaultValue: number; suffix: string }) {
  return <label className="space-y-2"><span className="block text-xs font-medium">{label}</span><div className="relative"><Input defaultValue={defaultValue} min="1" name={name} required type="number" /><span className="pointer-events-none absolute inset-y-0 right-3 flex items-center text-[11px] text-muted-foreground">{suffix}</span></div></label>;
}
