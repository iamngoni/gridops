import type * as React from "react";

import { cn } from "~/lib/utils";

export const inputClassName =
  "h-8 w-full min-w-0 rounded-md border border-border-strong bg-panel px-2.5 text-sm text-foreground shadow-[0_1px_1px_rgb(0_0_0/0.03)] outline-none transition-[border-color,box-shadow] placeholder:text-faint focus:border-primary/70 focus:ring-2 focus:ring-primary/20 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-danger/60";

export function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return <input data-slot="input" type={type} className={cn(inputClassName, className)} {...props} />;
}

export function Textarea({ className, ...props }: React.ComponentProps<"textarea">) {
  return <textarea data-slot="textarea" className={cn(inputClassName, "h-auto min-h-20 py-2 leading-5", className)} {...props} />;
}
