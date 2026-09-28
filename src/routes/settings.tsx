import { Link, Outlet, createFileRoute, useRouterState } from "@tanstack/react-router";
import { ChevronLeft, CloudCog, Github, Palette, Server, Settings, Settings2, UsersRound, type LucideIcon } from "lucide-react";

import { AppShell } from "~/components/app-shell";
import { LoadingRows, PageBody, PageHeader } from "~/components/page";
import { getSettingsPage } from "~/features/operations/operations.functions";
import { cn } from "~/lib/utils";

export const Route = createFileRoute("/settings")({
  loader: () => getSettingsPage(),
  pendingComponent: () => (
    <AppShell sidebar={<SettingsSidebar />}>
      <PageHeader title="Settings" />
      <PageBody><LoadingRows rows={6} /></PageBody>
    </AppShell>
  ),
  component: SettingsLayout,
});

type SettingsNavItem = { label: string; to: string; icon: LucideIcon };

const sections: Array<{ label: string; items: SettingsNavItem[] }> = [
  { label: "Account", items: [{ label: "Preferences", to: "/settings/preferences", icon: Palette }] },
  {
    label: "Workspace",
    items: [
      { label: "General", to: "/settings/general", icon: Settings2 },
      { label: "Members", to: "/settings/members", icon: UsersRound },
      { label: "Runner host", to: "/settings/runner-host", icon: Server },
    ],
  },
  {
    label: "Integrations",
    items: [
      { label: "GitHub", to: "/settings/github", icon: Github },
      { label: "Bitbucket", to: "/settings/integrations", icon: CloudCog },
    ],
  },
];

function SettingsLayout() {
  return (
    <AppShell sidebar={<SettingsSidebar />}>
      {/* Settings pages carry their own headings; on narrow screens this bar holds the menu button. */}
      <PageHeader className="lg:hidden" documentTitle={false} icon={Settings} title="Settings" />
      <PageBody>
        <Outlet />
      </PageBody>
    </AppShell>
  );
}

function SettingsSidebar() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  return (
    <aside className="flex h-full w-[244px] shrink-0 flex-col gap-4 px-2 py-2">
      <Link className="flex h-9 items-center gap-1.5 rounded-md px-2 text-sm font-medium text-secondary-foreground hover:bg-hover hover:text-foreground" to="/">
        <ChevronLeft className="size-4 text-muted-foreground" />
        Back to GridOps
      </Link>
      <nav aria-label="Settings" className="space-y-4">
        {sections.map((section) => (
          <div key={section.label}>
            <div className="mb-0.5 flex h-7 items-center px-2 text-xs font-medium text-faint">{section.label}</div>
            <div className="space-y-px">
              {section.items.map((item) => {
                const active = pathname === item.to;
                const Icon = item.icon;
                return (
                  <Link
                    aria-current={active ? "page" : undefined}
                    className={cn(
                      "flex h-7 items-center gap-2 rounded-md px-2 text-sm font-medium transition-colors",
                      active ? "bg-selected text-foreground" : "text-secondary-foreground/85 hover:bg-hover hover:text-foreground",
                    )}
                    key={item.to}
                    to={item.to}
                  >
                    <Icon className={cn("size-4", active ? "text-foreground" : "text-muted-foreground")} />
                    {item.label}
                  </Link>
                );
              })}
            </div>
          </div>
        ))}
      </nav>
    </aside>
  );
}
