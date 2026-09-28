import { cn } from "~/lib/utils";

/** Busy runners over online runners over target, drawn as one thin bar. */
export function CapacityMeter({ busy, online, desired, className }: { busy: number; online: number; desired: number; className?: string }) {
  const total = Math.max(desired, online, 1);
  return (
    <span className={cn("hidden w-32 shrink-0 items-center gap-2 md:flex", className)} title={`${busy} busy · ${online} online · ${desired} target`}>
      <span className="relative h-1.5 flex-1 overflow-hidden rounded-full bg-selected">
        <span className="absolute inset-y-0 left-0 rounded-full bg-success/45" style={{ width: `${(online / total) * 100}%` }} />
        <span className="absolute inset-y-0 left-0 rounded-full bg-info" style={{ width: `${(busy / total) * 100}%` }} />
      </span>
      <span className="tabular w-9 text-right text-xs text-muted-foreground">{online}/{desired}</span>
    </span>
  );
}
