CREATE TABLE `runner_pool_installations` (
	`pool_id` text NOT NULL,
	`installation_id` integer NOT NULL,
	PRIMARY KEY (`pool_id`,`installation_id`),
	FOREIGN KEY (`pool_id`) REFERENCES `runner_pools`(`id`) ON UPDATE no action ON DELETE cascade,
	FOREIGN KEY (`installation_id`) REFERENCES `installations`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX `runner_pool_installations_installation_idx` ON `runner_pool_installations` (`installation_id`,`pool_id`);
--> statement-breakpoint
INSERT INTO `runner_pool_installations` (`pool_id`,`installation_id`)
SELECT `id`,`installation_id` FROM `runner_pools`;
--> statement-breakpoint
INSERT OR IGNORE INTO `runner_pool_installations` (`pool_id`,`installation_id`)
SELECT membership.`pool_id`,repository.`installation_id`
FROM `runner_pool_repositories` membership
JOIN `repositories` repository ON repository.`id`=membership.`repository_id`;
--> statement-breakpoint
CREATE TRIGGER `runner_pool_installation_after_pool_insert`
AFTER INSERT ON `runner_pools`
BEGIN
	INSERT OR IGNORE INTO `runner_pool_installations` (`pool_id`,`installation_id`)
	VALUES (NEW.`id`,NEW.`installation_id`);
END;
--> statement-breakpoint
CREATE TRIGGER `runner_pool_installation_after_pool_update`
AFTER UPDATE OF `installation_id` ON `runner_pools`
BEGIN
	INSERT OR IGNORE INTO `runner_pool_installations` (`pool_id`,`installation_id`)
	VALUES (NEW.`id`,NEW.`installation_id`);
	DELETE FROM `runner_pool_installations`
	WHERE `pool_id`=NEW.`id` AND `installation_id`=OLD.`installation_id`
	  AND OLD.`installation_id`<>NEW.`installation_id`
	  AND NOT EXISTS (
		SELECT 1 FROM `runner_pool_repositories` membership
		JOIN `repositories` repository ON repository.`id`=membership.`repository_id`
		WHERE membership.`pool_id`=NEW.`id`
		  AND repository.`installation_id`=OLD.`installation_id`
	  );
END;
--> statement-breakpoint
CREATE TRIGGER `runner_pool_installation_after_repository_insert`
AFTER INSERT ON `runner_pool_repositories`
BEGIN
	INSERT OR IGNORE INTO `runner_pool_installations` (`pool_id`,`installation_id`)
	SELECT NEW.`pool_id`,`installation_id` FROM `repositories` WHERE `id`=NEW.`repository_id`;
END;
--> statement-breakpoint
CREATE TRIGGER `runner_pool_installation_before_repository_delete`
BEFORE DELETE ON `runner_pool_repositories`
BEGIN
	DELETE FROM `runner_pool_installations`
	WHERE `pool_id`=OLD.`pool_id`
	  AND `installation_id`=(SELECT `installation_id` FROM `repositories` WHERE `id`=OLD.`repository_id`)
	  AND `installation_id`<>(SELECT `installation_id` FROM `runner_pools` WHERE `id`=OLD.`pool_id`)
	  AND NOT EXISTS (
		SELECT 1 FROM `runner_pool_repositories` membership
		JOIN `repositories` repository ON repository.`id`=membership.`repository_id`
		WHERE membership.`pool_id`=OLD.`pool_id`
		  AND membership.`repository_id`<>OLD.`repository_id`
		  AND repository.`installation_id`=(SELECT `installation_id` FROM `repositories` WHERE `id`=OLD.`repository_id`)
	  );
END;
