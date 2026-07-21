import { Check, ChevronDown, Search, X } from "lucide-react";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";

import { cn } from "~/lib/utils";

export type SearchableSelectValue = string | number;

export type SearchableSelectOption<TValue extends SearchableSelectValue> = {
  value: TValue;
  label: string;
  description?: string;
  keywords?: string[];
};

type SearchableSelectProps<TValue extends SearchableSelectValue> = {
  options: Array<SearchableSelectOption<TValue>>;
  value: TValue | null;
  onValueChange: (value: TValue | null) => void;
  ariaLabel: string;
  placeholder?: string;
  searchPlaceholder?: string;
  emptyMessage?: string;
  disabled?: boolean;
  loading?: boolean;
  allowClear?: boolean;
  searchable?: boolean;
  className?: string;
  name?: string;
};

export function filterSearchableOptions<TValue extends SearchableSelectValue>(
  options: Array<SearchableSelectOption<TValue>>,
  query: string,
) {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  if (!normalizedQuery) return options;

  return options.filter((option) =>
    [option.label, option.description, String(option.value), ...(option.keywords ?? [])]
      .filter(Boolean)
      .some((field) => field?.toLocaleLowerCase().includes(normalizedQuery)),
  );
}

export function SearchableSelect<TValue extends SearchableSelectValue>({
  options,
  value,
  onValueChange,
  ariaLabel,
  placeholder = "Select an option…",
  searchPlaceholder = "Search options…",
  emptyMessage = "No options found",
  disabled = false,
  loading = false,
  allowClear = false,
  searchable = true,
  className,
  name,
}: SearchableSelectProps<TValue>) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const listboxRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listboxId = useId();
  const selectedOption = options.find((option) => option.value === value);
  const filteredOptions = useMemo(
    () => filterSearchableOptions(options, query),
    [options, query],
  );

  useEffect(() => {
    function closeOnOutsidePointer(event: PointerEvent) {
      if (!rootRef.current?.contains(event.target as Node)) close();
    }

    document.addEventListener("pointerdown", closeOnOutsidePointer);
    return () => document.removeEventListener("pointerdown", closeOnOutsidePointer);
  }, []);

  useEffect(() => {
    if (!open) return;
    window.requestAnimationFrame(() => {
      if (searchable) searchInputRef.current?.focus();
      else listboxRef.current?.focus();
    });
  }, [open, searchable]);

  function openSelect() {
    setActiveIndex(Math.max(0, options.findIndex((option) => option.value === value)));
    setOpen(true);
  }

  function close({ restoreFocus = false } = {}) {
    setOpen(false);
    setQuery("");
    setActiveIndex(0);
    if (restoreFocus) window.requestAnimationFrame(() => triggerRef.current?.focus());
  }

  function select(nextValue: TValue | null) {
    onValueChange(nextValue);
    close({ restoreFocus: true });
  }

  function moveActive(direction: 1 | -1) {
    if (filteredOptions.length === 0) return;
    setActiveIndex((current) => (current + direction + filteredOptions.length) % filteredOptions.length);
  }

  function handleListKeyDown(event: ReactKeyboardEvent) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      moveActive(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveActive(-1);
    } else if (event.key === "Enter" && filteredOptions[activeIndex]) {
      event.preventDefault();
      select(filteredOptions[activeIndex].value);
    } else if (event.key === "Escape") {
      event.preventDefault();
      close({ restoreFocus: true });
    }
  }

  return (
    <div className={cn("relative", className)} ref={rootRef}>
      {name ? <input name={name} type="hidden" value={value ?? ""} /> : null}
      <button
        aria-controls={listboxId}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label={ariaLabel}
        className={cn(
          "flex h-9 w-full items-center justify-between gap-3 rounded-md border border-input bg-background px-3 text-left text-sm outline-none transition-colors",
          "hover:border-ring/70 focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/30",
          open && "border-ring ring-2 ring-ring/30",
          (disabled || loading) && "cursor-not-allowed opacity-50",
          allowClear && selectedOption && "pr-14",
        )}
        disabled={disabled || loading}
        onClick={() => {
          if (open) close();
          else openSelect();
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            if (!open) openSelect();
          } else if (event.key === "Escape" && open) {
            event.preventDefault();
            close({ restoreFocus: true });
          }
        }}
        ref={triggerRef}
        type="button"
      >
        <span className={cn("min-w-0 flex-1 truncate", !selectedOption && "text-muted-foreground")}>
          {loading ? "Loading…" : selectedOption?.label ?? placeholder}
        </span>
        <ChevronDown className={cn("size-4 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")} />
      </button>

      {allowClear && selectedOption && !disabled && !loading ? (
        <button
          aria-label={`Clear ${ariaLabel}`}
          className="absolute right-7 top-1.5 z-10 grid size-6 place-items-center rounded-sm text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
          onClick={() => select(null)}
          type="button"
        >
          <X className="size-3.5" />
        </button>
      ) : null}

      {open ? (
        <div className="absolute z-50 mt-1 w-full min-w-64 overflow-hidden rounded-md border border-border bg-popover shadow-2xl shadow-black/30">
          {searchable ? (
            <div className="border-b border-border p-2">
              <div className="relative">
                <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                <input
                  aria-activedescendant={filteredOptions[activeIndex] ? `${listboxId}-option-${activeIndex}` : undefined}
                  aria-controls={listboxId}
                  aria-expanded={open}
                  aria-label={`Search ${ariaLabel}`}
                  className="h-9 w-full rounded-md border border-input bg-background pl-8 pr-3 text-sm outline-none placeholder:text-muted-foreground focus:border-ring focus:ring-2 focus:ring-ring/30"
                  onChange={(event) => {
                    setQuery(event.target.value);
                    setActiveIndex(0);
                  }}
                  onKeyDown={handleListKeyDown}
                  placeholder={searchPlaceholder}
                  ref={searchInputRef}
                  role="combobox"
                  value={query}
                />
              </div>
            </div>
          ) : null}

          <div
            aria-label={ariaLabel}
            aria-activedescendant={filteredOptions[activeIndex] ? `${listboxId}-option-${activeIndex}` : undefined}
            className="max-h-64 overflow-y-auto p-1"
            id={listboxId}
            onKeyDown={searchable ? undefined : handleListKeyDown}
            ref={listboxRef}
            role="listbox"
            tabIndex={searchable ? -1 : 0}
          >
            {filteredOptions.length === 0 ? (
              <div className="px-3 py-6 text-center text-xs text-muted-foreground">{emptyMessage}</div>
            ) : filteredOptions.map((option, index) => {
              const selected = option.value === value;
              const active = index === activeIndex;
              return (
                <button
                  aria-selected={selected}
                  className={cn(
                    "flex w-full items-center gap-3 rounded-sm px-3 py-2 text-left transition-colors",
                    active && "bg-accent",
                    selected && "text-primary",
                  )}
                  id={`${listboxId}-option-${index}`}
                  key={option.value}
                  onClick={() => select(option.value)}
                  onMouseEnter={() => setActiveIndex(index)}
                  role="option"
                  tabIndex={-1}
                  type="button"
                >
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium text-foreground">{option.label}</span>
                    {option.description ? (
                      <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">{option.description}</span>
                    ) : null}
                  </span>
                  {selected ? <Check className="size-4 shrink-0" /> : null}
                </button>
              );
            })}
          </div>
        </div>
      ) : null}
    </div>
  );
}
