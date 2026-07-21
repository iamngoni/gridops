DROP INDEX IF EXISTS `runners_github_id_unique`;
--> statement-breakpoint
CREATE INDEX `runners_github_id_idx` ON `runners` (`github_runner_id`);
