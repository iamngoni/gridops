import { CircleCheck, CircleX, Layers3 } from "lucide-react";

import { Badge } from "~/components/ui/badge";
import { cn } from "~/lib/utils";

type Provider = "docker" | "tart";

export function PoolPlanPreview({
  name,
  providers,
  labels,
  desiredCount,
  maxCount,
  repositoryCount,
  scope,
}: {
  name: string;
  providers: Provider[];
  labels: string[];
  desiredCount: number;
  maxCount: number;
  repositoryCount: number;
  scope: "repository" | "organization";
}) {
  const normalizedName = name.trim();
  const customLabels = labels.filter(Boolean);
  const repositoryCapacityOk = scope === "organization" || repositoryCount <= maxCount;
  const ready = Boolean(normalizedName) && providers.length > 0 && repositoryCapacityOk;
  return (
    <div className="rounded-md border border-border bg-muted/15 p-3 sm:col-span-2 xl:col-span-3">
      <div className="flex items-start gap-3">
        <div className="grid size-7 shrink-0 place-items-center rounded-md border border-primary/20 bg-primary/10 text-primary"><Layers3 className="size-4" /></div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2"><p className="text-xs font-medium">Preflight plan</p><Badge variant={ready ? "success" : "warning"}>{ready ? "ready to save" : "needs attention"}</Badge></div>
          <p className="mt-1 text-[11px] leading-5 text-muted-foreground">GridOps will register the pool name and selected provider labels automatically. This preview does not reserve capacity; admission happens immediately before each runner starts.</p>
          <div className="mt-3 grid gap-2 lg:grid-cols-2">
            {providers.map((provider) => <div className="rounded border border-border bg-background/60 p-2" key={provider}>
              <p className="text-[11px] font-medium">{provider === "tart" ? "Tart · macOS ARM64" : "Docker · Linux"}</p>
              <div className="mt-1 flex flex-wrap gap-1">
                {(provider === "tart" ? ["self-hosted", "macOS", "ARM64"] : ["self-hosted", "Linux", "host architecture"])
                  .concat(normalizedName ? [normalizedName] : [])
                  .concat(customLabels)
                  .map((label) => <Badge key={label} variant="outline">{label}</Badge>)}
              </div>
            </div>)}
          </div>
          <div className="mt-3 grid gap-2 text-[11px] leading-5 sm:grid-cols-2">
            <PlanCheck ok={repositoryCapacityOk} text={scope === "organization" ? "GitHub runner-group access defines repository eligibility." : `${repositoryCount} selected repositories fit within the ${maxCount}-runner maximum.`} />
            <PlanCheck ok={desiredCount <= maxCount} text={`${desiredCount} runners targeted now; autoscaling may grow to ${maxCount}.`} />
          </div>
        </div>
      </div>
    </div>
  );
}

function PlanCheck({ ok, text }: { ok: boolean; text: string }) {
  const Icon = ok ? CircleCheck : CircleX;
  return <div className={cn("flex gap-2 rounded border px-2 py-1.5", ok ? "border-emerald-500/20 bg-emerald-500/5" : "border-red-500/30 bg-red-500/5")}><Icon className={cn("mt-0.5 size-3.5 shrink-0", ok ? "text-emerald-500" : "text-red-500")} /><span>{text}</span></div>;
}
