import { useEffect, useState } from "react";

import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";

export type ConfirmOptions = {
  /** The action's button; defaults to the verb that opens the question ("Delete", "Stop"). */
  confirmLabel?: string;
  dismissLabel?: string;
  destructive?: boolean;
};

type Request = ConfirmOptions & { message: string; resolve: (confirmed: boolean) => void };

let showRequest: ((request: Request) => void) | null = null;

const DESTRUCTIVE_VERBS = new Set(["Cancel", "Delete", "Force-cancel", "Remove", "Stop"]);
const VERB = /\b(Cancel|Delete|Force-cancel|Grant|Pause|Rebuild|Remove|Restart|Stop)\b/;

/**
 * Asks for confirmation in GridOps' own dialog. A message reads "Question?
 * Details.": the question becomes the title and the rest its description.
 */
export function confirmAction(message: string, options: ConfirmOptions = {}) {
  return new Promise<boolean>((resolve) => {
    if (!showRequest) {
      resolve(window.confirm(message));
      return;
    }
    // Open on the next task: prompts usually come from a menu item, and the
    // menu must finish closing (and returning focus) before a modal takes over.
    const show = showRequest;
    window.setTimeout(() => show({ ...options, message, resolve }), 0);
  });
}

export function ConfirmHost() {
  const [request, setRequest] = useState<Request | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    showRequest = (next) => {
      setRequest(next);
      setOpen(true);
    };
    return () => {
      showRequest = null;
    };
  }, []);

  function settle(confirmed: boolean) {
    request?.resolve(confirmed);
    setOpen(false);
  }

  const split = request?.message.indexOf("?") ?? -1;
  const title = request && split >= 0 ? request.message.slice(0, split + 1) : request?.message;
  const description = request && split >= 0 ? request.message.slice(split + 1).trim() : "";
  const verb = request?.message.match(VERB)?.[1];
  const destructive = request?.destructive ?? (verb ? DESTRUCTIVE_VERBS.has(verb) : false);
  const confirmLabel = request?.confirmLabel ?? (verb === "Force-cancel" ? "Force cancel" : verb ?? "Continue");
  const dismissLabel = request?.dismissLabel ?? (verb === "Cancel" || verb === "Force-cancel" ? "Keep running" : "Cancel");

  return (
    <Dialog onOpenChange={(next) => { if (!next) settle(false); }} open={open}>
      <DialogContent className="top-[22vh] max-w-[420px]">
        <form
          className="p-5"
          onSubmit={(event) => {
            event.preventDefault();
            settle(true);
          }}
        >
          <DialogTitle className="text-base font-medium leading-6 text-foreground">{title}</DialogTitle>
          {description ? <DialogDescription className="mt-1.5 text-sm leading-5 text-muted-foreground">{description}</DialogDescription> : <DialogDescription className="sr-only">Confirm or dismiss this action.</DialogDescription>}
          <div className="mt-6 flex justify-end gap-2">
            {/* Enter answers with the focused button: the safe choice for destructive actions, the action otherwise. */}
            <Button autoFocus={destructive} onClick={() => settle(false)} variant="ghost">{dismissLabel}</Button>
            <Button autoFocus={!destructive} type="submit" variant={destructive ? "destructive" : "default"}>{confirmLabel}</Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}
