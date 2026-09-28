import { useEffect, useRef } from "react";

const SEQUENCE_TIMEOUT_MS = 1_200;

export function isTypingTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  return ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) || Boolean(target.closest("[role='combobox'],[cmdk-root]"));
}

/**
 * Linear's keyboard model: ⌘K opens the command menu, "G then <key>" jumps to a
 * view, and single keys trigger primary actions when focus is not in a field.
 */
export function useGlobalHotkeys({
  onCommandMenu,
  onGo,
  onKey,
}: {
  onCommandMenu: () => void;
  onGo: (key: string) => boolean;
  onKey?: (key: string) => boolean;
}) {
  const handlers = useRef({ onCommandMenu, onGo, onKey });
  useEffect(() => {
    handlers.current = { onCommandMenu, onGo, onKey };
  });

  useEffect(() => {
    let pendingGo = 0;
    function handle(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        handlers.current.onCommandMenu();
        return;
      }
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey || isTypingTarget(event.target)) return;
      if (document.querySelector("[role='dialog'][data-state='open'],[role='menu'][data-state='open']")) return;
      const key = event.key.toLowerCase();
      if (pendingGo && Date.now() - pendingGo < SEQUENCE_TIMEOUT_MS) {
        pendingGo = 0;
        if (handlers.current.onGo(key)) event.preventDefault();
        return;
      }
      if (key === "g") {
        pendingGo = Date.now();
        return;
      }
      if (handlers.current.onKey?.(event.key)) event.preventDefault();
    }
    window.addEventListener("keydown", handle);
    return () => window.removeEventListener("keydown", handle);
  }, []);
}
