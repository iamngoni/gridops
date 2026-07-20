import { createFileRoute } from "@tanstack/react-router";

import { completeGitHubOAuth } from "~/server/github/oauth.server";

export const Route = createFileRoute("/auth/github/callback")({
  server: {
    handlers: {
      GET: async ({ request }) => completeGitHubOAuth(request),
    },
  },
});
