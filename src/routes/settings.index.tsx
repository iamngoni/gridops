import { createFileRoute, redirect } from "@tanstack/react-router";
import { toast } from "sonner";

export const Route = createFileRoute("/settings/")({
  beforeLoad: ({ location, preload }) => {
    const search = location.search as Record<string, unknown>;
    // GitHub App setup failures land on /settings?appError=…; surface them on the GitHub page.
    if (typeof search.appError === "string") {
      if (!preload) toast.error(search.appError);
      throw redirect({ replace: true, to: "/settings/github" });
    }
    throw redirect({ replace: true, to: "/settings/general" });
  },
});
