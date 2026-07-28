import { createFileRoute } from "@tanstack/react-router";
import { CloudCog, LoaderCircle, Plus, ShieldCheck } from "lucide-react";
import { type FormEvent, useState } from "react";
import { toast } from "sonner";

import { AppShell } from "~/components/app-shell";
import { Button } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Input } from "~/components/ui/input";
import { createBitbucketConnection, getBitbucketConnections } from "~/features/platform-connections/platform-connections.functions";
import { formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/platform-connections")({
  loader: () => getBitbucketConnections(),
  component: PlatformConnectionsPage,
});

function PlatformConnectionsPage() {
  const initial = Route.useLoaderData();
  const [connections, setConnections] = useState(initial.items);
  const [name, setName] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [accessToken, setAccessToken] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const canManage = initial.canManage;

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const connection = await createBitbucketConnection({ name, workspace, accessToken });
      setConnections((current) => [...current, { ...connection, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), canManage: true }].sort((left, right) => left.name.localeCompare(right.name)));
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
    <AppShell>
      <div className="mx-auto max-w-4xl space-y-5">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight md:text-3xl">Platform connections</h1>
          <p className="mt-1 text-sm text-muted-foreground">Connect CI platforms once, then assign their repositories or workspaces to the same GridOps runner pools.</p>
        </div>

        <Card>
          <CardHeader><CardTitle className="flex items-center gap-2"><CloudCog className="size-5 text-primary" />Bitbucket Cloud</CardTitle></CardHeader>
          <CardContent className="space-y-4">
            <div className="rounded-md border border-border bg-muted/20 p-3 text-xs leading-5 text-muted-foreground">
              GridOps verifies the workspace before saving its token. The token is encrypted at rest and needs workspace read plus Pipelines runner read/write access. It is never returned to the browser after this form is submitted.
            </div>
            {canManage ? <form className="grid gap-4 md:grid-cols-2" onSubmit={submit}>
              <Field label="Connection name"><Input onChange={(event) => setName(event.target.value)} placeholder="Mobile delivery" required value={name} /></Field>
              <Field label="Workspace slug"><Input autoCapitalize="none" onChange={(event) => setWorkspace(event.target.value)} placeholder="acme-mobile" required value={workspace} /></Field>
              <Field className="md:col-span-2" label="Bitbucket API token"><Input autoComplete="off" onChange={(event) => setAccessToken(event.target.value)} placeholder="Paste a token with runner access" required type="password" value={accessToken} /></Field>
              {error ? <p className="md:col-span-2 rounded-md border border-red-500/25 bg-red-500/10 px-3 py-2 text-sm text-red-300" role="alert">{error}</p> : null}
              <div className="md:col-span-2 flex justify-end"><Button disabled={submitting} type="submit">{submitting ? <LoaderCircle className="animate-spin" /> : <Plus />}{submitting ? "Connecting…" : "Connect workspace"}</Button></div>
            </form> : <p className="text-sm text-muted-foreground">A GridOps system administrator can add or change platform connections.</p>}
          </CardContent>
        </Card>

        <Card>
          <CardHeader><CardTitle>Connected workspaces</CardTitle></CardHeader>
          <CardContent>
            {connections.length ? <div className="divide-y divide-border rounded-md border border-border">{connections.map((connection) => <div className="flex items-center gap-3 px-4 py-3" key={connection.id}><ShieldCheck className="size-4 text-emerald-400" /><div className="min-w-0 flex-1"><div className="font-medium">{connection.name}</div><div className="mt-0.5 text-xs text-muted-foreground">bitbucket.org/{connection.workspace} · verified {formatRelativeTime(connection.updatedAt)}</div></div></div>)}</div> : <div className="py-8 text-center text-sm text-muted-foreground">No Bitbucket workspaces are connected yet.</div>}
          </CardContent>
        </Card>
      </div>
    </AppShell>
  );
}

function Field({ label, className, children }: { label: string; className?: string; children: React.ReactNode }) {
  return <label className={className}><span className="mb-2 block text-xs font-medium">{label}</span>{children}</label>;
}
