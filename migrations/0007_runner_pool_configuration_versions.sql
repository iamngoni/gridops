ALTER TABLE `runner_pools` ADD `configuration_version` integer DEFAULT 1 NOT NULL;
--> statement-breakpoint
ALTER TABLE `runners` ADD `configuration_version` integer DEFAULT 1 NOT NULL;
--> statement-breakpoint
CREATE INDEX `runners_pool_configuration_idx` ON `runners` (`pool_id`,`configuration_version`);
