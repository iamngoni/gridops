import { useRouter } from "@tanstack/react-router";
import { LoaderCircle } from "lucide-react";
import { useState, type ReactNode } from "react";
import { toast } from "sonner";

import { confirmAction } from "./confirm-dialog";
import { Button, type ButtonSize, type ButtonVariant } from "./ui/button";
import { Tooltip } from "./ui/tooltip";

export function AsyncActionButton({
  children,
  icon,
  action,
  success,
  confirm,
  variant = "outline",
  size = "sm",
  disabled,
  title,
  className,
}: {
  children?: ReactNode;
  icon?: ReactNode;
  action: () => Promise<unknown>;
  success: string;
  confirm?: string;
  variant?: ButtonVariant;
  size?: ButtonSize;
  disabled?: boolean;
  title?: string;
  className?: string;
}) {
  const [pending, setPending] = useState(false);
  const router = useRouter();

  async function run() {
    if (confirm && !(await confirmAction(confirm))) return;
    setPending(true);
    try {
      await action();
      toast.success(success);
      await router.invalidate();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Action failed.");
    } finally {
      setPending(false);
    }
  }

  const button = (
    <Button aria-label={title} className={className} disabled={disabled || pending} onClick={run} size={size} variant={variant}>
      {pending ? <LoaderCircle className="animate-spin" /> : icon}
      {children}
    </Button>
  );
  return title ? <Tooltip content={title}>{button}</Tooltip> : button;
}
