import { Outlet, createFileRoute } from "@tanstack/react-router";

import { AppShell } from "~/components/app-shell";

export const Route = createFileRoute("/_app")({
  component: AppLayout,
});

function AppLayout() {
  return (
    <AppShell>
      <Outlet />
    </AppShell>
  );
}
