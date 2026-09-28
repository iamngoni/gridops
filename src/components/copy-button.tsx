import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import type * as React from "react";
import { toast } from "sonner";

import { Button } from "./ui/button";
import { cn } from "~/lib/utils";

const iconClassName = "absolute transition-[opacity,scale,filter] duration-200 ease-out-strong";

/**
 * Copies `value` and confirms in place: the copy icon blurs into a check for a
 * moment, so the feedback lands where the person is looking.
 */
export function CopyButton({
  value,
  label,
  children,
  onClick,
  ...props
}: Omit<React.ComponentProps<typeof Button>, "value"> & { value: string; label: string }) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return undefined;
    const timeout = window.setTimeout(() => setCopied(false), 1_600);
    return () => window.clearTimeout(timeout);
  }, [copied]);

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
    } catch {
      toast.error(`Couldn’t copy the ${label.toLowerCase()} to the clipboard.`);
    }
  }

  return (
    // Wrappers such as a tooltip trigger pass their own click handler; keep it and copy too.
    <Button aria-label={children ? undefined : `Copy ${label}`} {...props} onClick={(event) => { onClick?.(event); void copy(); }}>
      <span aria-hidden="true" className="relative grid size-4 place-items-center">
        <Copy className={cn(iconClassName, copied && "scale-50 opacity-0 blur-[2px]")} />
        <Check className={cn(iconClassName, "text-success", !copied && "scale-50 opacity-0 blur-[2px]")} />
      </span>
      {children ? <span>{copied ? "Copied" : children}</span> : null}
      <span aria-live="polite" className="sr-only">{copied ? `${label} copied` : ""}</span>
    </Button>
  );
}
