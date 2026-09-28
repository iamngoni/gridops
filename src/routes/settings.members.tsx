import { createFileRoute, getRouteApi } from "@tanstack/react-router";
import { UsersRound } from "lucide-react";

import { EmptyState } from "~/components/page";
import { SettingsLayout, SettingsSection } from "~/components/settings-ui";
import { Avatar } from "~/components/ui/avatar";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { updateUserRoleAction } from "~/features/operations/operations.functions";
import { useAction } from "~/lib/use-action";
import { formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/settings/members")({
  component: MembersSettings,
});

const settingsRoute = getRouteApi("/settings");

function MembersSettings() {
  const page = settingsRoute.useLoaderData();
  const run = useAction();
  if (!page.data) return <EmptyState description="Sign in to view members." icon={UsersRound} title="Members unavailable" />;
  const { users, user } = page.data;
  const isAdmin = user.role === "admin";

  return (
    <SettingsLayout
      description="Administrators manage credentials, backups, policy, and every installation they can access. Members keep read-only visibility unless they administer a GitHub installation."
      title="Members"
    >
      {isAdmin ? (
        <SettingsSection title={`${users.length} ${users.length === 1 ? "member" : "members"}`}>
          {users.map((member) => {
            const nextRole = member.role === "admin" ? "member" : "admin";
            return (
              <div className="flex items-center gap-3 px-4 py-3" key={member.id}>
                <Avatar name={member.login} size={32} src={member.avatarUrl} />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-sm font-medium">{member.name ?? member.login}</span>
                    {member.id === user.id ? <span className="text-xs text-faint">You</span> : null}
                  </div>
                  <div className="truncate text-xs text-muted-foreground">@{member.login} · last signed in {formatRelativeTime(member.lastLoginAt)}</div>
                </div>
                <Badge dot={member.role === "admin" ? "bg-primary" : false} variant="secondary">{member.role === "admin" ? "Admin" : "Member"}</Badge>
                <Button
                  disabled={nextRole === "member" && !member.canDemote}
                  onClick={() => void run({
                    action: () => updateUserRoleAction({ data: { userId: member.id, role: nextRole } }),
                    confirm: nextRole === "member" ? `Remove administrator access from @${member.login}?` : `Grant @${member.login} administrator access?`,
                    success: `@${member.login} is now ${nextRole === "admin" ? "an admin" : "a member"}.`,
                  })}
                  size="sm"
                  variant="outline"
                >
                  {nextRole === "admin" ? "Make admin" : "Make member"}
                </Button>
              </div>
            );
          })}
        </SettingsSection>
      ) : (
        <SettingsSection>
          <div className="flex items-center gap-3 px-4 py-3">
            <Avatar name={user.login} size={32} />
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium">@{user.login}</div>
              <div className="text-xs text-muted-foreground">You have member access. An administrator can grant admin rights.</div>
            </div>
            <Badge variant="secondary">Member</Badge>
          </div>
        </SettingsSection>
      )}
    </SettingsLayout>
  );
}
