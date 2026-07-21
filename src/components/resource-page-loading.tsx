import type { LucideIcon } from "lucide-react";
import { LoaderCircle } from "lucide-react";

import { ResourcePage } from "./resource-page";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";

export function ResourcePageLoading({
  title,
  description,
  icon,
}: {
  title: string;
  description: string;
  icon: LucideIcon;
}) {
  return (
    <ResourcePage
      title={title}
      description={description}
      icon={icon}
      emptyTitle="Loading"
      emptyDescription="GridOps is loading this view."
    >
      <Card aria-busy="true" aria-live="polite">
        <CardHeader>
          <div>
            <CardTitle>Loading data</CardTitle>
            <p className="mt-1 text-xs text-muted-foreground">The page is ready; its operational data is still arriving.</p>
          </div>
          <LoaderCircle className="size-4 animate-spin text-primary" />
        </CardHeader>
        <CardContent className="space-y-3">
          {Array.from({ length: 5 }, (_, index) => (
            <div className="grid grid-cols-[minmax(0,1.5fr)_minmax(5rem,0.6fr)_minmax(5rem,0.4fr)] gap-4" key={index}>
              <div className="h-9 animate-pulse rounded bg-muted" />
              <div className="h-9 animate-pulse rounded bg-muted" />
              <div className="h-9 animate-pulse rounded bg-muted" />
            </div>
          ))}
        </CardContent>
      </Card>
    </ResourcePage>
  );
}
