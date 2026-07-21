CREATE TABLE `capacity_samples` (
	`id` text PRIMARY KEY NOT NULL,
	`installation_id` integer NOT NULL,
	`pool_id` text NOT NULL,
	`available` integer DEFAULT 0 NOT NULL,
	`busy` integer DEFAULT 0 NOT NULL,
	`queued` integer DEFAULT 0 NOT NULL,
	`recorded_at` integer NOT NULL,
	FOREIGN KEY (`installation_id`) REFERENCES `installations`(`id`) ON UPDATE no action ON DELETE cascade,
	FOREIGN KEY (`pool_id`) REFERENCES `runner_pools`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE UNIQUE INDEX `capacity_samples_pool_time_unique` ON `capacity_samples` (`pool_id`,`recorded_at`);
--> statement-breakpoint
CREATE INDEX `capacity_samples_installation_time_idx` ON `capacity_samples` (`installation_id`,`recorded_at`);
