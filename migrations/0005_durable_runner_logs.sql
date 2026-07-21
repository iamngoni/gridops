ALTER TABLE `log_streams` ADD `installation_id` integer REFERENCES `installations`(`id`) ON UPDATE no action ON DELETE set null;
--> statement-breakpoint
ALTER TABLE `log_streams` ADD `runner_name` text;
--> statement-breakpoint
ALTER TABLE `log_streams` ADD `pool_name` text;
--> statement-breakpoint
ALTER TABLE `log_streams` ADD `repository` text;
--> statement-breakpoint
CREATE INDEX `log_streams_installation_idx` ON `log_streams` (`installation_id`,`created_at`);
