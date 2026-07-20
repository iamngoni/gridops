import { createFileRoute } from "@tanstack/react-router";

import { beginGitHubOAuth } from "~/server/github/oauth.server";

export const Route = createFileRoute("/auth/github")({
  server: {
    handlers: {
      GET: async ({ request }) => beginGitHubOAuth(request),
    },
  },
});
