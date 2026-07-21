const DEFAULT_RETURN_TO = "/";

export function safeReturnTo(value: unknown): string {
  if (typeof value !== "string" || !value.startsWith("/") || value.startsWith("//")) {
    return DEFAULT_RETURN_TO;
  }

  const pathname = value.split(/[?#]/, 1)[0] ?? DEFAULT_RETURN_TO;
  if (pathname === "/login" || pathname.startsWith("/auth/") || pathname.startsWith("/api/")) {
    return DEFAULT_RETURN_TO;
  }

  return value;
}
