import type * as React from "react";

import { cn } from "~/lib/utils";

export function Kbd({ className, ...props }: React.ComponentProps<"kbd">) {
  return (
    <kbd
      className={cn(
        "inline-flex h-[18px] min-w-[18px] items-center justify-center rounded-[4px] border border-border-strong bg-panel-subtle px-1 font-sans text-[10px] font-medium leading-none text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

/** Renders a shortcut such as "G then P" or "⌘ K" as separate keys. */
export function Shortcut({ keys, className }: { keys: string[]; className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-1", className)}>
      {keys.map((key, index) => (
        <span className="inline-flex items-center gap-1" key={`${key}-${index}`}>
          {index > 0 && keys.length === 2 && key.length === 1 && keys[0]?.length === 1 ? <span className="text-[10px] text-faint">then</span> : null}
          <Kbd>{key}</Kbd>
        </span>
      ))}
    </span>
  );
}
