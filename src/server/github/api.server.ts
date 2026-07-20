import "@tanstack/react-start/server-only";

const API_VERSION = "2026-03-10";

export type GitHubUser = {
  id: number;
  login: string;
  name: string | null;
  email: string | null;
  avatar_url: string;
};

export type GitHubInstallation = {
  id: number;
  account: {
    id: number;
    login: string;
    type: string;
    avatar_url: string;
  } | null;
  target_type: string;
  repository_selection: string;
  permissions: Record<string, string>;
  events: string[];
  suspended_at: string | null;
};

export type GitHubRepository = {
  id: number;
  name: string;
  full_name: string;
  private: boolean;
  archived: boolean;
  default_branch: string;
  html_url: string;
  updated_at: string | null;
  owner: { login: string };
  permissions?: Record<string, boolean>;
};

export async function githubRequest<T>(
  path: string,
  accessToken: string,
  init: RequestInit = {},
) {
  const response = await fetch(`https://api.github.com${path}`, {
    ...init,
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${accessToken}`,
      "X-GitHub-Api-Version": API_VERSION,
      "User-Agent": "GridOps",
      ...init.headers,
    },
  });

  if (!response.ok) {
    const details = await response.text();
    throw new Error(
      `GitHub API request failed (${response.status} ${response.statusText}): ${details.slice(0, 500)}`,
    );
  }

  if (response.status === 204 || response.headers.get("content-length") === "0") {
    return undefined as T;
  }

  const body = await response.text();
  return (body ? JSON.parse(body) : undefined) as T;
}
