CREATE TABLE `audit_events` (
	`id` text PRIMARY KEY NOT NULL,
	`actor_user_id` text,
	`actor_label` text NOT NULL,
	`action` text NOT NULL,
	`target_type` text NOT NULL,
	`target_id` text,
	`metadata` text DEFAULT '{}' NOT NULL,
	`ip_address` text,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`actor_user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE set null
);
--> statement-breakpoint
CREATE INDEX `audit_events_created_idx` ON `audit_events` (`created_at`);--> statement-breakpoint
CREATE INDEX `audit_events_target_idx` ON `audit_events` (`target_type`,`target_id`);--> statement-breakpoint
CREATE TABLE `installations` (
	`id` integer PRIMARY KEY NOT NULL,
	`account_id` integer NOT NULL,
	`account_login` text NOT NULL,
	`account_type` text NOT NULL,
	`account_avatar_url` text,
	`target_type` text NOT NULL,
	`repository_selection` text NOT NULL,
	`permissions` text DEFAULT '{}' NOT NULL,
	`events` text DEFAULT '[]' NOT NULL,
	`suspended_at` integer,
	`last_synced_at` integer,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `installations_account_unique` ON `installations` (`account_id`,`target_type`);--> statement-breakpoint
CREATE INDEX `installations_account_login_idx` ON `installations` (`account_login`);--> statement-breakpoint
CREATE TABLE `log_streams` (
	`id` text PRIMARY KEY NOT NULL,
	`job_id` integer,
	`runner_id` text,
	`source` text NOT NULL,
	`path` text NOT NULL,
	`size_bytes` integer DEFAULT 0 NOT NULL,
	`complete` integer DEFAULT false NOT NULL,
	`checksum` text,
	`expires_at` integer,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	FOREIGN KEY (`job_id`) REFERENCES `workflow_jobs`(`id`) ON UPDATE no action ON DELETE cascade,
	FOREIGN KEY (`runner_id`) REFERENCES `runners`(`id`) ON UPDATE no action ON DELETE set null
);
--> statement-breakpoint
CREATE INDEX `log_streams_job_idx` ON `log_streams` (`job_id`);--> statement-breakpoint
CREATE INDEX `log_streams_expiry_idx` ON `log_streams` (`expires_at`);--> statement-breakpoint
CREATE TABLE `oauth_states` (
	`id` text PRIMARY KEY NOT NULL,
	`state_hash` text NOT NULL,
	`code_verifier` text NOT NULL,
	`return_to` text DEFAULT '/' NOT NULL,
	`expires_at` integer NOT NULL,
	`created_at` integer NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `oauth_states_state_hash_unique` ON `oauth_states` (`state_hash`);--> statement-breakpoint
CREATE INDEX `oauth_states_expires_at_idx` ON `oauth_states` (`expires_at`);--> statement-breakpoint
CREATE TABLE `repositories` (
	`id` integer PRIMARY KEY NOT NULL,
	`installation_id` integer NOT NULL,
	`owner` text NOT NULL,
	`name` text NOT NULL,
	`full_name` text NOT NULL,
	`private` integer NOT NULL,
	`archived` integer DEFAULT false NOT NULL,
	`default_branch` text NOT NULL,
	`html_url` text NOT NULL,
	`permission` text,
	`github_updated_at` integer,
	`last_synced_at` integer NOT NULL,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	FOREIGN KEY (`installation_id`) REFERENCES `installations`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE UNIQUE INDEX `repositories_full_name_unique` ON `repositories` (`full_name`);--> statement-breakpoint
CREATE INDEX `repositories_installation_idx` ON `repositories` (`installation_id`);--> statement-breakpoint
CREATE TABLE `runner_events` (
	`id` text PRIMARY KEY NOT NULL,
	`runner_id` text,
	`pool_id` text,
	`level` text DEFAULT 'info' NOT NULL,
	`event` text NOT NULL,
	`message` text NOT NULL,
	`metadata` text DEFAULT '{}' NOT NULL,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`runner_id`) REFERENCES `runners`(`id`) ON UPDATE no action ON DELETE cascade,
	FOREIGN KEY (`pool_id`) REFERENCES `runner_pools`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX `runner_events_created_idx` ON `runner_events` (`created_at`);--> statement-breakpoint
CREATE TABLE `runner_pools` (
	`id` text PRIMARY KEY NOT NULL,
	`installation_id` integer NOT NULL,
	`repository_id` integer,
	`name` text NOT NULL,
	`scope` text NOT NULL,
	`mode` text DEFAULT 'ephemeral' NOT NULL,
	`labels` text DEFAULT '[]' NOT NULL,
	`image` text NOT NULL,
	`desired_count` integer DEFAULT 0 NOT NULL,
	`min_count` integer DEFAULT 0 NOT NULL,
	`max_count` integer DEFAULT 10 NOT NULL,
	`cpu_limit` integer DEFAULT 2 NOT NULL,
	`memory_limit_mb` integer DEFAULT 4096 NOT NULL,
	`ephemeral` integer DEFAULT true NOT NULL,
	`paused` integer DEFAULT false NOT NULL,
	`state` text DEFAULT 'active' NOT NULL,
	`created_by` text,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	FOREIGN KEY (`installation_id`) REFERENCES `installations`(`id`) ON UPDATE no action ON DELETE cascade,
	FOREIGN KEY (`repository_id`) REFERENCES `repositories`(`id`) ON UPDATE no action ON DELETE cascade,
	FOREIGN KEY (`created_by`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE set null
);
--> statement-breakpoint
CREATE UNIQUE INDEX `runner_pools_installation_name_unique` ON `runner_pools` (`installation_id`,`name`);--> statement-breakpoint
CREATE INDEX `runner_pools_repository_idx` ON `runner_pools` (`repository_id`);--> statement-breakpoint
CREATE INDEX `runner_pools_state_idx` ON `runner_pools` (`state`);--> statement-breakpoint
CREATE TABLE `runners` (
	`id` text PRIMARY KEY NOT NULL,
	`pool_id` text NOT NULL,
	`github_runner_id` integer,
	`container_id` text,
	`container_name` text,
	`name` text NOT NULL,
	`os` text DEFAULT 'linux' NOT NULL,
	`architecture` text DEFAULT 'x64' NOT NULL,
	`status` text DEFAULT 'starting' NOT NULL,
	`busy` integer DEFAULT false NOT NULL,
	`ephemeral` integer DEFAULT true NOT NULL,
	`current_job_id` integer,
	`failure_reason` text,
	`last_heartbeat_at` integer,
	`registered_at` integer,
	`deleted_at` integer,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	FOREIGN KEY (`pool_id`) REFERENCES `runner_pools`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE UNIQUE INDEX `runners_github_id_unique` ON `runners` (`github_runner_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `runners_container_id_unique` ON `runners` (`container_id`);--> statement-breakpoint
CREATE INDEX `runners_pool_status_idx` ON `runners` (`pool_id`,`status`);--> statement-breakpoint
CREATE TABLE `sessions` (
	`id` text PRIMARY KEY NOT NULL,
	`token_hash` text NOT NULL,
	`user_id` text NOT NULL,
	`user_agent` text,
	`ip_address` text,
	`expires_at` integer NOT NULL,
	`last_seen_at` integer NOT NULL,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE UNIQUE INDEX `sessions_token_hash_unique` ON `sessions` (`token_hash`);--> statement-breakpoint
CREATE INDEX `sessions_user_id_idx` ON `sessions` (`user_id`);--> statement-breakpoint
CREATE INDEX `sessions_expires_at_idx` ON `sessions` (`expires_at`);--> statement-breakpoint
CREATE TABLE `settings` (
	`key` text PRIMARY KEY NOT NULL,
	`value` text NOT NULL,
	`updated_by` text,
	`updated_at` integer NOT NULL,
	FOREIGN KEY (`updated_by`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE set null
);
--> statement-breakpoint
CREATE TABLE `user_installations` (
	`user_id` text NOT NULL,
	`installation_id` integer NOT NULL,
	`permission` text DEFAULT 'read' NOT NULL,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade,
	FOREIGN KEY (`installation_id`) REFERENCES `installations`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE UNIQUE INDEX `user_installations_unique` ON `user_installations` (`user_id`,`installation_id`);--> statement-breakpoint
CREATE INDEX `user_installations_installation_idx` ON `user_installations` (`installation_id`);--> statement-breakpoint
CREATE TABLE `users` (
	`id` text PRIMARY KEY NOT NULL,
	`github_id` integer NOT NULL,
	`login` text NOT NULL,
	`name` text,
	`email` text,
	`avatar_url` text,
	`access_token` text NOT NULL,
	`access_token_expires_at` integer,
	`refresh_token` text,
	`refresh_token_expires_at` integer,
	`last_login_at` integer NOT NULL,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `users_github_id_unique` ON `users` (`github_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `users_login_unique` ON `users` (`login`);--> statement-breakpoint
CREATE TABLE `webhook_deliveries` (
	`id` text PRIMARY KEY NOT NULL,
	`event` text NOT NULL,
	`action` text,
	`hook_id` integer,
	`installation_id` integer,
	`repository_id` integer,
	`signature_valid` integer NOT NULL,
	`status` text DEFAULT 'received' NOT NULL,
	`payload` text,
	`error` text,
	`received_at` integer NOT NULL,
	`processed_at` integer
);
--> statement-breakpoint
CREATE INDEX `webhook_deliveries_event_idx` ON `webhook_deliveries` (`event`,`received_at`);--> statement-breakpoint
CREATE INDEX `webhook_deliveries_status_idx` ON `webhook_deliveries` (`status`);--> statement-breakpoint
CREATE TABLE `workflow_jobs` (
	`id` integer PRIMARY KEY NOT NULL,
	`run_id` integer NOT NULL,
	`name` text NOT NULL,
	`status` text NOT NULL,
	`conclusion` text,
	`runner_id` integer,
	`runner_name` text,
	`runner_group_id` integer,
	`runner_group_name` text,
	`labels` text DEFAULT '[]' NOT NULL,
	`html_url` text NOT NULL,
	`started_at` integer,
	`completed_at` integer,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	FOREIGN KEY (`run_id`) REFERENCES `workflow_runs`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX `workflow_jobs_run_status_idx` ON `workflow_jobs` (`run_id`,`status`);--> statement-breakpoint
CREATE INDEX `workflow_jobs_runner_idx` ON `workflow_jobs` (`runner_id`);--> statement-breakpoint
CREATE TABLE `workflow_runs` (
	`id` integer PRIMARY KEY NOT NULL,
	`repository_id` integer NOT NULL,
	`workflow_id` integer,
	`workflow_name` text NOT NULL,
	`run_number` integer NOT NULL,
	`run_attempt` integer DEFAULT 1 NOT NULL,
	`event` text NOT NULL,
	`status` text NOT NULL,
	`conclusion` text,
	`head_branch` text,
	`head_sha` text NOT NULL,
	`actor_login` text,
	`html_url` text NOT NULL,
	`started_at` integer,
	`completed_at` integer,
	`github_created_at` integer NOT NULL,
	`github_updated_at` integer NOT NULL,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	FOREIGN KEY (`repository_id`) REFERENCES `repositories`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX `workflow_runs_repository_status_idx` ON `workflow_runs` (`repository_id`,`status`);--> statement-breakpoint
CREATE INDEX `workflow_runs_created_idx` ON `workflow_runs` (`github_created_at`);
