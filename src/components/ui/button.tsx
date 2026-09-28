import { cva, type VariantProps } from "class-variance-authority";
import type * as React from "react";

import { cn } from "~/lib/utils";

export const buttonVariants = cva(
  "inline-flex shrink-0 select-none items-center justify-center gap-1.5 whitespace-nowrap rounded-md border border-transparent font-medium transition-[background-color,color,border-color,box-shadow] outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-45 [&_svg]:pointer-events-none [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground shadow-[inset_0_1px_0_rgb(255_255_255/0.12),0_1px_2px_rgb(0_0_0/0.2)] hover:bg-primary-hover",
        destructive: "bg-destructive text-white hover:bg-destructive/90",
        outline: "border-border-strong bg-panel text-foreground shadow-[0_1px_1px_rgb(0_0_0/0.04)] hover:bg-hover",
        secondary: "border-border-strong bg-panel text-foreground shadow-[0_1px_1px_rgb(0_0_0/0.04)] hover:bg-hover",
        ghost: "text-muted-foreground hover:bg-hover hover:text-foreground",
        link: "h-auto border-0 px-0 text-primary underline-offset-4 hover:underline",
      },
      size: {
        default: "h-8 px-3 text-sm [&_svg]:size-4",
        sm: "h-7 px-2.5 text-xs [&_svg]:size-3.5",
        xs: "h-6 px-2 text-xs [&_svg]:size-3.5",
        lg: "h-9 px-4 text-sm [&_svg]:size-4",
        icon: "size-8 px-0 [&_svg]:size-4",
        "icon-sm": "size-7 px-0 [&_svg]:size-4",
        "icon-xs": "size-6 px-0 [&_svg]:size-3.5",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

export type ButtonVariant = NonNullable<VariantProps<typeof buttonVariants>["variant"]>;
export type ButtonSize = NonNullable<VariantProps<typeof buttonVariants>["size"]>;

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
