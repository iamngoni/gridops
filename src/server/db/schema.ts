import { sql } from "drizzle-orm";
import {
  index,
  integer,
  sqliteTable,
  text,
  uniqueIndex,
} from "drizzle-orm/sqlite-core";

const timestamps = {
  createdAt: integer("created_at", { mode: "timestamp_ms" })
    .notNull()
    .$defaultFn(() => new Date()),
  updatedAt: integer("updated_at", { mode: "timestamp_ms" })
    .notNull()
    .$defaultFn(() => new Date())
    .$onUpdate(() => new Date()),
};

export const users = sqliteTable(
  "users",
  {
    id: text("id").primaryKey(),
    githubId: integer("github_id").notNull(),
    login: text("login").notNull(),
    name: text("name"),
    email: text("email"),
    avatarUrl: text("avatar_url"),
    accessToken: text("access_token").notNull(),
    accessTokenExpiresAt: integer("access_token_expires_at", {
      mode: "timestamp_ms",
    }),
    refreshToken: text("refresh_token"),
    refreshTokenExpiresAt: integer("refresh_token_expires_at", {
      mode: "timestamp_ms",
    }),
    lastLoginAt: integer("last_login_at", { mode: "timestamp_ms" }).notNull(),
    ...timestamps,
  },
  (table) => [
    uniqueIndex("users_github_id_unique").on(table.githubId),
    uniqueIndex("users_login_unique").on(table.login),
  ],
);

export const sessions = sqliteTable(
  "sessions",
  {
    id: text("id").primaryKey(),
    tokenHash: text("token_hash").notNull(),
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    userAgent: text("user_agent"),
    ipAddress: text("ip_address"),
    expiresAt: integer("expires_at", { mode: "timestamp_ms" }).notNull(),
    lastSeenAt: integer("last_seen_at", { mode: "timestamp_ms" }).notNull(),
    createdAt: integer("created_at", { mode: "timestamp_ms" })
      .notNull()
      .$defaultFn(() => new Date()),
  },
  (table) => [
    uniqueIndex("sessions_token_hash_unique").on(table.tokenHash),
    index("sessions_user_id_idx").on(table.userId),
    index("sessions_expires_at_idx").on(table.expiresAt),
  ],
);

export const oauthStates = sqliteTable(
  "oauth_states",
  {
    id: text("id").primaryKey(),
    stateHash: text("state_hash").notNull(),
    codeVerifier: text("code_verifier").notNull(),
    returnTo: text("return_to").notNull().default("/"),
    expiresAt: integer("expires_at", { mode: "timestamp_ms" }).notNull(),
    createdAt: integer("created_at", { mode: "timestamp_ms" })
      .notNull()
      .$defaultFn(() => new Date()),
  },
  (table) => [
    uniqueIndex("oauth_states_state_hash_unique").on(table.stateHash),
    index("oauth_states_expires_at_idx").on(table.expiresAt),
  ],
);

export const installations = sqliteTable(
  "installations",
  {
    id: integer("id").primaryKey(),
    accountId: integer("account_id").notNull(),
    accountLogin: text("account_login").notNull(),
    accountType: text("account_type").notNull(),
    accountAvatarUrl: text("account_avatar_url"),
    targetType: text("target_type").notNull(),
    repositorySelection: text("repository_selection").notNull(),
    permissions: text("permissions", { mode: "json" })
      .$type<Record<string, string>>()
      .notNull()
      .default(sql`'{}'`),
    events: text("events", { mode: "json" })
      .$type<string[]>()
      .notNull()
      .default(sql`'[]'`),
    suspendedAt: integer("suspended_at", { mode: "timestamp_ms" }),
    lastSyncedAt: integer("last_synced_at", { mode: "timestamp_ms" }),
    ...timestamps,
  },
  (table) => [
    uniqueIndex("installations_account_unique").on(
      table.accountId,
      table.targetType,
    ),
    index("installations_account_login_idx").on(table.accountLogin),
  ],
);

export const userInstallations = sqliteTable(
  "user_installations",
  {
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    installationId: integer("installation_id")
      .notNull()
      .references(() => installations.id, { onDelete: "cascade" }),
    permission: text("permission").notNull().default("read"),
    createdAt: integer("created_at", { mode: "timestamp_ms" })
      .notNull()
      .$defaultFn(() => new Date()),
  },
  (table) => [
    uniqueIndex("user_installations_unique").on(
      table.userId,
      table.installationId,
    ),
    index("user_installations_installation_idx").on(table.installationId),
  ],
);

export const repositories = sqliteTable(
  "repositories",
  {
    id: integer("id").primaryKey(),
    installationId: integer("installation_id")
      .notNull()
      .references(() => installations.id, { onDelete: "cascade" }),
    owner: text("owner").notNull(),
    name: text("name").notNull(),
    fullName: text("full_name").notNull(),
    private: integer("private", { mode: "boolean" }).notNull(),
    archived: integer("archived", { mode: "boolean" }).notNull().default(false),
    defaultBranch: text("default_branch").notNull(),
    htmlUrl: text("html_url").notNull(),
    permission: text("permission"),
    githubUpdatedAt: integer("github_updated_at", { mode: "timestamp_ms" }),
    lastSyncedAt: integer("last_synced_at", { mode: "timestamp_ms" }).notNull(),
    ...timestamps,
  },
  (table) => [
    uniqueIndex("repositories_full_name_unique").on(table.fullName),
    index("repositories_installation_idx").on(table.installationId),
  ],
);

export const runnerPools = sqliteTable(
  "runner_pools",
  {
    id: text("id").primaryKey(),
    installationId: integer("installation_id")
      .notNull()
      .references(() => installations.id, { onDelete: "cascade" }),
    repositoryId: integer("repository_id").references(() => repositories.id, {
      onDelete: "cascade",
    }),
    name: text("name").notNull(),
    scope: text("scope").notNull(),
    mode: text("mode").notNull().default("ephemeral"),
    labels: text("labels", { mode: "json" })
      .$type<string[]>()
      .notNull()
      .default(sql`'[]'`),
    image: text("image").notNull(),
    desiredCount: integer("desired_count").notNull().default(0),
    minCount: integer("min_count").notNull().default(0),
    maxCount: integer("max_count").notNull().default(10),
    autoscalingEnabled: integer("autoscaling_enabled", { mode: "boolean" })
      .notNull()
      .default(true),
    queueScaleFactor: integer("queue_scale_factor").notNull().default(1),
    idleTimeoutMinutes: integer("idle_timeout_minutes").notNull().default(5),
    cpuLimit: integer("cpu_limit").notNull().default(2),
    memoryLimitMb: integer("memory_limit_mb").notNull().default(4096),
    runnerGroupId: integer("runner_group_id").notNull().default(1),
    ephemeral: integer("ephemeral", { mode: "boolean" }).notNull().default(true),
    paused: integer("paused", { mode: "boolean" }).notNull().default(false),
    state: text("state").notNull().default("active"),
    createdBy: text("created_by").references(() => users.id, {
      onDelete: "set null",
    }),
    ...timestamps,
  },
  (table) => [
    uniqueIndex("runner_pools_installation_name_unique").on(
      table.installationId,
      table.name,
    ),
    index("runner_pools_repository_idx").on(table.repositoryId),
    index("runner_pools_state_idx").on(table.state),
  ],
);

export const runners = sqliteTable(
  "runners",
  {
    id: text("id").primaryKey(),
    poolId: text("pool_id")
      .notNull()
      .references(() => runnerPools.id, { onDelete: "cascade" }),
    githubRunnerId: integer("github_runner_id"),
    containerId: text("container_id"),
    containerName: text("container_name"),
    name: text("name").notNull(),
    os: text("os").notNull().default("linux"),
    architecture: text("architecture").notNull().default("x64"),
    status: text("status").notNull().default("starting"),
    busy: integer("busy", { mode: "boolean" }).notNull().default(false),
    ephemeral: integer("ephemeral", { mode: "boolean" }).notNull().default(true),
    currentJobId: integer("current_job_id"),
    failureReason: text("failure_reason"),
    lastHeartbeatAt: integer("last_heartbeat_at", { mode: "timestamp_ms" }),
    registeredAt: integer("registered_at", { mode: "timestamp_ms" }),
    deletedAt: integer("deleted_at", { mode: "timestamp_ms" }),
    ...timestamps,
  },
  (table) => [
    uniqueIndex("runners_github_id_unique").on(table.githubRunnerId),
    uniqueIndex("runners_container_id_unique").on(table.containerId),
    index("runners_pool_status_idx").on(table.poolId, table.status),
  ],
);

export const workflowRuns = sqliteTable(
  "workflow_runs",
  {
    id: integer("id").primaryKey(),
    repositoryId: integer("repository_id")
      .notNull()
      .references(() => repositories.id, { onDelete: "cascade" }),
    workflowId: integer("workflow_id"),
    workflowName: text("workflow_name").notNull(),
    runNumber: integer("run_number").notNull(),
    runAttempt: integer("run_attempt").notNull().default(1),
    event: text("event").notNull(),
    status: text("status").notNull(),
    conclusion: text("conclusion"),
    headBranch: text("head_branch"),
    headSha: text("head_sha").notNull(),
    actorLogin: text("actor_login"),
    htmlUrl: text("html_url").notNull(),
    startedAt: integer("started_at", { mode: "timestamp_ms" }),
    completedAt: integer("completed_at", { mode: "timestamp_ms" }),
    githubCreatedAt: integer("github_created_at", { mode: "timestamp_ms" }).notNull(),
    githubUpdatedAt: integer("github_updated_at", { mode: "timestamp_ms" }).notNull(),
    ...timestamps,
  },
  (table) => [
    index("workflow_runs_repository_status_idx").on(
      table.repositoryId,
      table.status,
    ),
    index("workflow_runs_created_idx").on(table.githubCreatedAt),
  ],
);

export const workflowJobs = sqliteTable(
  "workflow_jobs",
  {
    id: integer("id").primaryKey(),
    runId: integer("run_id")
      .notNull()
      .references(() => workflowRuns.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    status: text("status").notNull(),
    conclusion: text("conclusion"),
    runnerId: integer("runner_id"),
    runnerName: text("runner_name"),
    runnerGroupId: integer("runner_group_id"),
    runnerGroupName: text("runner_group_name"),
    labels: text("labels", { mode: "json" })
      .$type<string[]>()
      .notNull()
      .default(sql`'[]'`),
    htmlUrl: text("html_url").notNull(),
    startedAt: integer("started_at", { mode: "timestamp_ms" }),
    completedAt: integer("completed_at", { mode: "timestamp_ms" }),
    ...timestamps,
  },
  (table) => [
    index("workflow_jobs_run_status_idx").on(table.runId, table.status),
    index("workflow_jobs_runner_idx").on(table.runnerId),
  ],
);

export const webhookDeliveries = sqliteTable(
  "webhook_deliveries",
  {
    id: text("id").primaryKey(),
    event: text("event").notNull(),
    action: text("action"),
    hookId: integer("hook_id"),
    installationId: integer("installation_id"),
    repositoryId: integer("repository_id"),
    signatureValid: integer("signature_valid", { mode: "boolean" }).notNull(),
    status: text("status").notNull().default("received"),
    payload: text("payload", { mode: "json" }).$type<Record<string, unknown>>(),
    error: text("error"),
    receivedAt: integer("received_at", { mode: "timestamp_ms" }).notNull(),
    processedAt: integer("processed_at", { mode: "timestamp_ms" }),
  },
  (table) => [
    index("webhook_deliveries_event_idx").on(table.event, table.receivedAt),
    index("webhook_deliveries_status_idx").on(table.status),
  ],
);

export const auditEvents = sqliteTable(
  "audit_events",
  {
    id: text("id").primaryKey(),
    actorUserId: text("actor_user_id").references(() => users.id, {
      onDelete: "set null",
    }),
    actorLabel: text("actor_label").notNull(),
    action: text("action").notNull(),
    targetType: text("target_type").notNull(),
    targetId: text("target_id"),
    metadata: text("metadata", { mode: "json" })
      .$type<Record<string, unknown>>()
      .notNull()
      .default(sql`'{}'`),
    ipAddress: text("ip_address"),
    createdAt: integer("created_at", { mode: "timestamp_ms" })
      .notNull()
      .$defaultFn(() => new Date()),
  },
  (table) => [
    index("audit_events_created_idx").on(table.createdAt),
    index("audit_events_target_idx").on(table.targetType, table.targetId),
  ],
);

export const runnerEvents = sqliteTable(
  "runner_events",
  {
    id: text("id").primaryKey(),
    runnerId: text("runner_id").references(() => runners.id, {
      onDelete: "cascade",
    }),
    poolId: text("pool_id").references(() => runnerPools.id, {
      onDelete: "cascade",
    }),
    level: text("level").notNull().default("info"),
    event: text("event").notNull(),
    message: text("message").notNull(),
    metadata: text("metadata", { mode: "json" })
      .$type<Record<string, unknown>>()
      .notNull()
      .default(sql`'{}'`),
    createdAt: integer("created_at", { mode: "timestamp_ms" })
      .notNull()
      .$defaultFn(() => new Date()),
  },
  (table) => [index("runner_events_created_idx").on(table.createdAt)],
);

export const logStreams = sqliteTable(
  "log_streams",
  {
    id: text("id").primaryKey(),
    jobId: integer("job_id").references(() => workflowJobs.id, {
      onDelete: "cascade",
    }),
    runnerId: text("runner_id").references(() => runners.id, {
      onDelete: "set null",
    }),
    source: text("source").notNull(),
    path: text("path").notNull(),
    sizeBytes: integer("size_bytes").notNull().default(0),
    complete: integer("complete", { mode: "boolean" }).notNull().default(false),
    checksum: text("checksum"),
    expiresAt: integer("expires_at", { mode: "timestamp_ms" }),
    ...timestamps,
  },
  (table) => [
    index("log_streams_job_idx").on(table.jobId),
    index("log_streams_expiry_idx").on(table.expiresAt),
  ],
);

export const settings = sqliteTable("settings", {
  key: text("key").primaryKey(),
  value: text("value", { mode: "json" }).$type<unknown>().notNull(),
  updatedBy: text("updated_by").references(() => users.id, {
    onDelete: "set null",
  }),
  updatedAt: integer("updated_at", { mode: "timestamp_ms" })
    .notNull()
    .$defaultFn(() => new Date())
    .$onUpdate(() => new Date()),
});
