import { api } from "~/lib/api";

export type BitbucketConnection = {
  id: string;
  provider: "bitbucket";
  name: string;
  workspace: string;
  workspaceUuid: string;
  createdAt: string;
  updatedAt: string;
  canManage: boolean;
};

export const getBitbucketConnections = () =>
  api<{ canManage: boolean; items: BitbucketConnection[] }>("/api/v1/platform-connections/bitbucket");

export const createBitbucketConnection = (data: { name: string; workspace: string; accessToken: string }) =>
  api<Pick<BitbucketConnection, "id" | "provider" | "name" | "workspace" | "workspaceUuid">>(
    "/api/v1/platform-connections/bitbucket",
    { method: "POST", body: data },
  );
