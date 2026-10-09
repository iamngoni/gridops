-- A complete inventory is evidence for one authenticated authority session,
-- boot and epoch. A boolean left over from an older session cannot admit work.
ALTER TABLE fleet_hosts ADD COLUMN authority_incarnation TEXT;
ALTER TABLE fleet_hosts ADD COLUMN inventory_epoch INTEGER CHECK (inventory_epoch > 0);
ALTER TABLE fleet_hosts ADD COLUMN inventory_session_id TEXT;
ALTER TABLE fleet_hosts ADD COLUMN inventory_boot_id TEXT;

-- Earlier snapshots lack session association and cannot prove readiness.
UPDATE fleet_hosts SET inventory_complete=0,last_inventory_at=NULL;

CREATE TRIGGER fleet_hosts_inventory_insert_requires_current_authority
BEFORE INSERT ON fleet_hosts
WHEN NEW.inventory_complete=1 AND (
  NEW.last_inventory_at IS NULL OR NEW.inventory_epoch IS NOT NEW.epoch
  OR NEW.agent_session_id IS NULL OR NEW.inventory_session_id IS NOT NEW.agent_session_id
  OR NEW.boot_id IS NULL OR NEW.inventory_boot_id IS NOT NEW.boot_id
  OR NEW.authority_incarnation IS NULL
)
BEGIN SELECT RAISE(ABORT,'inventory must match current authority'); END;

CREATE TRIGGER fleet_hosts_invalidate_inventory_on_authority_change
AFTER UPDATE OF epoch,agent_session_id,boot_id,authority_incarnation ON fleet_hosts
WHEN NEW.epoch IS NOT OLD.epoch
  OR NEW.agent_session_id IS NOT OLD.agent_session_id
  OR NEW.boot_id IS NOT OLD.boot_id
  OR NEW.authority_incarnation IS NOT OLD.authority_incarnation
BEGIN
  UPDATE fleet_hosts SET inventory_complete=0,last_inventory_at=NULL,
    inventory_epoch=NULL,inventory_session_id=NULL,inventory_boot_id=NULL
  WHERE id=NEW.id;
END;

CREATE TRIGGER fleet_hosts_inventory_requires_current_authority
BEFORE UPDATE OF inventory_complete,last_inventory_at,inventory_epoch,inventory_session_id,inventory_boot_id ON fleet_hosts
WHEN NEW.inventory_complete=1 AND (
  NEW.last_inventory_at IS NULL OR NEW.inventory_epoch IS NOT NEW.epoch
  OR NEW.agent_session_id IS NULL OR NEW.inventory_session_id IS NOT NEW.agent_session_id
  OR NEW.boot_id IS NULL OR NEW.inventory_boot_id IS NOT NEW.boot_id
  OR NEW.authority_incarnation IS NULL
)
BEGIN SELECT RAISE(ABORT,'inventory must match current authority'); END;
