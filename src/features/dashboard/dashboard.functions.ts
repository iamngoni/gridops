import type { DashboardOverview } from "./types";
import { api } from "~/lib/api";

export function getDashboardOverview() {
  return api<DashboardOverview>("/api/v1/overview");
}
