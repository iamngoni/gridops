CREATE TABLE `github_runner_cleanup` (
	`id` text PRIMARY KEY NOT NULL,
	`installation_id` integer NOT NULL,
	`target_owner` text NOT NULL,
	`target_repository` text,
	`github_runner_id` integer,
	`runner_name` text NOT NULL,
	`attempts` integer DEFAULT 0 NOT NULL,
	`last_error` text,
	`next_attempt_at` integer NOT NULL,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	FOREIGN KEY (`installation_id`) REFERENCES `installations`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX `github_runner_cleanup_due_idx` ON `github_runner_cleanup` (`next_attempt_at`);
