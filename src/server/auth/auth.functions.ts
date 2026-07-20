import { createServerFn } from "@tanstack/react-start";
import { getRequest, setResponseHeaders } from "@tanstack/react-start/server";

import { getSessionUser } from "./session.server";
import { getSqlite, migrateDatabase } from "../db/client.server";

export const getViewer = createServerFn({ method: "GET" }).handler(async () => {
  setResponseHeaders(
    new Headers({
      "Cache-Control": "private, no-store",
      Vary: "Cookie, Authorization",
    }),
  );
  const user = getSessionUser(getRequest());
  if (!user) return null;

  migrateDatabase();
  const alerts = getSqlite().prepare(`
    SELECT
      (SELECT COUNT(*) FROM runners r
        JOIN runner_pools p ON p.id = r.pool_id
        JOIN user_installations ui ON ui.installation_id = p.installation_id
        WHERE ui.user_id = ? AND r.deleted_at IS NULL AND r.status = 'failed') AS failedRunners,
      (SELECT COUNT(*) FROM webhook_deliveries wd
        WHERE wd.status IN ('failed','rejected') AND (
          wd.installation_id IS NULL OR EXISTS (
            SELECT 1 FROM user_installations ui
            WHERE ui.installation_id = wd.installation_id AND ui.user_id = ?
          )
        )) AS failedWebhooks,
      (SELECT COUNT(*) FROM workflow_jobs wj
        JOIN workflow_runs wr ON wr.id = wj.run_id
        JOIN repositories repo ON repo.id = wr.repository_id
        JOIN user_installations ui ON ui.installation_id = repo.installation_id
        WHERE ui.user_id = ? AND wj.status = 'queued') AS queuedJobs
  `).get(user.id, user.id, user.id) as {
    failedRunners: number;
    failedWebhooks: number;
    queuedJobs: number;
  };

  return {
    id: user.id,
    githubId: user.githubId,
    login: user.login,
    name: user.name,
    email: user.email,
    avatarUrl: user.avatarUrl,
    alerts,
  };
});
