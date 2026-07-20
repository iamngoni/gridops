import { createFileRoute } from "@tanstack/react-router";

import { receiveGitHubWebhook } from "~/server/github/webhooks.server";

export const Route = createFileRoute("/api/webhooks/github")({
  server: {
    handlers: {
      POST: async ({ request }) => receiveGitHubWebhook(request),
    },
  },
});
