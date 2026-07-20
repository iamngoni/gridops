import { createServerFn } from "@tanstack/react-start";

import { loadDashboardOverview } from "./dashboard.server";

export const getDashboardOverview = createServerFn({ method: "GET" }).handler(
  async () => loadDashboardOverview(),
);
