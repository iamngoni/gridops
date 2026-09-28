import { useRouter } from "@tanstack/react-router";
import { useCallback } from "react";
import { toast } from "sonner";

/** Runs a mutating action with optional confirmation, a toast, and a route refresh. */
export function useAction() {
  const router = useRouter();
  return useCallback(async (options: { action: () => Promise<unknown>; success: string; confirm?: string; after?: () => void | Promise<void> }) => {
    if (options.confirm && !window.confirm(options.confirm)) return false;
    const pending = options.action();
    toast.promise(pending, {
      loading: "Working…",
      success: options.success,
      error: (error: unknown) => error instanceof Error ? error.message : "Action failed.",
    });
    try {
      await pending;
    } catch {
      return false;
    }
    await options.after?.();
    await router.invalidate();
    return true;
  }, [router]);
}
