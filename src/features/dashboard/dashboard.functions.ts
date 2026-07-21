import type { CapacityHistory, CapacityWindow, DashboardOverview } from "./types";
import { api } from "~/lib/api";

export function getDashboardOverview() {
  return api<DashboardOverview>("/api/v1/overview");
}

export function getCapacityHistory(window: CapacityWindow) {
  return api<CapacityHistory>(`/api/v1/overview/capacity?window=${window}`);
}
