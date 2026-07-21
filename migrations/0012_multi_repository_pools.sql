CREATE TABLE `runner_pool_repositories` (
	`pool_id` text NOT NULL,
	`repository_id` integer NOT NULL,
	`created_at` integer NOT NULL,
	PRIMARY KEY (`pool_id`,`repository_id`),
	FOREIGN KEY (`pool_id`) REFERENCES `runner_pools`(`id`) ON UPDATE no action ON DELETE cascade,
	FOREIGN KEY (`repository_id`) REFERENCES `repositories`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX `runner_pool_repositories_repository_idx` ON `runner_pool_repositories` (`repository_id`,`pool_id`);
--> statement-breakpoint
INSERT INTO `runner_pool_repositories` (`pool_id`,`repository_id`,`created_at`)
SELECT `id`,`repository_id`,`created_at` FROM `runner_pools` WHERE `repository_id` IS NOT NULL;
--> statement-breakpoint
ALTER TABLE `runners` ADD `target_repository_id` integer REFERENCES `repositories`(`id`) ON UPDATE no action ON DELETE set null;
--> statement-breakpoint
UPDATE `runners` SET `target_repository_id`=(
	SELECT `repository_id` FROM `runner_pools` WHERE `runner_pools`.`id`=`runners`.`pool_id`
) WHERE `target_repository_id` IS NULL;
--> statement-breakpoint
CREATE INDEX `runners_target_repository_idx` ON `runners` (`pool_id`,`target_repository_id`,`status`);
