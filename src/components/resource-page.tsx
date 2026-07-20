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
  children,
}: {
  title: string;
  description: string;
  icon: LucideIcon;
  emptyTitle: string;
  emptyDescription: string;
  action?: string;
  actionHref?: string;
  children?: React.ReactNode;
}) {
  return (
    <AppShell>
      <div className="flex flex-col gap-6">
        <div className="flex flex-col justify-between gap-4 sm:flex-row sm:items-center">
          <div>
            <h1 className="text-2xl font-semibold tracking-tight md:text-3xl">{title}</h1>
            <p className="mt-1 text-sm text-muted-foreground">{description}</p>
          </div>
          {action && actionHref && (
            <Link className="inline-flex h-9 shrink-0 items-center justify-center gap-2 rounded-md bg-primary px-3 text-sm font-medium text-primary-foreground hover:bg-primary/90" to={actionHref}>
              <Plus />{action}
            </Link>
          )}
          {action && !actionHref && <Button><Plus />{action}</Button>}
        </div>

        {children ?? (
          <Card>
            <CardContent className="grid min-h-[460px] place-items-center p-6 text-center">
              <div className="max-w-sm">
                <div className="mx-auto grid size-11 place-items-center rounded-lg border border-border bg-muted text-muted-foreground">
                  <Icon className="size-5" />
                </div>
                <h2 className="mt-4 text-base font-semibold">{emptyTitle}</h2>
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
