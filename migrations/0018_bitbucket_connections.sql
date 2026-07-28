CREATE TABLE `bitbucket_connections` (
  `id` text PRIMARY KEY NOT NULL,
  `name` text NOT NULL,
  `workspace` text NOT NULL,
  `workspace_uuid` text NOT NULL,
  `access_token_key` text NOT NULL,
  `created_by` text,
  `created_at` integer NOT NULL,
  `updated_at` integer NOT NULL,
  FOREIGN KEY (`created_by`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE set null
);
--> statement-breakpoint
CREATE UNIQUE INDEX `bitbucket_connections_workspace_unique` ON `bitbucket_connections` (`workspace`);
--> statement-breakpoint
CREATE UNIQUE INDEX `bitbucket_connections_access_token_key_unique` ON `bitbucket_connections` (`access_token_key`);
