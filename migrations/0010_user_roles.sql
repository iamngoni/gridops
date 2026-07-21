ALTER TABLE `users` ADD `role` text DEFAULT 'member' NOT NULL;
--> statement-breakpoint
UPDATE `users` SET `role`='admin'
WHERE `id`=(SELECT `id` FROM `users` ORDER BY `created_at`,`id` LIMIT 1);
--> statement-breakpoint
CREATE INDEX `users_role_idx` ON `users` (`role`);
