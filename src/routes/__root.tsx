/// <reference types="vite/client" />

import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { Outlet, createRootRouteWithContext, redirect } from "@tanstack/react-router";
import { Toaster } from "sonner";

import { ThemeProvider, useTheme } from "~/components/theme-provider";
import { getViewer } from "~/lib/api";
import { safeReturnTo } from "~/lib/auth-navigation";

type RouterContext = { queryClient: QueryClient };

export const Route = createRootRouteWithContext<RouterContext>()({
  beforeLoad: async ({ location }) => {
    const viewer = await getViewer();
    const loginRoute = location.pathname === "/login";

    if (!viewer && !loginRoute) {
      throw redirect({
        replace: true,
        search: { returnTo: safeReturnTo(location.href) },
        to: "/login",
      });
    }

    if (viewer && loginRoute) {
      const search = location.search as Record<string, unknown>;
      throw redirect({ href: safeReturnTo(search.returnTo), replace: true });
    }

    return { viewer };
  },
  loader: ({ context }) => context.viewer,
  component: RootComponent,
  notFoundComponent: NotFound,
});

function RootComponent() {
  const { queryClient } = Route.useRouteContext();
  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <Outlet />
        <ThemeToaster />
      </QueryClientProvider>
    </ThemeProvider>
  );
}

function ThemeToaster() {
  const { theme } = useTheme();
  return <Toaster theme={theme} richColors position="bottom-right" />;
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
