import * as DialogPrimitive from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import type * as React from "react";

import { Button } from "./button";
import { cn } from "~/lib/utils";

export const Dialog = DialogPrimitive.Root;
export const DialogTrigger = DialogPrimitive.Trigger;
export const DialogClose = DialogPrimitive.Close;
export const DialogTitle = DialogPrimitive.Title;
export const DialogDescription = DialogPrimitive.Description;

function Overlay({ className }: { className?: string }) {
  return (
    <DialogPrimitive.Overlay
      className={cn("fixed inset-0 z-[80] bg-black/40 data-[state=open]:animate-in data-[state=open]:fade-in-0 dark:bg-black/60", className)}
    />
  );
}

/** Centered modal, sized like Linear's compose and command dialogs. */
export function DialogContent({ className, children, ...props }: React.ComponentProps<typeof DialogPrimitive.Content>) {
  return (
    <DialogPrimitive.Portal>
      <Overlay />
      <DialogPrimitive.Content
        className={cn(
          "fixed left-1/2 top-[14vh] z-[81] flex max-h-[76vh] w-[calc(100%-2rem)] max-w-xl -translate-x-1/2 flex-col overflow-hidden rounded-xl border border-border-strong bg-popover text-foreground shadow-popover outline-none data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-[0.98]",
          className,
        )}
        {...props}
      >
        {children}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}

/** Right-hand peek panel, Linear's pattern for inspecting a row without leaving the list. */
export function SheetContent({
  className,
  children,
  title,
  description,
  actions,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & { title: React.ReactNode; description?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <DialogPrimitive.Portal>
      <Overlay className="bg-black/20 dark:bg-black/40" />
      <DialogPrimitive.Content
        className={cn(
          "fixed inset-y-2 right-2 z-[81] flex w-[calc(100%-1rem)] max-w-2xl flex-col overflow-hidden rounded-xl border border-border-strong bg-panel text-foreground shadow-popover outline-none data-[state=open]:animate-in data-[state=open]:slide-in-from-right-8 data-[state=open]:fade-in-0",
          className,
        )}
        {...props}
      >
        <header className="flex h-12 shrink-0 items-center gap-3 border-b border-border px-4">
          <div className="min-w-0 flex-1">
            <DialogPrimitive.Title className="truncate text-sm font-medium">{title}</DialogPrimitive.Title>
            {description ? <DialogPrimitive.Description className="truncate text-2xs text-muted-foreground">{description}</DialogPrimitive.Description> : null}
          </div>
          {actions}
          <DialogPrimitive.Close asChild>
            <Button aria-label="Close" size="icon-sm" variant="ghost"><X /></Button>
          </DialogPrimitive.Close>
        </header>
        <div className="min-h-0 flex-1 overflow-auto">{children}</div>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}
