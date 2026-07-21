ALTER TABLE `runners` ADD `last_job_id` integer REFERENCES `workflow_jobs`(`id`) ON UPDATE no action ON DELETE set null;
--> statement-breakpoint
CREATE INDEX `runners_last_job_idx` ON `runners` (`last_job_id`);
