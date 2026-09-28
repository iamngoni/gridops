import { createFileRoute, redirect } from "@tanstack/react-router";

// Platform connections moved into Settings → Integrations; keep old links working.
export const Route = createFileRoute("/_app/platform-connections")({
  beforeLoad: () => {
    throw redirect({ replace: true, to: "/settings/integrations" });
  },
});
