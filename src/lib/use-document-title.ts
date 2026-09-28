import { useEffect } from "react";

/** Names the browser tab after the current view, so tabs and history entries are tellable apart. */
export function useDocumentTitle(title: string | undefined) {
  useEffect(() => {
    if (!title) return undefined;
    const previous = document.title;
    document.title = `${title} · GridOps`;
    return () => {
      document.title = previous;
    };
  }, [title]);
}
