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
            "z-[100] flex max-w-72 items-center gap-2 rounded-md border border-border-strong bg-popover px-2 py-1 text-xs text-foreground shadow-popover animate-in fade-in-0 zoom-in-95",
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
