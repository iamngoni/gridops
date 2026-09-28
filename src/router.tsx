import { QueryClient } from "@tanstack/react-query";
import { createRouter } from "@tanstack/react-router";

import { ApiError } from "./lib/api";
import { routeTree } from "./routeTree.gen";

export function getRouter() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 10_000,
        refetchOnWindowFocus: false,
        // One quick retry covers a blip; repeating a 4xx or retrying three times
        // only keeps a skeleton on screen long after the answer is known.
        retry: (failureCount, error) => failureCount < 1 && !(error instanceof ApiError && error.status < 500),
      },
    },
  });

  return createRouter({
    routeTree,
    context: { queryClient },
    defaultPreload: "intent",
    // Quick loads swap views without flashing a skeleton; once a skeleton does
    // appear it stays long enough to read as a state rather than a flicker.
    defaultPendingMs: 150,
    defaultPendingMinMs: 300,
    scrollRestoration: true,
  });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof getRouter>;
  }
}
