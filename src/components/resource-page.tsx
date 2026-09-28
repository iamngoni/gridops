import { Link } from "@tanstack/react-router";
import type { LucideIcon } from "lucide-react";
import { Plus } from "lucide-react";

import { EmptyState, PageBody, PageHeader } from "./page";
import { Button, buttonVariants } from "./ui/button";

/**
 * A simple single-list view: header bar plus a body that falls back to an
 * empty state when the view has nothing to show.
 */
export function ResourcePage({
  title,
  description,
  icon,
  emptyTitle,
  emptyDescription,
  action,
  actionHref,
  actionIcon: ActionIcon = Plus,
  count,
  headerActions,
  toolbar,
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
  count?: number;
  headerActions?: React.ReactNode;
  toolbar?: React.ReactNode;
  children?: React.ReactNode;
}) {
  const primary = action && actionHref ? (
    <Link className={buttonVariants({ size: "sm" })} to={actionHref}><ActionIcon />{action}</Link>
  ) : action ? <Button size="sm"><ActionIcon />{action}</Button> : null;

  return (
    <>
      <PageHeader actions={<>{headerActions}{primary}</>} count={count} icon={icon} title={title} />
      {toolbar}
      <PageBody>
        {children ?? (
          <EmptyState description={emptyDescription || description} icon={icon} title={emptyTitle}>
            {action && actionHref ? <Link className={buttonVariants({ variant: "outline" })} to={actionHref}><ActionIcon />{action}</Link> : null}
          </EmptyState>
        )}
      </PageBody>
    </>
  );
}
