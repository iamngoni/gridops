import { Link, type LinkProps } from "@tanstack/react-router";
import type { LucideIcon } from "lucide-react";
import { ChevronRight, Info, LoaderCircle, TriangleAlert } from "lucide-react";
import type * as React from "react";

import { MobileNavButton } from "./app-shell-context";
import { cn } from "~/lib/utils";

type Crumb = { label: string; to?: LinkProps["to"]; params?: LinkProps["params"] };

/** The 44px bar at the top of every view: breadcrumbs on the left, view actions on the right. */
export function PageHeader({
  title,
  icon: Icon,
  breadcrumbs = [],
  count,
  actions,
  children,
}: {
  title: React.ReactNode;
  icon?: LucideIcon;
  breadcrumbs?: Crumb[];
  count?: number;
  actions?: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <header className="flex h-11 shrink-0 items-center gap-2 border-b border-border px-3 sm:px-4">
      <MobileNavButton />
      <nav aria-label="Breadcrumb" className="flex min-w-0 flex-1 items-center gap-1 text-sm">
        {breadcrumbs.map((crumb) => (
          <span className="flex shrink-0 items-center gap-1" key={crumb.label}>
            {crumb.to ? (
              <Link className="flex items-center gap-1.5 rounded-md px-1.5 py-0.5 text-muted-foreground hover:bg-hover hover:text-foreground" params={crumb.params} to={crumb.to}>
                {crumb.label}
              </Link>
            ) : (
              <span className="px-1.5 text-muted-foreground">{crumb.label}</span>
            )}
            <ChevronRight className="size-3.5 text-faint" />
          </span>
        ))}
        <h1 className="flex min-w-0 items-center gap-2 px-1.5 font-medium text-foreground">
          {Icon ? <Icon className="size-4 shrink-0 text-muted-foreground" /> : null}
          <span className="truncate">{title}</span>
          {count !== undefined ? <span className="tabular shrink-0 text-xs font-normal text-faint">{count}</span> : null}
        </h1>
        {children}
      </nav>
      {actions ? <div className="flex shrink-0 items-center gap-1.5">{actions}</div> : null}
    </header>
  );
}

/** Secondary bar under the header, for view tabs on the left and display controls on the right. */
export function PageToolbar({ children, actions, className }: { children?: React.ReactNode; actions?: React.ReactNode; className?: string }) {
  return (
    <div className={cn("flex min-h-11 shrink-0 flex-wrap items-center gap-2 border-b border-border px-3 py-1.5 sm:px-4", className)}>
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1">{children}</div>
      {actions ? <div className="flex shrink-0 items-center gap-1.5">{actions}</div> : null}
    </div>
  );
}

export function ViewTab({ active, children, count, ...props }: React.ComponentProps<"button"> & { active?: boolean; count?: number }) {
  return (
    <button
      aria-pressed={active}
      className={cn(
        "inline-flex h-7 items-center gap-1.5 rounded-md border px-2.5 text-xs font-medium transition-colors",
        active ? "border-border-strong bg-selected text-foreground" : "border-transparent text-muted-foreground hover:bg-hover hover:text-foreground",
      )}
      type="button"
      {...props}
    >
      {children}
      {count !== undefined ? <span className="tabular text-faint">{count}</span> : null}
    </button>
  );
}

export function ViewTabLink({ active, children, count, ...props }: LinkProps & { active?: boolean; count?: number; children: React.ReactNode }) {
  return (
    <Link
      className={cn(
        "inline-flex h-7 items-center gap-1.5 rounded-md border px-2.5 text-xs font-medium transition-colors",
        active ? "border-border-strong bg-selected text-foreground" : "border-transparent text-muted-foreground hover:bg-hover hover:text-foreground",
      )}
      {...props}
    >
      {children}
      {count !== undefined ? <span className="tabular text-faint">{count}</span> : null}
    </Link>
  );
}

export function PageBody({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("min-h-0 flex-1 overflow-auto", className)}>{children}</div>;
}

/** Group header for grouped lists ("In progress 3"), sticky while its rows scroll. */
export function ListGroup({
  label,
  icon,
  count,
  actions,
  children,
}: {
  label: React.ReactNode;
  icon?: React.ReactNode;
  count?: number;
  actions?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section>
      <div className="sticky top-0 z-10 flex h-9 items-center gap-2 border-b border-border bg-panel-subtle/95 px-4 text-xs font-medium backdrop-blur supports-[backdrop-filter]:bg-panel-subtle/80">
        {icon}
        <span className="text-foreground">{label}</span>
        {count !== undefined ? <span className="tabular text-faint">{count}</span> : null}
        {actions ? <div className="ml-auto flex items-center gap-1">{actions}</div> : null}
      </div>
      <div>{children}</div>
    </section>
  );
}

export const listRowClassName =
  "group/row flex min-h-11 items-center gap-3 border-b border-border px-4 py-2 text-sm transition-colors hover:bg-hover focus-visible:bg-hover focus-visible:outline-none";

export function ListRow({ className, ...props }: React.ComponentProps<"div">) {
  return <div className={cn(listRowClassName, className)} {...props} />;
}

export function EmptyState({
  icon: Icon,
  title,
  description,
  children,
  className,
}: {
  icon: LucideIcon;
  title: string;
  description?: React.ReactNode;
  children?: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("grid min-h-[60vh] place-items-center px-6 py-16 text-center", className)}>
      <div className="flex max-w-sm flex-col items-center">
        <div className="relative mb-5 grid size-14 place-items-center rounded-2xl border border-border-strong bg-panel-subtle text-faint shadow-[inset_0_1px_0_rgb(255_255_255/0.04)]">
          <Icon className="size-6" strokeWidth={1.5} />
        </div>
        <h2 className="text-base font-medium text-foreground">{title}</h2>
        {description ? <p className="mt-1.5 text-sm leading-5 text-muted-foreground">{description}</p> : null}
        {children ? <div className="mt-5 flex flex-wrap items-center justify-center gap-2">{children}</div> : null}
      </div>
    </div>
  );
}

export function LoadingRows({ rows = 8 }: { rows?: number }) {
  return (
    <div aria-busy="true" aria-live="polite">
      <span className="sr-only">Loading</span>
      {Array.from({ length: rows }, (_, index) => (
        <div className="flex h-11 items-center gap-3 border-b border-border px-4" key={index}>
          <div className="size-3.5 animate-pulse rounded-full bg-selected" />
          <div className="h-3 animate-pulse rounded bg-selected" style={{ width: `${28 + ((index * 17) % 34)}%` }} />
          <div className="ml-auto h-3 w-16 animate-pulse rounded bg-selected" />
        </div>
      ))}
    </div>
  );
}

export function InlineLoading({ label }: { label: string }) {
  return <div className="flex items-center gap-2 px-4 py-6 text-sm text-muted-foreground"><LoaderCircle className="size-4 animate-spin" />{label}</div>;
}

export function Callout({
  tone = "neutral",
  title,
  children,
  action,
  className,
}: {
  tone?: "neutral" | "warning" | "danger" | "info";
  title?: React.ReactNode;
  children?: React.ReactNode;
  action?: React.ReactNode;
  className?: string;
}) {
  const accent = tone === "danger" ? "text-danger" : tone === "warning" ? "text-warning" : tone === "info" ? "text-info" : "text-muted-foreground";
  return (
    <div className={cn("flex items-start gap-3 rounded-lg border border-border-strong bg-panel-subtle px-3 py-2.5 text-sm", className)} role={tone === "danger" ? "alert" : undefined}>
      {tone === "danger" || tone === "warning" ? <TriangleAlert className={cn("mt-0.5 size-4 shrink-0", accent)} /> : <Info className={cn("mt-0.5 size-4 shrink-0", accent)} />}
      <div className="min-w-0 flex-1">
        {title ? <div className="font-medium text-foreground">{title}</div> : null}
        {children ? <div className={cn("text-xs leading-5 text-muted-foreground", title ? "mt-0.5" : undefined)}>{children}</div> : null}
      </div>
      {action ? <div className="shrink-0 self-center">{action}</div> : null}
    </div>
  );
}

/** Right-hand properties column on detail views. */
export function PropertiesPanel({ children, className }: { children: React.ReactNode; className?: string }) {
  return <aside className={cn("w-full shrink-0 border-t border-border lg:w-[296px] lg:border-l lg:border-t-0", className)}><div className="space-y-6 p-4 lg:sticky lg:top-0">{children}</div></aside>;
}

export function PropertyGroup({ title, children, action }: { title: string; children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div>
      <div className="mb-2 flex items-center justify-between"><h3 className="text-xs font-medium text-muted-foreground">{title}</h3>{action}</div>
      <dl className="space-y-0.5">{children}</dl>
    </div>
  );
}

export function PropertyRow({ label, children, title }: { label: string; children: React.ReactNode; title?: string }) {
  return (
    <div className="grid min-h-8 grid-cols-[96px_minmax(0,1fr)] items-center gap-2 rounded-md text-sm">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="flex min-w-0 items-center gap-1.5 truncate text-foreground" title={title}>{children}</dd>
    </div>
  );
}

export function SectionHeading({ children, actions, className }: { children: React.ReactNode; actions?: React.ReactNode; className?: string }) {
  return (
    <div className={cn("flex items-center justify-between gap-3", className)}>
      <h2 className="text-sm font-medium text-foreground">{children}</h2>
      {actions ? <div className="flex items-center gap-1.5">{actions}</div> : null}
    </div>
  );
}
