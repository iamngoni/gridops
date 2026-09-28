import {
  Activity,
  Boxes,
  CircleGauge,
  FileClock,
  GitPullRequestArrow,
  PackageSearch,
  Radio,
  Settings,
  Webhook,
  type LucideIcon,
} from "lucide-react";

export type NavItem = {
  label: string;
  to: string;
  icon: LucideIcon;
  /** Second key of the Linear-style "G then …" jump shortcut. */
  shortcut: string;
  alert?: "queuedJobs" | "failedRunners" | "failedWebhooks";
};

export type NavGroup = { label: string | null; items: NavItem[] };

export const navigation: NavGroup[] = [
  {
    label: null,
    items: [
      { label: "Overview", to: "/", icon: CircleGauge, shortcut: "o" },
      { label: "Live logs", to: "/live-logs", icon: Radio, shortcut: "l" },
    ],
  },
  {
    label: "Operate",
    items: [
      { label: "Runner pools", to: "/runner-pools", icon: Boxes, shortcut: "p" },
      { label: "Runners", to: "/runners", icon: Activity, shortcut: "r", alert: "failedRunners" },
      { label: "Workflow runs", to: "/workflow-runs", icon: GitPullRequestArrow, shortcut: "w", alert: "queuedJobs" },
      { label: "Repositories", to: "/repositories", icon: PackageSearch, shortcut: "e" },
    ],
  },
  {
    label: "Observe",
    items: [
      { label: "Webhooks", to: "/webhooks", icon: Webhook, shortcut: "h" },
      { label: "Audit log", to: "/audit-log", icon: FileClock, shortcut: "a" },
    ],
  },
];

export const settingsNavItem: NavItem = { label: "Settings", to: "/settings", icon: Settings, shortcut: "s" };

export const allNavItems = [...navigation.flatMap((group) => group.items), settingsNavItem];

export function isActivePath(pathname: string, to: string) {
  return to === "/" ? pathname === "/" : pathname === to || pathname.startsWith(`${to}/`);
}
