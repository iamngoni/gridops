import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import type * as React from "react";

import { Shortcut } from "./kbd";
import { cn } from "~/lib/utils";

export const TooltipProvider = TooltipPrimitive.Provider;

export function Tooltip({
  content,
  shortcut,
  side = "bottom",
  children,
  className,
}: {
  content: React.ReactNode;
  shortcut?: string[];
  side?: "top" | "right" | "bottom" | "left";
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <TooltipPrimitive.Root>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          className={cn(
            // A tooltip opened while another was just showing (Radix's "instant-open")
            // appears without animation, so scanning a toolbar feels immediate.
            "z-[100] flex max-w-72 origin-(--radix-tooltip-content-transform-origin) items-center gap-2 rounded-md border border-border-strong bg-popover px-2 py-1 text-xs text-foreground shadow-popover ease-out-strong data-[state=delayed-open]:animate-in data-[state=delayed-open]:fade-in-0 data-[state=delayed-open]:zoom-in-[0.97] data-[state=delayed-open]:duration-150 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:duration-100",
            className,
          )}
          side={side}
          sideOffset={6}
        >
          <span>{content}</span>
          {shortcut ? <Shortcut keys={shortcut} /> : null}
        </TooltipPrimitive.Content>
      </TooltipPrimitive.Portal>
    </TooltipPrimitive.Root>
  );
}
