ALTER TABLE `runner_pools` ADD `autoscaling_enabled` integer DEFAULT true NOT NULL;--> statement-breakpoint
ALTER TABLE `runner_pools` ADD `queue_scale_factor` integer DEFAULT 1 NOT NULL;--> statement-breakpoint
ALTER TABLE `runner_pools` ADD `idle_timeout_minutes` integer DEFAULT 5 NOT NULL;
