import { createFileRoute, useRouter } from "@tanstack/react-router";
import { LoaderCircle, Plus } from "lucide-react";
import { type FormEvent, useState } from "react";
import { toast } from "sonner";

import { Callout } from "~/components/page";
import { SettingsLayout, SettingsRow, SettingsSection } from "~/components/settings-ui";
import { StatusDot } from "~/components/status-icon";
import { Button } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import {
  type BitbucketConnection,
  createBitbucketConnection,
  getBitbucketConnections,
  removeBitbucketConnection,
  updateBitbucketConnection,
} from "~/features/platform-connections/platform-connections.functions";
import { useAction } from "~/lib/use-action";
import { formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/settings/integrations")({
  loader: () => getBitbucketConnections(),
  component: BitbucketSettings,
});

function BitbucketSettings() {
  const { canManage, items: connections } = Route.useLoaderData();
  const router = useRouter();
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
      await createBitbucketConnection({ name, workspace, accessToken });
      setName("");
      setWorkspace("");
      setAccessToken("");
      toast.success("Bitbucket workspace connected.");
      await router.invalidate();
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
          <ConnectionRow canManage={canManage} connection={connection} key={connection.id} />
        )) : <div className="px-4 py-6 text-center text-sm text-muted-foreground">No Bitbucket workspaces are connected yet.</div>}
      </SettingsSection>

      {canManage ? (
        <form onSubmit={submit}>
          <SettingsSection description="GridOps verifies the workspace before saving. The token is encrypted at rest and never returned to the browser. Each workspace is connected once; edit an existing connection to replace its token." title="Connect a workspace">
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

/** One connected workspace, with inline editing and removal for administrators. */
function ConnectionRow({ connection, canManage }: { connection: BitbucketConnection; canManage: boolean }) {
  const router = useRouter();
  const run = useAction();
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(connection.name);
  const [accessToken, setAccessToken] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameId = `bb-edit-name-${connection.id}`;
  const tokenId = `bb-edit-token-${connection.id}`;

  function startEditing() {
    setName(connection.name);
    setAccessToken("");
    setError(null);
    setEditing(true);
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSaving(true);
    setError(null);
    try {
      const token = accessToken.trim();
      await updateBitbucketConnection(connection.id, { name, ...(token ? { accessToken: token } : {}) });
      toast.success(token ? "Connection updated and token replaced." : "Connection updated.");
      setEditing(false);
      setAccessToken("");
      await router.invalidate();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The connection could not be updated.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <div>
      <div className="flex items-center gap-3 px-4 py-3">
        <StatusDot tone="success" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium">{connection.name}</div>
          <div className="truncate text-xs text-muted-foreground">bitbucket.org/{connection.workspace}</div>
        </div>
        <span className="hidden text-xs text-faint sm:inline">Verified {formatRelativeTime(connection.updatedAt)}</span>
        {canManage && !editing ? (
          <div className="flex shrink-0 items-center gap-1">
            <Button onClick={startEditing} size="sm" variant="ghost">Edit</Button>
            <Button
              onClick={() => void run({
                action: () => removeBitbucketConnection(connection.id),
                confirm: `Remove ${connection.name}? GridOps deletes its stored token. Pools that use it must be changed first.`,
                success: `${connection.name} removed.`,
              })}
              size="sm"
              variant="ghost"
            >
              Remove
            </Button>
          </div>
        ) : null}
      </div>
      {editing ? (
        <form className="space-y-3 border-t border-border px-4 py-3" onSubmit={save}>
          <div className="grid gap-3 sm:grid-cols-2">
            <div>
              <label className="mb-1.5 block text-xs font-medium text-foreground" htmlFor={nameId}>Connection name</label>
              <Input id={nameId} onChange={(event) => setName(event.target.value)} required value={name} />
            </div>
            <div>
              <label className="mb-1.5 block text-xs font-medium text-foreground" htmlFor={tokenId}>New API token</label>
              <Input autoComplete="off" id={tokenId} onChange={(event) => setAccessToken(event.target.value)} placeholder="Leave blank to keep the current token" type="password" value={accessToken} />
            </div>
          </div>
          {error ? <Callout title="Could not save" tone="danger">{error}</Callout> : null}
          <div className="flex justify-end gap-2">
            <Button disabled={saving} onClick={() => setEditing(false)} type="button" variant="ghost">Cancel</Button>
            <Button disabled={saving} type="submit">{saving ? <LoaderCircle className="animate-spin" /> : null}{saving ? "Saving…" : "Save"}</Button>
          </div>
        </form>
      ) : null}
    </div>
  );
}
