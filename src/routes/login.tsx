import { createFileRoute } from "@tanstack/react-router";
import { Boxes, Github, Radio, ShieldCheck } from "lucide-react";

import { GridLogo } from "~/components/grid-logo";
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
    <main className="relative min-h-screen overflow-hidden bg-background px-5 py-8 text-foreground">
      <div className="capacity-grid pointer-events-none absolute inset-0 opacity-45" />
      <div className="pointer-events-none absolute left-1/2 top-0 h-96 w-[48rem] -translate-x-1/2 rounded-full bg-emerald-500/8 blur-3xl" />

      <div className="relative z-10 mx-auto flex min-h-[calc(100vh-4rem)] max-w-6xl flex-col">
        <GridLogo className="h-10" />

        <div className="grid flex-1 items-center gap-12 py-12 lg:grid-cols-[minmax(0,1.1fr)_minmax(360px,0.72fr)]">
          <section className="max-w-2xl">
            <div className="inline-flex items-center gap-2 rounded-full border border-emerald-400/20 bg-emerald-400/5 px-3 py-1.5 text-xs font-medium text-emerald-300">
              <span className="size-1.5 rounded-full bg-emerald-400" />
              Self-hosted runner operations
            </div>
            <h1 className="mt-6 text-4xl font-semibold tracking-[-0.035em] sm:text-5xl lg:text-6xl">
              Your GitHub Actions fleet, under control.
            </h1>
            <p className="mt-5 max-w-xl text-base leading-7 text-muted-foreground sm:text-lg">
              Provision isolated runners, watch live jobs, and operate every pool from one private control plane.
            </p>

            <div className="mt-9 hidden gap-3 sm:grid sm:grid-cols-3">
              <Feature icon={Boxes} label="Runner pools" detail="Provision and scale" />
              <Feature icon={Radio} label="Live activity" detail="Runs, jobs, and logs" />
              <Feature icon={ShieldCheck} label="Private by default" detail="GitHub-authenticated access" />
            </div>
          </section>

          <section className="rounded-xl border border-border bg-card/95 p-6 shadow-2xl shadow-black/25 backdrop-blur sm:p-8">
            <div className="grid size-11 place-items-center rounded-lg border border-border bg-background">
              <Github className="size-5" />
            </div>
            <h2 className="mt-6 text-xl font-semibold">Sign in to GridOps</h2>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">
              Continue with GitHub to access your installations, runners, and workflow operations.
            </p>

            {authError ? (
              <p className="mt-5 rounded-md border border-red-500/25 bg-red-500/10 px-3 py-2.5 text-xs leading-5 text-red-300" role="alert">
                {authError}
              </p>
            ) : null}

            <a className={cn(buttonVariants({ size: "lg" }), "mt-6 w-full")} href={oauthHref}>
              <Github />
              Continue with GitHub
            </a>

            <div className="mt-6 border-t border-border pt-5">
              <div className="flex items-start gap-3 text-xs leading-5 text-muted-foreground">
                <ShieldCheck className="mt-0.5 size-4 shrink-0 text-emerald-400" />
                <span>Authentication is handled by GitHub. GridOps never receives or stores your GitHub password.</span>
              </div>
            </div>
          </section>
        </div>

        <p className="text-center text-[11px] text-muted-foreground/70">
          GridOps · Self-hosted control plane for GitHub Actions runners
        </p>
      </div>
    </main>
  );
}

function Feature({ icon: Icon, label, detail }: { icon: typeof Boxes; label: string; detail: string }) {
  return (
    <div className="rounded-lg border border-border/80 bg-card/45 p-3.5 backdrop-blur-sm">
      <Icon className="size-4 text-primary" />
      <div className="mt-3 text-sm font-medium">{label}</div>
      <div className="mt-0.5 text-[11px] text-muted-foreground">{detail}</div>
    </div>
  );
}
