import { createFileRoute } from "@tanstack/react-router";

import { getSessionUser } from "~/server/auth/session.server";
import { getSqlite, migrateDatabase } from "~/server/db/client.server";
import { getValidGitHubAccessToken } from "~/server/github/token.server";

export const Route = createFileRoute("/api/workflow-runs/$runId/logs")({
  server: { handlers: { GET: downloadWorkflowLogs } },
});

async function downloadWorkflowLogs({ request, params }: { request: Request; params: { runId: string } }) {
  const user = getSessionUser(request);
  if (!user) return Response.json({ error: "Authentication required." }, { status: 401 });
  migrateDatabase();
  const runId = Number(params.runId);
  const run = getSqlite().prepare(`
    SELECT wr.id, repo.owner, repo.name
    FROM workflow_runs wr
    JOIN repositories repo ON repo.id = wr.repository_id
    JOIN user_installations ui ON ui.installation_id = repo.installation_id
    WHERE wr.id = ? AND ui.user_id = ?
  `).get(runId, user.id) as { id: number; owner: string; name: string } | undefined;
  if (!run) return Response.json({ error: "Workflow run not found." }, { status: 404 });

  const token = await getValidGitHubAccessToken(user.id);
  const response = await fetch(`https://api.github.com/repos/${run.owner}/${run.name}/actions/runs/${run.id}/logs`, {
    redirect: "manual",
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": "2026-03-10",
      "User-Agent": "GridOps",
    },
  });
  const location = response.headers.get("location");
  if (location && response.status >= 300 && response.status < 400) return Response.redirect(location, 302);
  if (!response.ok) return Response.json({ error: "GitHub workflow logs are not available." }, { status: response.status });
  return new Response(response.body, {
    headers: {
      "Content-Type": response.headers.get("content-type") ?? "application/zip",
      "Content-Disposition": `attachment; filename="gridops-run-${run.id}-logs.zip"`,
      "Cache-Control": "private, no-store",
    },
  });
}
