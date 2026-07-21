import { Check, ChevronDown, Search, X } from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState } from "react";

import {
  type SearchableSelectOption,
  type SearchableSelectValue,
  filterSearchableOptions,
} from "~/components/ui/searchable-select";
import { cn } from "~/lib/utils";

type SearchableMultiSelectProps<TValue extends SearchableSelectValue> = {
  options: Array<SearchableSelectOption<TValue>>;
  values: TValue[];
  onValueChange: (values: TValue[]) => void;
  ariaLabel: string;
  placeholder?: string;
  searchPlaceholder?: string;
  emptyMessage?: string;
  loading?: boolean;
  disabled?: boolean;
  maxSelected?: number;
};

export function SearchableMultiSelect<TValue extends SearchableSelectValue>({
  options,
  values,
  onValueChange,
  ariaLabel,
  placeholder = "Select options…",
  searchPlaceholder = "Search options…",
  emptyMessage = "No options found",
  loading = false,
  disabled = false,
  maxSelected,
}: SearchableMultiSelectProps<TValue>) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listboxId = useId();
  const filteredOptions = useMemo(
    () => filterSearchableOptions(options, query),
    [options, query],
  );
  const selectedOptions = options.filter((option) => values.includes(option.value));

  useEffect(() => {
    function closeOnOutsidePointer(event: PointerEvent) {
      if (!rootRef.current?.contains(event.target as Node)) close();
    }
    document.addEventListener("pointerdown", closeOnOutsidePointer);
    return () => document.removeEventListener("pointerdown", closeOnOutsidePointer);
  }, []);

  useEffect(() => {
    if (open) window.requestAnimationFrame(() => searchInputRef.current?.focus());
  }, [open]);

  function close({ restoreFocus = false } = {}) {
    setOpen(false);
    setQuery("");
    if (restoreFocus) window.requestAnimationFrame(() => triggerRef.current?.focus());
  }

  function toggle(value: TValue) {
    if (values.includes(value)) {
      onValueChange(values.filter((selected) => selected !== value));
      return;
    }
    if (maxSelected !== undefined && values.length >= maxSelected) return;
    onValueChange([...values, value]);
  }

  function moveActive(direction: 1 | -1) {
    if (filteredOptions.length === 0) return;
    setActiveIndex((current) => (current + direction + filteredOptions.length) % filteredOptions.length);
  }

  function toggleActive() {
    const option = filteredOptions[activeIndex];
    if (option) toggle(option.value);
  }

  const summary = selectedOptions.length === 0
    ? placeholder
    : selectedOptions.length === 1
      ? selectedOptions[0]?.label
      : `${selectedOptions.length} repositories selected`;

  return (
    <div className="relative" ref={rootRef}>
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
        )}
        disabled={disabled || loading}
        onClick={() => setOpen((current) => {
          const next = !current;
          if (next) setActiveIndex(0);
          return next;
        })}
        ref={triggerRef}
        type="button"
      >
        <span className={cn("min-w-0 flex-1 truncate", values.length === 0 && "text-muted-foreground")}>
          {loading ? "Loading repositories…" : summary}
        </span>
        <ChevronDown className={cn("size-4 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")} />
      </button>

      {selectedOptions.length > 0 ? (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {selectedOptions.map((option) => (
            <button
              aria-label={`Remove ${option.label}`}
              className="inline-flex max-w-full items-center gap-1 rounded-md border border-border bg-muted/40 px-2 py-1 text-[11px] text-foreground hover:border-ring/60"
              disabled={disabled}
              key={option.value}
              onClick={() => toggle(option.value)}
              type="button"
            >
              <span className="max-w-56 truncate">{option.label}</span>
              <X className="size-3 text-muted-foreground" />
            </button>
          ))}
        </div>
      ) : null}

      {open ? (
        <div className="absolute z-50 mt-1 w-full min-w-72 overflow-hidden rounded-md border border-border bg-popover shadow-2xl shadow-black/30">
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
                onKeyDown={(event) => {
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    moveActive(1);
                  } else if (event.key === "ArrowUp") {
                    event.preventDefault();
                    moveActive(-1);
                  } else if (event.key === "Home") {
                    event.preventDefault();
                    setActiveIndex(0);
                  } else if (event.key === "End") {
                    event.preventDefault();
                    setActiveIndex(Math.max(0, filteredOptions.length - 1));
                  } else if (event.key === "Enter") {
                    event.preventDefault();
                    toggleActive();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    close({ restoreFocus: true });
                  }
                }}
                placeholder={searchPlaceholder}
                ref={searchInputRef}
                role="combobox"
                value={query}
              />
            </div>
            {maxSelected !== undefined ? (
              <div className="mt-2 text-[11px] text-muted-foreground">
                {values.length} of {maxSelected} repository slots used
              </div>
            ) : null}
          </div>
          <div aria-label={ariaLabel} className="max-h-72 overflow-y-auto p-1" id={listboxId} role="listbox" aria-multiselectable="true">
            {filteredOptions.length === 0 ? (
              <div className="px-3 py-6 text-center text-xs text-muted-foreground">{emptyMessage}</div>
            ) : filteredOptions.map((option, index) => {
              const selected = values.includes(option.value);
              const atLimit = !selected && maxSelected !== undefined && values.length >= maxSelected;
              return (
                <button
                  aria-selected={selected}
                  className={cn(
                    "flex w-full items-center gap-3 rounded-sm px-3 py-2 text-left transition-colors hover:bg-accent",
                    index === activeIndex && "bg-accent",
                    selected && "text-primary",
                    atLimit && "cursor-not-allowed opacity-40",
                  )}
                  disabled={atLimit}
                  id={`${listboxId}-option-${index}`}
                  key={option.value}
                  onClick={() => toggle(option.value)}
                  onMouseEnter={() => setActiveIndex(index)}
                  role="option"
                  tabIndex={-1}
                  type="button"
                >
                  <span className={cn(
                    "grid size-4 shrink-0 place-items-center rounded border",
                    selected ? "border-primary bg-primary text-primary-foreground" : "border-input",
                  )}>
                    {selected ? <Check className="size-3" /> : null}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium text-foreground">{option.label}</span>
                    {option.description ? <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">{option.description}</span> : null}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      ) : null}
    </div>
  );
}
