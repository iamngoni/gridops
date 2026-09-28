import { createFileRoute } from "@tanstack/react-router";
import { LoaderCircle, Plus } from "lucide-react";
import { type FormEvent, useState } from "react";
import { toast } from "sonner";

import { Callout } from "~/components/page";
import { SettingsLayout, SettingsRow, SettingsSection } from "~/components/settings-ui";
import { StatusDot } from "~/components/status-icon";
import { Button } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import { createBitbucketConnection, getBitbucketConnections } from "~/features/platform-connections/platform-connections.functions";
import { formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/settings/integrations")({
  loader: () => getBitbucketConnections(),
  component: BitbucketSettings,
});

function BitbucketSettings() {
  const initial = Route.useLoaderData();
  const [connections, setConnections] = useState(initial.items);
  const [name, setName] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [accessToken, setAccessToken] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const connection = await createBitbucketConnection({ name, workspace, accessToken });
      const now = new Date().toISOString();
      setConnections((current) => [...current, { ...connection, createdAt: now, updatedAt: now, canManage: true }].sort((left, right) => left.name.localeCompare(right.name)));
      setName("");
      setWorkspace("");
      setAccessToken("");
      toast.success("Bitbucket workspace connected.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Bitbucket workspace could not be connected.");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <SettingsLayout description="Connect Bitbucket Cloud workspaces once, then assign them to runner pools alongside GitHub repositories." title="Bitbucket">
      <SettingsSection title={`Connected workspaces${connections.length ? ` · ${connections.length}` : ""}`}>
        {connections.length ? connections.map((connection) => (
          <div className="flex items-center gap-3 px-4 py-3" key={connection.id}>
            <StatusDot tone="success" />
            <div className="min-w-0 flex-1">
              <div className="truncate text-sm font-medium">{connection.name}</div>
              <div className="truncate text-xs text-muted-foreground">bitbucket.org/{connection.workspace}</div>
            </div>
            <span className="text-xs text-faint">Verified {formatRelativeTime(connection.updatedAt)}</span>
          </div>
        )) : <div className="px-4 py-6 text-center text-sm text-muted-foreground">No Bitbucket workspaces are connected yet.</div>}
      </SettingsSection>

      {initial.canManage ? (
        <form onSubmit={submit}>
          <SettingsSection description="GridOps verifies the workspace before saving. The token is encrypted at rest and never returned to the browser." title="Connect a workspace">
            <SettingsRow htmlFor="bb-name" label="Connection name">
              <Input id="bb-name" onChange={(event) => setName(event.target.value)} placeholder="Mobile delivery" required value={name} />
            </SettingsRow>
            <SettingsRow htmlFor="bb-workspace" label="Workspace slug">
              <Input autoCapitalize="none" id="bb-workspace" onChange={(event) => setWorkspace(event.target.value)} placeholder="acme-mobile" required value={workspace} />
            </SettingsRow>
            <SettingsRow description="Needs workspace read plus Pipelines runner read and write." htmlFor="bb-token" label="API token">
              <Input autoComplete="off" id="bb-token" onChange={(event) => setAccessToken(event.target.value)} placeholder="Paste a token" required type="password" value={accessToken} />
            </SettingsRow>
            {error ? <div className="px-4 py-3"><Callout title="Could not connect" tone="danger">{error}</Callout></div> : null}
            <div className="flex justify-end px-4 py-3">
              <Button disabled={submitting} type="submit">{submitting ? <LoaderCircle className="animate-spin" /> : <Plus />}{submitting ? "Connecting…" : "Connect workspace"}</Button>
            </div>
          </SettingsSection>
        </form>
      ) : <Callout title="Read-only access">A GridOps administrator can add or change platform connections.</Callout>}
    </SettingsLayout>
  );
}
