import { useRouter } from "@tanstack/react-router";
import { useEffect } from "react";

export function useLiveRouteRefresh(intervalMilliseconds: number, enabled = true) {
  const router = useRouter();

  useEffect(() => {
    if (!enabled) return undefined;
    let refreshing = false;
    let cancelled = false;

    async function refresh() {
      if (cancelled || refreshing || document.visibilityState === "hidden") return;
      refreshing = true;
      try {
        await router.invalidate().catch(() => undefined);
      } finally {
        refreshing = false;
      }
    }

    function handleVisibilityChange() {
      if (document.visibilityState === "visible") void refresh();
    }

    const interval = window.setInterval(() => void refresh(), intervalMilliseconds);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [enabled, intervalMilliseconds, router]);
}
