import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** Counts as people read them: 11,770 rather than 11770. */
export function formatCount(value: number) {
  return value.toLocaleString();
}

export function formatMemory(megabytes: number) {
  return megabytes >= 1024 ? `${Math.round(megabytes / 102.4) / 10} GB` : `${megabytes} MB`;
}

/** One runner's size, written the same way everywhere: "2 CPUs · 2 GB". */
export function formatRunnerShape(cpus: number, memoryMb: number) {
  return `${cpus} ${cpus === 1 ? "CPU" : "CPUs"} · ${formatMemory(memoryMb)}`;
}

export function formatDuration(startedAt?: string | null, completedAt?: string | null) {
  if (!startedAt) return "—";

  const start = new Date(startedAt).getTime();
  const end = completedAt ? new Date(completedAt).getTime() : Date.now();
  const seconds = Math.max(0, Math.floor((end - start) / 1000));
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;

  return minutes > 0 ? `${minutes}m ${remainder}s` : `${remainder}s`;
}

/** Compact age for dense rows, as Linear shows it: "now", "5m", "3h", "4d", then a date. */
export function formatAge(value: string) {
  const elapsed = Date.now() - new Date(value).getTime();
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d`;
  return new Date(value).toLocaleDateString([], { month: "short", day: "numeric" });
}

export function formatDateTime(value: string) {
  return new Date(value).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

export function formatRelativeTime(value: string) {
  const elapsed = Date.now() - new Date(value).getTime();
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}
