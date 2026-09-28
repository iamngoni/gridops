import * as SwitchPrimitive from "@radix-ui/react-switch";
import type * as React from "react";

import { cn } from "~/lib/utils";

/** Centered settings column with a large title, as in Linear's settings screens. */
export function SettingsLayout({ title, description, actions, children, className }: { title: string; description?: React.ReactNode; actions?: React.ReactNode; children: React.ReactNode; className?: string }) {
  return (
    <div className={cn("mx-auto w-full max-w-[720px] px-5 pb-20 pt-10 sm:px-8 sm:pt-14", className)}>
      <div className="mb-8 flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-[-0.015em] text-foreground">{title}</h1>
          {description ? <p className="mt-1.5 max-w-[60ch] text-sm leading-5 text-muted-foreground">{description}</p> : null}
        </div>
        {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
      </div>
      <div className="space-y-10">{children}</div>
    </div>
  );
}

export function SettingsSection({ title, description, actions, children, className }: { title?: string; description?: React.ReactNode; actions?: React.ReactNode; children: React.ReactNode; className?: string }) {
  return (
    <section className={className}>
      {title ? (
        <div className="mb-3 flex items-end justify-between gap-3">
          <div>
            <h2 className="text-base font-medium text-foreground">{title}</h2>
            {description ? <p className="mt-0.5 text-sm text-muted-foreground">{description}</p> : null}
          </div>
          {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
        </div>
      ) : null}
      <div className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-panel">{children}</div>
    </section>
  );
}

/** Label and description on the left, control on the right; stacks on narrow screens. */
export function SettingsRow({
  label,
  description,
  children,
  htmlFor,
  stacked = false,
  className,
}: {
  label: React.ReactNode;
  description?: React.ReactNode;
  children?: React.ReactNode;
  htmlFor?: string;
  stacked?: boolean;
  className?: string;
}) {
  return (
    <div className={cn("flex gap-3 px-4 py-3.5", stacked ? "flex-col" : "flex-col sm:flex-row sm:items-center sm:justify-between sm:gap-6", className)}>
      <div className={cn("min-w-0", !stacked && "sm:max-w-[52%]")}>
        <label className="block text-sm font-medium text-foreground" htmlFor={htmlFor}>{label}</label>
        {description ? <div className="mt-0.5 text-xs leading-5 text-muted-foreground">{description}</div> : null}
      </div>
      {children !== undefined ? <div className={cn("min-w-0", stacked ? "w-full" : "w-full sm:w-auto sm:min-w-[240px] sm:max-w-[320px] sm:flex-1 sm:text-right [&>*]:sm:ml-auto")}>{children}</div> : null}
    </div>
  );
}

export function Switch({ className, ...props }: React.ComponentProps<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      className={cn(
        "peer inline-flex h-[18px] w-[30px] shrink-0 cursor-pointer items-center rounded-full border border-transparent bg-border-strong p-[2px] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-primary",
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb className="pointer-events-none block size-3.5 rounded-full bg-white shadow-sm transition-transform data-[state=checked]:translate-x-3 data-[state=unchecked]:translate-x-0" />
    </SwitchPrimitive.Root>
  );
}

/** A value row for read-only settings. */
export function SettingsValue({ label, value, mono = false }: { label: string; value: React.ReactNode; mono?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-6 px-4 py-3 text-sm">
      <span className="text-muted-foreground">{label}</span>
      <span className={cn("min-w-0 truncate text-right text-foreground", mono && "font-mono text-xs")}>{value}</span>
    </div>
  );
}

/** Input with a trailing unit, used for numbers like "30 days". */
export function UnitField({ unit, children }: { unit: string; children: React.ReactNode }) {
  return (
    <div className="relative">
      {children}
      <span className="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-xs text-faint">{unit}</span>
    </div>
  );
}
