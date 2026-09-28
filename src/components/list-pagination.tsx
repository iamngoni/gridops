import { ChevronLeft, ChevronRight } from "lucide-react";

import { Button } from "~/components/ui/button";
import { pageNumbers } from "~/lib/pagination";
import { formatCount } from "~/lib/utils";

type ListPaginationProps = {
  /** Previous/next only, for narrow panes. */
  compact?: boolean;
  itemCount: number;
  noun: string;
  onPageChange: (page: number) => void;
  page: number;
  perPage: number;
  total: number;
};

/** Quiet list footer: range on the left, compact page stepper on the right. A single page needs neither. */
export function ListPagination({ compact = false, itemCount, noun, onPageChange, page, perPage, total }: ListPaginationProps) {
  const totalPages = Math.max(1, Math.ceil(total / perPage));
  if (total === 0 || totalPages === 1) return null;
  const currentPage = Math.min(page, totalPages);
  const start = (currentPage - 1) * perPage + 1;
  const end = start + itemCount - 1;

  return (
    <nav aria-label={`${noun} pagination`} className="flex min-h-11 flex-wrap items-center gap-x-3 gap-y-1 px-4 py-2 text-xs text-muted-foreground">
      <span className="tabular">{formatCount(start)}–{formatCount(end)} of {formatCount(total)}{compact ? "" : ` ${noun}`}</span>
      {totalPages > 1 ? (
        <div className="ml-auto flex max-w-full flex-nowrap items-center gap-0.5 overflow-x-auto">
          <Button aria-label={`Previous ${noun} page`} disabled={currentPage === 1} onClick={() => onPageChange(currentPage - 1)} size="icon-xs" variant="ghost"><ChevronLeft /></Button>
          {compact ? null : pageNumbers(currentPage, totalPages).map((pageNumber) => (
            <Button
              aria-current={pageNumber === currentPage ? "page" : undefined}
              aria-label={`${noun} page ${pageNumber}`}
              className={pageNumber === currentPage ? "bg-selected text-foreground" : undefined}
              key={pageNumber}
              onClick={() => onPageChange(pageNumber)}
              size="icon-xs"
              variant="ghost"
            >
              <span className="tabular text-xs">{pageNumber}</span>
            </Button>
          ))}
          <Button aria-label={`Next ${noun} page`} disabled={currentPage === totalPages} onClick={() => onPageChange(currentPage + 1)} size="icon-xs" variant="ghost"><ChevronRight /></Button>
        </div>
      ) : null}
    </nav>
  );
}
