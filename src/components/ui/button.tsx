import { cva, type VariantProps } from "class-variance-authority";
import type * as React from "react";

import { cn } from "~/lib/utils";

export const buttonVariants = cva(
  "inline-flex h-9 shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-lg border border-transparent px-3 text-sm font-medium transition-[background-color,color,border-color,box-shadow,transform] outline-none focus-visible:ring-2 focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground shadow-[0_1px_0_hsl(150_70%_90%/0.18)_inset,0_1px_3px_hsl(160_80%_2%/0.35)] hover:bg-primary/90",
        destructive:
          "bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/40",
        outline:
          "border-border/80 bg-background/70 text-foreground shadow-[0_1px_0_hsl(150_20%_95%/0.03)_inset] hover:border-border hover:bg-accent hover:text-accent-foreground",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-secondary/80",
        ghost: "text-muted-foreground hover:bg-accent hover:text-foreground",
        link: "h-auto px-0 text-primary underline-offset-4 hover:underline",
      },
      size: {
        default: "h-9 px-3",
        sm: "h-8 rounded-md px-2.5 text-xs",
        lg: "h-10 rounded-md px-5",
        icon: "size-9 px-0",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

export function Button({
  className,
  variant,
  size,
  type = "button",
  ...props
}: React.ComponentProps<"button"> & VariantProps<typeof buttonVariants>) {
  return (
    <button
      data-slot="button"
      type={type}
      className={cn(buttonVariants({ variant, size }), className)}
      {...props}
    />
  );
}
