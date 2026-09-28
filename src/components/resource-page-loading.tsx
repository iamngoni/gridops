import type { LucideIcon } from "lucide-react";

import { LoadingRows, PageBody, PageHeader } from "./page";

export function ResourcePageLoading({
  title,
  icon,
}: {
  title: string;
  description?: string;
  icon: LucideIcon;
}) {
  return (
    <>
      <PageHeader icon={icon} title={title} />
      <PageBody>
        <LoadingRows />
      </PageBody>
    </>
  );
}
