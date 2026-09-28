import { cva, type VariantProps } from "class-variance-authority";
import type * as React from "react";

import { cn } from "~/lib/utils";

/** Linear-style chip: a quiet bordered pill, optionally led by a colored dot. */
const badgeVariants = cva(
  "inline-flex h-5 max-w-full shrink-0 items-center gap-1.5 truncate rounded-full border px-2 text-2xs font-medium leading-none",
  {
    variants: {
      variant: {
        default: "border-primary/25 bg-primary/10 text-primary",
        secondary: "border-border-strong bg-panel text-secondary-foreground",
        success: "border-border-strong bg-panel text-secondary-foreground",
        warning: "border-border-strong bg-panel text-secondary-foreground",
        destructive: "border-danger/30 bg-danger/10 text-danger",
        info: "border-border-strong bg-panel text-secondary-foreground",
        outline: "border-border-strong bg-transparent text-muted-foreground",
      },
    },
    defaultVariants: { variant: "secondary" },
  },
);

const dotColors = {
  default: "bg-primary",
  secondary: null,
  success: "bg-success",
  warning: "bg-warning",
  destructive: "bg-danger",
  info: "bg-info",
  outline: null,
} as const;

export function Badge({
  className,
  variant,
  dot,
  children,
  ...props
}: React.ComponentProps<"span"> & VariantProps<typeof badgeVariants> & { dot?: string | boolean }) {
  const variantDot = dotColors[variant ?? "secondary"];
  const dotClass = typeof dot === "string" ? dot : dot === false ? null : variantDot;
  return (
    <span data-slot="badge" className={cn(badgeVariants({ variant }), className)} {...props}>
      {dotClass ? <span aria-hidden="true" className={cn("size-1.5 shrink-0 rounded-full", dotClass)} /> : null}
      {children}
    </span>
  );
}
