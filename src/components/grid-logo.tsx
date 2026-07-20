import { cn } from "~/lib/utils";

export function GridLogo({ compact = false, className }: { compact?: boolean; className?: string }) {
  return (
    <div className={cn("flex items-center gap-2.5", className)} aria-label="GridOps">
      <span className="grid size-6 grid-cols-3 gap-[2px]" aria-hidden="true">
        {Array.from({ length: 9 }, (_, index) => (
          <span
            key={index}
            className={cn(
              "rounded-[1px] bg-zinc-600",
              index === 2 && "bg-emerald-400",
              index === 4 && "bg-zinc-300",
            )}
          />
        ))}
      </span>
      {!compact && <span className="text-base font-semibold tracking-tight text-foreground">GridOps</span>}
    </div>
  );
}
