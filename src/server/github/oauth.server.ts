import "@tanstack/react-start/server-only";

import { createHash } from "node:crypto";

import { eq } from "drizzle-orm";
import { nanoid } from "nanoid";

import { requireOAuthConfig } from "../config.server";
import { hashToken, randomToken, seal, unseal } from "../crypto.server";
import { getDb, getSqlite, migrateDatabase } from "../db/client.server";
import {
  installations,
  oauthStates,
  repositories,
  userInstallations,
  users,
} from "../db/schema";
import { createSession } from "../auth/session.server";
import {
  githubRequest,
  type GitHubInstallation,
  type GitHubRepository,
  type GitHubUser,
} from "./api.server";

type TokenResponse = {
  access_token?: string;
  expires_in?: number;
  refresh_token?: string;
  refresh_token_expires_in?: number;
  token_type?: string;
  error?: string;
  error_description?: string;
};

type GitHubWorkflowRun = {
  id: number;
  workflow_id: number;
  name: string | null;
  display_title: string;
  run_number: number;
  run_attempt: number;
  event: string;
  status: string;
  conclusion: string | null;
  head_branch: string | null;
  head_sha: string;
  actor: { login: string } | null;
  html_url: string;
  run_started_at: string | null;
  created_at: string;
  updated_at: string;
};

function safeReturnTo(value: string | null) {
  if (!value || !value.startsWith("/") || value.startsWith("//")) return "/";
  return value;
}

function codeChallenge(verifier: string) {
  return createHash("sha256").update(verifier).digest("base64url");
}

export function beginGitHubOAuth(request: Request) {
  const config = requireOAuthConfig();
  migrateDatabase();

  const requestUrl = new URL(request.url);
  const state = randomToken();
  const verifier = randomToken(48);
  const redirectUri = new URL("/auth/github/callback", config.baseUrl).toString();

  getDb()
    .insert(oauthStates)
    .values({
      id: nanoid(),
      stateHash: hashToken(state),
      codeVerifier: seal(verifier),
      returnTo: safeReturnTo(requestUrl.searchParams.get("returnTo")),
      expiresAt: new Date(Date.now() + 10 * 60_000),
    })
    .run();

  const authorizeUrl = new URL("https://github.com/login/oauth/authorize");
  authorizeUrl.searchParams.set("client_id", config.githubClientId);
  authorizeUrl.searchParams.set("redirect_uri", redirectUri);
  authorizeUrl.searchParams.set("state", state);
  authorizeUrl.searchParams.set("code_challenge", codeChallenge(verifier));
  authorizeUrl.searchParams.set("code_challenge_method", "S256");

  return Response.redirect(authorizeUrl, 302);
}

export async function completeGitHubOAuth(request: Request) {
  const config = requireOAuthConfig();
  migrateDatabase();

  const requestUrl = new URL(request.url);
  const code = requestUrl.searchParams.get("code");
  const state = requestUrl.searchParams.get("state");
  const oauthError = requestUrl.searchParams.get("error");

  if (oauthError) {
    return redirectWithError(config.baseUrl, requestUrl.searchParams.get("error_description") ?? oauthError);
  }
  if (!code || !state) {
    return redirectWithError(config.baseUrl, "GitHub did not return a valid authorization code.");
  }

  const stateRecord = getDb()
    .select()
    .from(oauthStates)
    .where(eq(oauthStates.stateHash, hashToken(state)))
    .get();

  if (!stateRecord || stateRecord.expiresAt.getTime() <= Date.now()) {
    return redirectWithError(config.baseUrl, "The GitHub authorization request expired or was invalid.");
  }

  getDb().delete(oauthStates).where(eq(oauthStates.id, stateRecord.id)).run();

  const tokenResponse = await fetch("https://github.com/login/oauth/access_token", {
    method: "POST",
    headers: { Accept: "application/json", "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: config.githubClientId,
      client_secret: config.githubClientSecret,
      code,
      redirect_uri: new URL("/auth/github/callback", config.baseUrl).toString(),
      code_verifier: unseal(stateRecord.codeVerifier),
    }),
  });

  const token = (await tokenResponse.json()) as TokenResponse;
  if (!tokenResponse.ok || !token.access_token) {
    return redirectWithError(
      config.baseUrl,
      token.error_description ?? token.error ?? "GitHub token exchange failed.",
    );
  }

  const profile = await githubRequest<GitHubUser>("/user", token.access_token);
  const now = new Date();
  const accessTokenExpiresAt = token.expires_in
    ? new Date(now.getTime() + token.expires_in * 1000)
    : null;
  const refreshTokenExpiresAt = token.refresh_token_expires_in
    ? new Date(now.getTime() + token.refresh_token_expires_in * 1000)
    : null;
  const newUserId = nanoid();

  const userRecord = getDb()
    .insert(users)
    .values({
      id: newUserId,
      githubId: profile.id,
      login: profile.login,
      name: profile.name,
      email: profile.email,
      avatarUrl: profile.avatar_url,
      accessToken: seal(token.access_token),
      accessTokenExpiresAt,
      refreshToken: token.refresh_token ? seal(token.refresh_token) : null,
      refreshTokenExpiresAt,
      lastLoginAt: now,
    })
    .onConflictDoUpdate({
      target: users.githubId,
      set: {
        login: profile.login,
        name: profile.name,
        email: profile.email,
        avatarUrl: profile.avatar_url,
        accessToken: seal(token.access_token),
        accessTokenExpiresAt,
        refreshToken: token.refresh_token ? seal(token.refresh_token) : null,
        refreshTokenExpiresAt,
        lastLoginAt: now,
        updatedAt: now,
      },
    })
    .returning({ id: users.id })
    .get();

  await syncUserInstallations(userRecord.id, token.access_token);

  const headers = new Headers({
    Location: new URL(stateRecord.returnTo, config.baseUrl).toString(),
    "Cache-Control": "no-store",
    "Set-Cookie": createSession(userRecord.id, request),
  });
  return new Response(null, { status: 302, headers });
}

export async function syncUserInstallations(userId: string, accessToken: string) {
  const allInstallations: GitHubInstallation[] = [];
  for (let page = 1; ; page += 1) {
    const response = await githubRequest<{ installations: GitHubInstallation[] }>(
      `/user/installations?per_page=100&page=${page}`,
      accessToken,
    );
    allInstallations.push(...response.installations);
    if (response.installations.length < 100) break;
  }
  const now = new Date();

  for (const installation of allInstallations) {
    if (!installation.account) continue;

    getDb()
      .insert(installations)
      .values({
        id: installation.id,
        accountId: installation.account.id,
        accountLogin: installation.account.login,
        accountType: installation.account.type,
        accountAvatarUrl: installation.account.avatar_url,
        targetType: installation.target_type,
        repositorySelection: installation.repository_selection,
        permissions: installation.permissions,
        events: installation.events,
        suspendedAt: installation.suspended_at ? new Date(installation.suspended_at) : null,
        lastSyncedAt: now,
      })
      .onConflictDoUpdate({
        target: installations.id,
        set: {
          accountLogin: installation.account.login,
          accountType: installation.account.type,
          accountAvatarUrl: installation.account.avatar_url,
          repositorySelection: installation.repository_selection,
          permissions: installation.permissions,
          events: installation.events,
          suspendedAt: installation.suspended_at ? new Date(installation.suspended_at) : null,
          lastSyncedAt: now,
          updatedAt: now,
        },
      })
      .run();

    getDb()
      .insert(userInstallations)
      .values({ userId, installationId: installation.id, permission: "admin" })
      .onConflictDoUpdate({
        target: [userInstallations.userId, userInstallations.installationId],
        set: { permission: "admin" },
      })
      .run();

    const installationRepositories: GitHubRepository[] = [];
    for (let page = 1; ; page += 1) {
      const repositoriesResponse = await githubRequest<{ repositories: GitHubRepository[] }>(
        `/user/installations/${installation.id}/repositories?per_page=100&page=${page}`,
        accessToken,
      );
      installationRepositories.push(...repositoriesResponse.repositories);
      if (repositoriesResponse.repositories.length < 100) break;
    }

    for (const repository of installationRepositories) {
      const permission = repository.permissions
        ? Object.entries(repository.permissions).find(([, allowed]) => allowed)?.[0] ?? null
        : null;
      getDb()
        .insert(repositories)
        .values({
          id: repository.id,
          installationId: installation.id,
          owner: repository.owner.login,
          name: repository.name,
          fullName: repository.full_name,
          private: repository.private,
          archived: repository.archived,
          defaultBranch: repository.default_branch,
          htmlUrl: repository.html_url,
          permission,
          githubUpdatedAt: repository.updated_at ? new Date(repository.updated_at) : null,
          lastSyncedAt: now,
        })
        .onConflictDoUpdate({
          target: repositories.id,
          set: {
            installationId: installation.id,
            owner: repository.owner.login,
            name: repository.name,
            fullName: repository.full_name,
            private: repository.private,
            archived: repository.archived,
            defaultBranch: repository.default_branch,
            htmlUrl: repository.html_url,
            permission,
            githubUpdatedAt: repository.updated_at ? new Date(repository.updated_at) : null,
            lastSyncedAt: now,
            updatedAt: now,
          },
        })
        .run();

      await syncRecentWorkflowRuns(repository, accessToken).catch((error) => {
        const message = error instanceof Error ? error.message : "Unknown workflow sync error";
        console.warn(`[github] Could not sync workflow runs for ${repository.full_name}: ${message}`);
      });
    }
  }

  if (allInstallations.length === 0) {
    getSqlite().prepare("DELETE FROM user_installations WHERE user_id = ?").run(userId);
  } else {
    const ids = allInstallations.map((installation) => installation.id);
    const placeholders = ids.map(() => "?").join(",");
    getSqlite().prepare(`
      DELETE FROM user_installations
      WHERE user_id = ? AND installation_id NOT IN (${placeholders})
    `).run(userId, ...ids);
  }
}

async function syncRecentWorkflowRuns(repository: GitHubRepository, accessToken: string) {
  const response = await githubRequest<{ workflow_runs: GitHubWorkflowRun[] }>(
    `/repos/${repository.owner.login}/${repository.name}/actions/runs?per_page=50`,
    accessToken,
  );
  const statement = getSqlite().prepare(`
    INSERT INTO workflow_runs (
      id, repository_id, workflow_id, workflow_name, run_number, run_attempt,
      event, status, conclusion, head_branch, head_sha, actor_login, html_url,
      started_at, completed_at, github_created_at, github_updated_at, created_at, updated_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET
      workflow_name = excluded.workflow_name,
      run_attempt = excluded.run_attempt,
      status = excluded.status,
      conclusion = excluded.conclusion,
      started_at = excluded.started_at,
      completed_at = excluded.completed_at,
      github_updated_at = excluded.github_updated_at,
      updated_at = excluded.updated_at
  `);
  const now = Date.now();
  const write = getSqlite().transaction(() => {
    for (const run of response.workflow_runs) {
      const updatedAt = Date.parse(run.updated_at);
      statement.run(
        run.id,
        repository.id,
        run.workflow_id,
        run.name ?? run.display_title ?? "Workflow",
        run.run_number,
        run.run_attempt,
        run.event,
        run.status,
        run.conclusion,
        run.head_branch,
        run.head_sha,
        run.actor?.login ?? null,
        run.html_url,
        run.run_started_at ? Date.parse(run.run_started_at) : null,
        run.status === "completed" ? updatedAt : null,
        Date.parse(run.created_at),
        updatedAt,
        now,
        now,
      );
    }
  });
  write();
}

function redirectWithError(baseUrl: string, message: string) {
  const url = new URL("/", baseUrl);
  url.searchParams.set("authError", message);
  return Response.redirect(url, 302);
}
