import { createFileRoute } from "@tanstack/react-router";

import { clearSessionCookie, deleteSession } from "~/server/auth/session.server";

export const Route = createFileRoute("/auth/logout")({
  server: {
    handlers: {
      POST: async ({ request }) => {
        deleteSession(request);
        return new Response(null, {
          status: 302,
          headers: {
            Location: "/",
            "Set-Cookie": clearSessionCookie(request),
            "Cache-Control": "no-store",
          },
        });
      },
    },
  },
});
