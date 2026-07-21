import type { LucideIcon } from "lucide-react";
import { ArrowRight, Plus } from "lucide-react";
import { Link } from "@tanstack/react-router";

import { AppShell } from "./app-shell";
import { Button } from "./ui/button";
import { Card, CardContent } from "./ui/card";

export function ResourcePage({
  title,
  description,
  icon: Icon,
  emptyTitle,
  emptyDescription,
  action,
  actionHref,
  actionIcon: ActionIcon = Plus,
  children,
}: {
  title: string;
  description: string;
  icon: LucideIcon;
  emptyTitle: string;
  emptyDescription: string;
  action?: string;
  actionHref?: string;
  actionIcon?: LucideIcon;
  children?: React.ReactNode;
}) {
  return (
    <AppShell>
      <div className="flex flex-col gap-8">
        <div className="flex flex-col justify-between gap-5 sm:flex-row sm:items-end">
          <div className="flex min-w-0 items-start gap-3.5">
            <div className="mt-0.5 grid size-10 shrink-0 place-items-center rounded-xl border border-primary/15 bg-primary/[0.08] text-primary shadow-[0_1px_0_hsl(150_70%_90%/0.06)_inset]">
              <Icon className="size-5" />
            </div>
            <div className="min-w-0">
              <p className="text-[10px] font-semibold uppercase tracking-[0.12em] text-primary/80">GridOps control plane</p>
              <h1 className="mt-1 text-2xl font-semibold tracking-[-0.025em] md:text-3xl">{title}</h1>
              <p className="mt-1.5 max-w-[62ch] text-sm leading-6 text-muted-foreground">{description}</p>
            </div>
          </div>
          {action && actionHref && (
            <Link className="inline-flex h-9 shrink-0 items-center justify-center gap-2 rounded-md bg-primary px-3 text-sm font-medium text-primary-foreground hover:bg-primary/90" to={actionHref}>
              <ActionIcon />{action}
            </Link>
          )}
          {action && !actionHref && <Button><ActionIcon />{action}</Button>}
        </div>

        {children ?? (
          <Card className="overflow-hidden bg-[radial-gradient(circle_at_50%_10%,hsl(153_55%_20%/0.12),transparent_22rem)]">
            <CardContent className="grid min-h-[440px] place-items-center p-8 text-center">
              <div className="max-w-sm">
                <div className="mx-auto grid size-12 place-items-center rounded-xl border border-primary/15 bg-primary/[0.08] text-primary">
                  <Icon className="size-5.5" />
                </div>
                <h2 className="mt-5 text-lg font-semibold tracking-tight">{emptyTitle}</h2>
                <p className="mt-2 text-sm leading-6 text-muted-foreground">{emptyDescription}</p>
                {action && actionHref && (
                  <Link className="mt-5 inline-flex h-9 items-center justify-center gap-2 rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-accent" to={actionHref}>
                    {action}<ArrowRight />
                  </Link>
                )}
                {action && !actionHref && <Button className="mt-5" variant="outline">{action}<ArrowRight /></Button>}
              </div>
            </CardContent>
          </Card>
        )}
      </div>
    </AppShell>
  );
}
