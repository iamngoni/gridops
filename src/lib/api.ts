export type Viewer = {
  id: string;
  githubId: number;
  login: string;
  name: string | null;
  email: string | null;
  avatarUrl: string | null;
  alerts: { failedRunners: number; failedWebhooks: number; queuedJobs: number; deferredRunnerCleanup: number };
};

type ApiOptions = Omit<RequestInit, "body"> & { body?: unknown };

export async function api<T>(path: string, options: ApiOptions = {}): Promise<T> {
  const headers = new Headers(options.headers);
  headers.set("Accept", "application/json");
  if (options.body !== undefined) headers.set("Content-Type", "application/json");
  const response = await fetch(path, {
    ...options,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
    credentials: "same-origin",
    headers,
  });
  if (!response.ok) {
    const error = await response.json().catch(() => null) as { error?: string } | null;
    throw new Error(error?.error ?? `GridOps request failed (${response.status}).`);
  }
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

export async function getViewer(): Promise<Viewer | null> {
  try {
    return await api<Viewer>("/api/v1/auth/me");
  } catch (error) {
    if (error instanceof Error && error.message === "authentication required") return null;
    throw error;
  }
}
