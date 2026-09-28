import { createFileRoute, getRouteApi, useRouter } from "@tanstack/react-router";
import { LoaderCircle, Settings2 } from "lucide-react";
import { type FormEvent, useState } from "react";
import { toast } from "sonner";

import { EmptyState } from "~/components/page";
import { SettingsLayout, SettingsRow, SettingsSection, SettingsValue, Switch, UnitField } from "~/components/settings-ui";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import { saveSettingsAction } from "~/features/operations/operations.functions";

export const Route = createFileRoute("/settings/general")({
  component: GeneralSettings,
});

const settingsRoute = getRouteApi("/settings");

function GeneralSettings() {
  const page = settingsRoute.useLoaderData();
  const router = useRouter();
  const [pending, setPending] = useState(false);
  if (!page.data) return <EmptyState description="Sign in to view operational policy." icon={Settings2} title="Settings unavailable" />;
  const { settings, user } = page.data;
  const isAdmin = user.role === "admin";

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    const form = new FormData(event.currentTarget);
    try {
      await saveSettingsAction({ data: {
        logRetentionDays: Number(form.get("logRetentionDays")),
        logStorageBudgetMb: Number(form.get("logStorageBudgetMb")),
        webhookRetentionDays: Number(form.get("webhookRetentionDays")),
        auditRetentionDays: Number(form.get("auditRetentionDays")),
        reconcileIntervalSeconds: Number(form.get("reconcileIntervalSeconds")),
        githubSyncIntervalSeconds: Number(form.get("githubSyncIntervalSeconds")),
        autoUpdateImages: form.get("autoUpdateImages") === "on",
        provisioningPaused: form.get("provisioningPaused") === "on",
      } });
      toast.success("Policy saved.");
      await router.invalidate();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Could not save settings.");
    } finally {
      setPending(false);
    }
  }

  if (!isAdmin) {
    return (
      <SettingsLayout actions={<Badge variant="outline">Read only</Badge>} description="System policy is visible to members and editable by administrators." title="General">
        <SettingsSection title="Provisioning">
          <SettingsValue label="New runner provisioning" value={settings.provisioningPaused ? "Paused" : "Enabled"} />
          <SettingsValue label="Refresh runner images" value={settings.autoUpdateImages ? "On" : "Off"} />
          <SettingsValue label="Reconcile interval" value={`${settings.reconcileIntervalSeconds} seconds`} />
          <SettingsValue label="GitHub polling interval" value={`${settings.githubSyncIntervalSeconds} seconds`} />
        </SettingsSection>
        <SettingsSection title="Retention">
          <SettingsValue label="Runner logs" value={`${settings.logRetentionDays} days · ${settings.logStorageBudgetMb.toLocaleString()} MB`} />
          <SettingsValue label="Webhook deliveries" value={`${settings.webhookRetentionDays} days`} />
          <SettingsValue label="Audit events" value={`${settings.auditRetentionDays} days`} />
        </SettingsSection>
      </SettingsLayout>
    );
  }

  return (
    <form className="contents" onSubmit={submit}>
      <SettingsLayout description="Policy stored in SQLite and included in backups. The runner manager enforces host limits on top of it." title="General">
        <SettingsSection title="Provisioning">
          <SettingsRow description="Running jobs continue and idle capacity is still removed, but no automatic or manual action can start another runner." label="Pause all new provisioning">
            <Switch aria-label="Pause all new provisioning" defaultChecked={settings.provisioningPaused} name="provisioningPaused" />
          </SettingsRow>
          <SettingsRow description="Pull configured image tags before provisioning replacement runners." label="Refresh runner images">
            <Switch aria-label="Refresh runner images" defaultChecked={settings.autoUpdateImages} name="autoUpdateImages" />
          </SettingsRow>
          <SettingsRow description="How often the reconciler compares desired and actual runners." htmlFor="reconcile" label="Reconcile interval">
            <UnitField unit="seconds"><Input className="pr-16" defaultValue={settings.reconcileIntervalSeconds} id="reconcile" max={3600} min={5} name="reconcileIntervalSeconds" required type="number" /></UnitField>
          </SettingsRow>
          <SettingsRow description="Workflow state polling, used alongside or instead of webhooks." htmlFor="github-sync" label="GitHub polling interval">
            <UnitField unit="seconds"><Input className="pr-16" defaultValue={settings.githubSyncIntervalSeconds} id="github-sync" max={3600} min={30} name="githubSyncIntervalSeconds" required type="number" /></UnitField>
          </SettingsRow>
        </SettingsSection>

        <SettingsSection description="Older records are pruned automatically." title="Retention">
          <SettingsRow htmlFor="log-retention" label="Runner logs">
            <UnitField unit="days"><Input className="pr-12" defaultValue={settings.logRetentionDays} id="log-retention" max={3650} min={1} name="logRetentionDays" required type="number" /></UnitField>
          </SettingsRow>
          <SettingsRow description="Oldest logs are removed first once retained logs exceed this size." htmlFor="log-budget" label="Log storage budget">
            <UnitField unit="MB"><Input className="pr-10" defaultValue={settings.logStorageBudgetMb} id="log-budget" max={1048576} min={100} name="logStorageBudgetMb" required type="number" /></UnitField>
          </SettingsRow>
          <SettingsRow htmlFor="webhook-retention" label="Webhook deliveries">
            <UnitField unit="days"><Input className="pr-12" defaultValue={settings.webhookRetentionDays} id="webhook-retention" max={3650} min={1} name="webhookRetentionDays" required type="number" /></UnitField>
          </SettingsRow>
          <SettingsRow htmlFor="audit-retention" label="Audit events">
            <UnitField unit="days"><Input className="pr-12" defaultValue={settings.auditRetentionDays} id="audit-retention" max={3650} min={1} name="auditRetentionDays" required type="number" /></UnitField>
          </SettingsRow>
        </SettingsSection>

        <div className="flex justify-end">
          <Button disabled={pending} type="submit">{pending ? <LoaderCircle className="animate-spin" /> : null}{pending ? "Saving…" : "Save changes"}</Button>
        </div>
      </SettingsLayout>
    </form>
  );
}
