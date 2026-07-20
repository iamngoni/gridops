import "@tanstack/react-start/server-only";

import { getConfig } from "../config.server";

export type RunnerManagerHealth = {
  status: "ok";
  dockerVersion: string;
  apiVersion: string;
};

export async function runnerManagerRequest<T>(path: string, init: RequestInit = {}) {
  const config = getConfig();
  if (!config.managerToken) {
    throw new Error("GRIDOPS_MANAGER_TOKEN is not configured.");
  }

  const response = await fetch(new URL(path, config.managerUrl), {
    ...init,
    headers: {
      Authorization: `Bearer ${config.managerToken}`,
      "Content-Type": "application/json",
      ...init.headers,
    },
  });

  if (!response.ok) {
    const message = await response.text();
    throw new Error(`Runner manager request failed (${response.status}): ${message.slice(0, 500)}`);
  }

  const contentType = response.headers.get("content-type") ?? "";
  return (contentType.includes("application/json") ? response.json() : response.text()) as Promise<T>;
}

export function getRunnerManagerHealth() {
  return runnerManagerRequest<RunnerManagerHealth>("/v1/health");
}
