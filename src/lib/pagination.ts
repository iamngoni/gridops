export const DEFAULT_PAGE_SIZE = 25;

export function parsePage(value: unknown) {
  const page = typeof value === "number" ? value : typeof value === "string" ? Number(value) : 1;
  return Number.isFinite(page) && page >= 1 ? Math.floor(page) : 1;
}

export function validatePageSearch(search: Record<string, unknown>): { page?: number } {
  const page = parsePage(search.page);
  return page === 1 ? {} : { page };
}

export function pageNumbers(page: number, totalPages: number) {
  const start = Math.max(1, Math.min(page - 2, totalPages - 4));
  const end = Math.min(totalPages, start + 4);
  return Array.from({ length: end - start + 1 }, (_, index) => start + index);
}
