import * as DropdownMenuPrimitive from "@radix-ui/react-dropdown-menu";
import { Check } from "lucide-react";
import { cloneElement, isValidElement } from "react";
import type * as React from "react";

import { Shortcut } from "./kbd";
import { cn } from "~/lib/utils";

export const DropdownMenu = DropdownMenuPrimitive.Root;
export const DropdownMenuTrigger = DropdownMenuPrimitive.Trigger;
export const DropdownMenuGroup = DropdownMenuPrimitive.Group;

export function DropdownMenuContent({
  className,
  sideOffset = 4,
  align = "end",
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Content>) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        align={align}
        className={cn(
          "z-[90] min-w-48 overflow-hidden rounded-lg border border-border-strong bg-popover p-1 text-sm text-foreground shadow-popover data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-1",
          className,
        )}
        sideOffset={sideOffset}
        {...props}
      />
    </DropdownMenuPrimitive.Portal>
  );
}

export function DropdownMenuItem({
  className,
  icon,
  shortcut,
  destructive,
  asChild,
  children,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Item> & { icon?: React.ReactNode; shortcut?: string[]; destructive?: boolean }) {
  const itemClassName = cn(
    "relative flex h-8 cursor-default select-none items-center gap-2 rounded-md px-2 text-sm text-foreground outline-none data-[disabled]:pointer-events-none data-[disabled]:opacity-45 data-[highlighted]:bg-hover [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-muted-foreground",
    destructive && "text-danger data-[highlighted]:bg-danger/10 [&_svg]:text-danger",
    className,
  );
  const content = (label: React.ReactNode) => (
    <>
      {icon}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {shortcut ? <Shortcut keys={shortcut} /> : null}
    </>
  );
  // Radix's Slot needs exactly one child, so links get the icon and label injected instead.
  if (asChild && isValidElement<{ children?: React.ReactNode }>(children)) {
    return (
      <DropdownMenuPrimitive.Item asChild className={itemClassName} {...props}>
        {cloneElement(children, undefined, content(children.props.children))}
      </DropdownMenuPrimitive.Item>
    );
  }
  return (
    <DropdownMenuPrimitive.Item className={itemClassName} {...props}>
      {content(children)}
    </DropdownMenuPrimitive.Item>
  );
}

export function DropdownMenuCheckboxItem({
  className,
  children,
  checked,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.CheckboxItem>) {
  return (
    <DropdownMenuPrimitive.CheckboxItem
      checked={checked}
      className={cn(
        "relative flex h-8 cursor-default select-none items-center gap-2 rounded-md pl-2 pr-8 text-sm outline-none data-[highlighted]:bg-hover",
        className,
      )}
      {...props}
    >
      {children}
      <DropdownMenuPrimitive.ItemIndicator className="absolute right-2 inline-flex">
        <Check className="size-4 text-foreground" />
      </DropdownMenuPrimitive.ItemIndicator>
    </DropdownMenuPrimitive.CheckboxItem>
  );
}

export function DropdownMenuLabel({ className, ...props }: React.ComponentProps<typeof DropdownMenuPrimitive.Label>) {
  return <DropdownMenuPrimitive.Label className={cn("px-2 pb-1 pt-1.5 text-2xs font-medium text-faint", className)} {...props} />;
}

export function DropdownMenuSeparator({ className, ...props }: React.ComponentProps<typeof DropdownMenuPrimitive.Separator>) {
  return <DropdownMenuPrimitive.Separator className={cn("-mx-1 my-1 h-px bg-border", className)} {...props} />;
}
