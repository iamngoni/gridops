import { cn } from "~/lib/utils";

/** GridOps mark: a 3×3 grid with one live cell, sized like a Linear workspace avatar. */
export function GridMark({ size = 20, className }: { size?: number; className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn("grid shrink-0 grid-cols-3 gap-[1.5px] rounded-[5px] bg-primary p-[3.5px]", className)}
      style={{ width: size, height: size }}
    >
      {Array.from({ length: 9 }, (_, index) => (
        <span className={cn("rounded-[1px] bg-white/35", index === 2 && "bg-white", index === 4 && "bg-white/80")} key={index} />
      ))}
    </span>
  );
}

export function GridLogo({ compact = false, className }: { compact?: boolean; className?: string }) {
  return (
    <div aria-label="GridOps" className={cn("flex items-center gap-2", className)}>
      <GridMark size={22} />
      {!compact && <span className="text-base font-semibold tracking-[-0.01em] text-foreground">GridOps</span>}
    </div>
  );
}
