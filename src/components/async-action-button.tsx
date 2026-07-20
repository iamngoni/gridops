import { useRouter } from "@tanstack/react-router";
import { LoaderCircle } from "lucide-react";
import { useState, type ReactNode } from "react";
import { toast } from "sonner";

import { Button } from "./ui/button";

export function AsyncActionButton({
  children,
  icon,
  action,
  success,
  confirm,
  variant = "outline",
  size = "sm",
  disabled,
}: {
  children: ReactNode;
  icon?: ReactNode;
  action: () => Promise<unknown>;
  success: string;
  confirm?: string;
  variant?: "default" | "destructive" | "outline" | "secondary" | "ghost" | "link";
  size?: "default" | "sm" | "lg" | "icon";
  disabled?: boolean;
}) {
  const [pending, setPending] = useState(false);
  const router = useRouter();

  async function run() {
    if (confirm && !window.confirm(confirm)) return;
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

  return (
    <Button disabled={disabled || pending} onClick={run} size={size} variant={variant}>
      {pending ? <LoaderCircle className="animate-spin" /> : icon}
      {children}
    </Button>
  );
}
