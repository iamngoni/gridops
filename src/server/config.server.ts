import "@tanstack/react-start/server-only";

import { z } from "zod";

const optionalString = z.preprocess(
  (value) => (typeof value === "string" && value.trim() === "" ? undefined : value),
  z.string().min(1).optional(),
);

const configSchema = z.object({
  baseUrl: z.string().url(),
  databasePath: z.string().min(1),
  logDirectory: z.string().min(1),
  githubClientId: optionalString,
  githubClientSecret: optionalString,
  githubAppId: optionalString,
  githubAppPrivateKey: optionalString,
  githubAppSlug: z.string().min(1),
  githubWebhookSecret: optionalString,
  sessionSecret: optionalString,
  encryptionKey: optionalString,
  managerUrl: z.string().url(),
  managerToken: optionalString,
  dockerSocket: z.string().min(1),
  runnerNetwork: z.string().min(1),
  runnerImage: z.string().min(1),
});

export type GridOpsConfig = z.infer<typeof configSchema>;

export function getConfig(): GridOpsConfig {
  return configSchema.parse({
    baseUrl: process.env.GRIDOPS_BASE_URL ?? "http://localhost:3000",
    databasePath: process.env.GRIDOPS_DATABASE_PATH ?? "./data/gridops.sqlite",
    logDirectory: process.env.GRIDOPS_LOG_DIRECTORY ?? "./data/logs",
    githubClientId: process.env.GITHUB_CLIENT_ID,
    githubClientSecret: process.env.GITHUB_CLIENT_SECRET,
    githubAppId: process.env.GITHUB_APP_ID,
    githubAppPrivateKey: process.env.GITHUB_APP_PRIVATE_KEY?.replaceAll("\\n", "\n"),
    githubAppSlug: process.env.GITHUB_APP_SLUG ?? "gridops",
    githubWebhookSecret: process.env.GITHUB_WEBHOOK_SECRET,
    sessionSecret: process.env.GRIDOPS_SESSION_SECRET,
    encryptionKey: process.env.GRIDOPS_ENCRYPTION_KEY,
    managerUrl: process.env.GRIDOPS_MANAGER_URL ?? "http://localhost:8788",
    managerToken: process.env.GRIDOPS_MANAGER_TOKEN,
    dockerSocket: process.env.GRIDOPS_DOCKER_SOCKET ?? "/var/run/docker.sock",
    runnerNetwork: process.env.GRIDOPS_RUNNER_NETWORK ?? "gridops-runners",
    runnerImage:
      process.env.GRIDOPS_RUNNER_IMAGE ?? "ghcr.io/actions/actions-runner:latest",
  });
}

export function getConfigurationState() {
  const config = getConfig();

  return {
    githubOAuth:
      Boolean(config.githubClientId) && Boolean(config.githubClientSecret),
    githubAppControl:
      (Boolean(config.githubAppId) && Boolean(config.githubAppPrivateKey)) ||
      (Boolean(config.githubClientId) && Boolean(config.githubClientSecret)),
    webhookVerification: Boolean(config.githubWebhookSecret),
    secureStorage: Boolean(config.encryptionKey) && Boolean(config.sessionSecret),
    runnerManager: Boolean(config.managerToken),
    callbackUrl: new URL("/auth/github/callback", config.baseUrl).toString(),
    webhookUrl: new URL("/api/webhooks/github", config.baseUrl).toString(),
  };
}

export function requireOAuthConfig() {
  const config = getConfig();

  if (!config.githubClientId || !config.githubClientSecret) {
    throw new Error("GitHub App OAuth credentials are not configured.");
  }

  return config as GridOpsConfig & {
    githubClientId: string;
    githubClientSecret: string;
  };
}
