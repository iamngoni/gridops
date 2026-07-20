import "@tanstack/react-start/server-only";

import { eq } from "drizzle-orm";
import { nanoid } from "nanoid";

import { hashToken, randomToken, signValue, unseal, verifySignature } from "../crypto.server";
import { getDb, migrateDatabase } from "../db/client.server";
import { sessions, users } from "../db/schema";

const SESSION_COOKIE = "gridops_session";
const SESSION_TTL_SECONDS = 60 * 60 * 24 * 30;

function parseCookies(request: Request) {
  const header = request.headers.get("cookie") ?? "";
  return new Map(
    header
      .split(";")
      .map((part) => part.trim())
      .filter(Boolean)
      .map((part) => {
        const separator = part.indexOf("=");
        return [
          decodeURIComponent(separator === -1 ? part : part.slice(0, separator)),
          decodeURIComponent(separator === -1 ? "" : part.slice(separator + 1)),
        ];
      }),
  );
}

function cookieValue(token: string) {
  return `${token}.${signValue(token)}`;
}

function serializeSessionCookie(token: string, request: Request) {
  const secure = new URL(request.url).protocol === "https:";
  return [
    `${SESSION_COOKIE}=${encodeURIComponent(cookieValue(token))}`,
    "Path=/",
    "HttpOnly",
    "SameSite=Lax",
    `Max-Age=${SESSION_TTL_SECONDS}`,
    secure ? "Secure" : null,
  ]
    .filter(Boolean)
    .join("; ");
}

export function clearSessionCookie(request: Request) {
  const secure = new URL(request.url).protocol === "https:";
  return [
    `${SESSION_COOKIE}=`,
    "Path=/",
    "HttpOnly",
    "SameSite=Lax",
    "Max-Age=0",
    secure ? "Secure" : null,
  ]
    .filter(Boolean)
    .join("; ");
}

export function createSession(userId: string, request: Request) {
  migrateDatabase();
  const token = randomToken();
  const now = new Date();
  const expiresAt = new Date(now.getTime() + SESSION_TTL_SECONDS * 1000);

  getDb()
    .insert(sessions)
    .values({
      id: nanoid(),
      tokenHash: hashToken(token),
      userId,
      userAgent: request.headers.get("user-agent"),
      ipAddress:
        request.headers.get("x-forwarded-for")?.split(",")[0]?.trim() ?? null,
      expiresAt,
      lastSeenAt: now,
    })
    .run();

  return serializeSessionCookie(token, request);
}

export function deleteSession(request: Request) {
  const value = parseCookies(request).get(SESSION_COOKIE);
  if (!value) return;
  const [token] = value.split(".");
  if (!token) return;

  migrateDatabase();
  getDb().delete(sessions).where(eq(sessions.tokenHash, hashToken(token))).run();
}

export function getSessionUser(request: Request) {
  const value = parseCookies(request).get(SESSION_COOKIE);
  if (!value) return null;
  const [token, signature] = value.split(".");
  if (!token || !signature || !verifySignature(token, signature)) return null;

  migrateDatabase();
  const record = getDb()
    .select({
      sessionId: sessions.id,
      expiresAt: sessions.expiresAt,
      userId: users.id,
      githubId: users.githubId,
      login: users.login,
      name: users.name,
      email: users.email,
      avatarUrl: users.avatarUrl,
      accessToken: users.accessToken,
      accessTokenExpiresAt: users.accessTokenExpiresAt,
      refreshToken: users.refreshToken,
      refreshTokenExpiresAt: users.refreshTokenExpiresAt,
    })
    .from(sessions)
    .innerJoin(users, eq(users.id, sessions.userId))
    .where(eq(sessions.tokenHash, hashToken(token)))
    .get();

  if (!record || record.expiresAt.getTime() <= Date.now()) {
    if (record) {
      getDb().delete(sessions).where(eq(sessions.id, record.sessionId)).run();
    }
    return null;
  }

  getDb()
    .update(sessions)
    .set({ lastSeenAt: new Date() })
    .where(eq(sessions.id, record.sessionId))
    .run();

  return {
    id: record.userId,
    githubId: record.githubId,
    login: record.login,
    name: record.name,
    email: record.email,
    avatarUrl: record.avatarUrl,
    accessToken: unseal(record.accessToken),
    accessTokenExpiresAt: record.accessTokenExpiresAt,
    refreshToken: record.refreshToken ? unseal(record.refreshToken) : null,
    refreshTokenExpiresAt: record.refreshTokenExpiresAt,
  };
}
