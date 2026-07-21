import type * as React from "react";

import { cn } from "~/lib/utils";

export function Card({ className, ...props }: React.ComponentProps<"section">) {
  return (
    <section
      data-slot="card"
      className={cn("rounded-xl border border-border/70 bg-card text-card-foreground", className)}
      {...props}
    />
  );
}

export function CardHeader({ className, ...props }: React.ComponentProps<"header">) {
  return <header className={cn("flex items-start justify-between gap-4 p-5", className)} {...props} />;
}

export function CardTitle({ className, ...props }: React.ComponentProps<"h2">) {
  return <h2 className={cn("text-sm font-semibold tracking-[-0.01em]", className)} {...props} />;
}

export function CardDescription({ className, ...props }: React.ComponentProps<"p">) {
  return <p className={cn("text-xs text-muted-foreground", className)} {...props} />;
}

export function CardContent({ className, ...props }: React.ComponentProps<"div">) {
  return <div className={cn("px-5 pb-5", className)} {...props} />;
}
