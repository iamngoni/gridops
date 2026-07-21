import { ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight } from "lucide-react";

import { Button } from "~/components/ui/button";
import { pageNumbers } from "~/lib/pagination";

type ListPaginationProps = {
  itemCount: number;
  noun: string;
  onPageChange: (page: number) => void;
  page: number;
  perPage: number;
  total: number;
};

export function ListPagination({ itemCount, noun, onPageChange, page, perPage, total }: ListPaginationProps) {
  if (total === 0) return null;
  const totalPages = Math.max(1, Math.ceil(total / perPage));
  const currentPage = Math.min(page, totalPages);
  const start = (currentPage - 1) * perPage + 1;
  const end = start + itemCount - 1;

  return (
    <nav aria-label={`${noun} pagination`} className="flex flex-col gap-3 border-t border-border px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
      <p className="text-xs text-muted-foreground">
        Showing {start}–{end} of {total} {noun}
      </p>
      {totalPages > 1 ? (
        <div className="flex flex-wrap items-center gap-1">
          <Button aria-label={`First ${noun} page`} disabled={currentPage === 1} onClick={() => onPageChange(1)} size="icon" variant="outline"><ChevronsLeft /></Button>
          <Button aria-label={`Previous ${noun} page`} disabled={currentPage === 1} onClick={() => onPageChange(currentPage - 1)} size="icon" variant="outline"><ChevronLeft /></Button>
          {pageNumbers(currentPage, totalPages).map((pageNumber) => (
            <Button
              aria-current={pageNumber === currentPage ? "page" : undefined}
              aria-label={`${noun} page ${pageNumber}`}
              key={pageNumber}
              onClick={() => onPageChange(pageNumber)}
              size="icon"
              variant={pageNumber === currentPage ? "secondary" : "outline"}
            >
              {pageNumber}
            </Button>
          ))}
          <Button aria-label={`Next ${noun} page`} disabled={currentPage === totalPages} onClick={() => onPageChange(currentPage + 1)} size="icon" variant="outline"><ChevronRight /></Button>
          <Button aria-label={`Last ${noun} page`} disabled={currentPage === totalPages} onClick={() => onPageChange(totalPages)} size="icon" variant="outline"><ChevronsRight /></Button>
        </div>
      ) : null}
    </nav>
  );
}
