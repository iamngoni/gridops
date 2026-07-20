import "@tanstack/react-start/server-only";

import { eq } from "drizzle-orm";

import { requireOAuthConfig } from "../config.server";
import { seal, unseal } from "../crypto.server";
import { getDb, migrateDatabase } from "../db/client.server";
import { users } from "../db/schema";

type RefreshResponse = {
  access_token?: string;
  expires_in?: number;
  refresh_token?: string;
  refresh_token_expires_in?: number;
  error?: string;
  error_description?: string;
};

export async function getValidGitHubAccessToken(userId: string) {
  migrateDatabase();
  const user = getDb().select().from(users).where(eq(users.id, userId)).get();
  if (!user) throw new Error("GridOps user does not exist.");

  const refreshThreshold = Date.now() + 5 * 60_000;
  if (!user.accessTokenExpiresAt || user.accessTokenExpiresAt.getTime() > refreshThreshold) {
    return unseal(user.accessToken);
  }
  if (!user.refreshToken) {
    throw new Error("GitHub authorization expired. Connect GitHub again.");
  }

  const config = requireOAuthConfig();
  const response = await fetch("https://github.com/login/oauth/access_token", {
    method: "POST",
    headers: { Accept: "application/json", "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: config.githubClientId,
      client_secret: config.githubClientSecret,
      grant_type: "refresh_token",
      refresh_token: unseal(user.refreshToken),
    }),
  });
  const token = (await response.json()) as RefreshResponse;
  if (!response.ok || !token.access_token) {
    throw new Error(
      token.error_description ?? token.error ?? "GitHub access token refresh failed.",
    );
  }

  const now = new Date();
  getDb()
    .update(users)
    .set({
      accessToken: seal(token.access_token),
      accessTokenExpiresAt: token.expires_in
        ? new Date(now.getTime() + token.expires_in * 1000)
        : null,
      refreshToken: token.refresh_token ? seal(token.refresh_token) : user.refreshToken,
      refreshTokenExpiresAt: token.refresh_token_expires_in
        ? new Date(now.getTime() + token.refresh_token_expires_in * 1000)
        : user.refreshTokenExpiresAt,
      updatedAt: now,
    })
    .where(eq(users.id, userId))
    .run();

  return token.access_token;
}
