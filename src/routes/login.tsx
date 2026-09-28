import { createFileRoute } from "@tanstack/react-router";
import { Github } from "lucide-react";

import { GridMark } from "~/components/grid-logo";
import { Callout } from "~/components/page";
import { ThemeToggle } from "~/components/theme-toggle";
import { buttonVariants } from "~/components/ui/button";
import { safeReturnTo } from "~/lib/auth-navigation";
import { cn } from "~/lib/utils";

type LoginSearch = {
  returnTo: string;
  authError?: string;
};

export const Route = createFileRoute("/login")({
  validateSearch: (search: Record<string, unknown>): LoginSearch => ({
    returnTo: safeReturnTo(search.returnTo),
    authError: typeof search.authError === "string" ? search.authError : undefined,
  }),
  component: LoginPage,
});

function LoginPage() {
  const { returnTo, authError } = Route.useSearch();
  const oauthHref = `/auth/github?returnTo=${encodeURIComponent(returnTo)}`;

  return (
    <main className="relative flex min-h-dvh flex-col bg-background px-6 text-foreground">
      <div className="pointer-events-none absolute inset-x-0 top-0 h-[420px] bg-[radial-gradient(ellipse_at_top,color-mix(in_oklab,var(--primary)_18%,transparent),transparent_65%)]" />
      <div className="relative flex justify-end pt-4">
        <ThemeToggle />
      </div>
      <div className="relative flex flex-1 items-center justify-center pb-24">
        <div className="flex w-full max-w-[340px] flex-col items-center text-center">
          <GridMark className="shadow-[0_8px_24px_color-mix(in_oklab,var(--primary)_35%,transparent)]" size={44} />
          <h1 className="mt-8 text-2xl font-semibold tracking-[-0.015em]">Log in to GridOps</h1>
          <p className="mt-2 text-sm leading-5 text-muted-foreground">Operate your self-hosted GitHub Actions runners from one private control plane.</p>

          {authError ? <Callout className="mt-6 w-full text-left" title="Sign-in failed" tone="danger">{authError}</Callout> : null}

          <a className={cn(buttonVariants({ size: "lg" }), "mt-8 h-11 w-full text-sm")} href={oauthHref}>
            <Github />
            Continue with GitHub
          </a>
          <p className="mt-6 text-xs leading-5 text-faint">GitHub handles authentication. GridOps never sees your password.</p>
        </div>
      </div>
    </main>
  );
}
