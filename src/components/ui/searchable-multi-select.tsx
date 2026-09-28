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
  selectedNoun?: string;
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
  selectedNoun = "options",
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
  const selectedOptions = values
    .map((value) => options.find((option) => option.value === value))
    .filter((option): option is SearchableSelectOption<TValue> => option !== undefined);

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
      : `${selectedOptions.length} ${selectedNoun} selected`;

  return (
    <div className="relative" ref={rootRef}>
      <button
        aria-controls={listboxId}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label={ariaLabel}
        className={cn(
          "flex h-8 w-full items-center justify-between gap-2 rounded-md border border-border-strong bg-panel px-2.5 text-left text-sm text-foreground shadow-[0_1px_1px_rgb(0_0_0/0.03)] outline-none transition-colors",
          "hover:bg-hover focus-visible:border-primary/70 focus-visible:ring-2 focus-visible:ring-primary/20",
          open && "border-primary/70 ring-2 ring-primary/20",
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
          {loading ? `Loading ${selectedNoun}…` : summary}
        </span>
        <ChevronDown className={cn("size-4 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")} />
      </button>

      {selectedOptions.length > 0 ? (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {selectedOptions.map((option) => (
            <button
              aria-label={`Remove ${option.label}`}
              className="inline-flex h-6 max-w-full items-center gap-1 rounded-full border border-border-strong bg-panel-subtle px-2 text-2xs font-medium text-secondary-foreground hover:border-faint hover:text-foreground"
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
        <div className="absolute z-50 mt-1 w-full min-w-72 overflow-hidden rounded-lg border border-border-strong bg-popover text-left shadow-popover">
          <div className="border-b border-border p-1">
            <div className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
              <input
                aria-activedescendant={filteredOptions[activeIndex] ? `${listboxId}-option-${activeIndex}` : undefined}
                aria-controls={listboxId}
                aria-expanded={open}
                aria-label={`Search ${ariaLabel}`}
                className="h-8 w-full rounded-md bg-transparent pl-8 pr-3 text-sm outline-none placeholder:text-faint"
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
              <div className="mt-2 text-2xs text-muted-foreground">
                {values.length} of {maxSelected} {selectedNoun} selected
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
                    "flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-hover",
                    index === activeIndex && "bg-hover",
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
                    selected ? "border-primary bg-primary text-primary-foreground" : "border-border-strong",
                  )}>
                    {selected ? <Check className="size-3" /> : null}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm text-foreground">{option.label}</span>
                    {option.description ? <span className="mt-0.5 block truncate text-2xs text-muted-foreground">{option.description}</span> : null}
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
