/// <reference types="vite/client" />

import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { Outlet, createRootRouteWithContext } from "@tanstack/react-router";
import { Toaster } from "sonner";

import { getViewer } from "~/lib/api";

type RouterContext = { queryClient: QueryClient };

export const Route = createRootRouteWithContext<RouterContext>()({
  loader: () => getViewer(),
  component: RootComponent,
  notFoundComponent: NotFound,
});

function RootComponent() {
  const { queryClient } = Route.useRouteContext();
  return (
    <QueryClientProvider client={queryClient}>
      <Outlet />
      <Toaster theme="dark" richColors position="bottom-right" />
    </QueryClientProvider>
  );
}

function NotFound() {
  return (
    <main className="grid min-h-screen place-items-center bg-background px-6 text-center">
      <div>
        <p className="text-sm font-medium text-primary">404</p>
        <h1 className="mt-2 text-2xl font-semibold">That GridOps view does not exist.</h1>
        <a className="mt-5 inline-block text-sm text-primary hover:underline" href="/">Return to overview</a>
      </div>
    </main>
  );
}
