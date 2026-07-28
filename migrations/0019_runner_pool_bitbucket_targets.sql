CREATE TABLE `runner_pool_bitbucket_connections` (
  `pool_id` text NOT NULL,
  `connection_id` text NOT NULL,
  `created_at` integer NOT NULL,
  PRIMARY KEY (`pool_id`,`connection_id`),
  FOREIGN KEY (`pool_id`) REFERENCES `runner_pools`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`connection_id`) REFERENCES `bitbucket_connections`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX `runner_pool_bitbucket_connections_connection_idx`
  ON `runner_pool_bitbucket_connections` (`connection_id`,`pool_id`);
--> statement-breakpoint
ALTER TABLE `runners` ADD COLUMN `ci_platform` text NOT NULL DEFAULT 'github'
  CHECK (`ci_platform` IN ('github','bitbucket'));
--> statement-breakpoint
ALTER TABLE `runners` ADD COLUMN `bitbucket_connection_id` text
  REFERENCES `bitbucket_connections`(`id`) ON UPDATE no action ON DELETE set null;
--> statement-breakpoint
ALTER TABLE `runners` ADD COLUMN `bitbucket_runner_uuid` text;
--> statement-breakpoint
CREATE INDEX `runners_bitbucket_connection_idx`
  ON `runners` (`pool_id`,`bitbucket_connection_id`,`status`);
