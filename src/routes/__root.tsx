/// <reference types="vite/client" />

import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { Link, Outlet, createRootRouteWithContext, redirect } from "@tanstack/react-router";
import type * as React from "react";
import { Toaster } from "sonner";

import { GridMark } from "~/components/grid-logo";
import { ThemeProvider, useTheme } from "~/components/theme-provider";
import { buttonVariants } from "~/components/ui/button";
import { TooltipProvider } from "~/components/ui/tooltip";
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
        <TooltipProvider delayDuration={450} skipDelayDuration={200}>
          <Outlet />
          <ThemeToaster />
        </TooltipProvider>
      </QueryClientProvider>
    </ThemeProvider>
  );
}

function ThemeToaster() {
  const { theme } = useTheme();
  return (
    <Toaster
      gap={8}
      position="bottom-right"
      style={{
        "--normal-bg": "var(--popover)",
        "--normal-border": "var(--border-strong)",
        "--normal-text": "var(--foreground)",
        "--border-radius": "0.625rem",
        "--width": "340px",
      } as React.CSSProperties}
      theme={theme}
      toastOptions={{ className: "!text-sm !shadow-popover", descriptionClassName: "!text-muted-foreground" }}
    />
  );
}

function NotFound() {
  return (
    <main className="grid min-h-dvh place-items-center bg-background px-6 text-center">
      <div className="flex flex-col items-center">
        <GridMark size={32} />
        <p className="mt-6 text-sm font-medium text-muted-foreground">404</p>
        <h1 className="mt-1 text-xl font-semibold tracking-tight">This view doesn’t exist</h1>
        <p className="mt-2 max-w-xs text-sm text-muted-foreground">The link may be outdated, or the resource was removed.</p>
        <Link className={buttonVariants({ className: "mt-6", variant: "outline" })} to="/">Back to overview</Link>
      </div>
    </main>
  );
}
