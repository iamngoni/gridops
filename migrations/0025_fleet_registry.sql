-- Registry/enrollment durability. Secrets remain verifier-only; this migration
-- adds recovery and rotation state without rewriting foundation ownership data.

ALTER TABLE fleet_hosts ADD COLUMN inventory_revision INTEGER NOT NULL DEFAULT 0
  CHECK (inventory_revision >= 0);
ALTER TABLE fleet_hosts ADD COLUMN inventory_digest TEXT;
ALTER TABLE fleet_hosts ADD COLUMN current_credential_generation INTEGER
  CHECK (current_credential_generation IS NULL OR current_credential_generation > 0);
-- The consumed enrollment that established the host's current epoch is the
-- causal lineage.  Timestamps and UUID ordering are not a substitute for it.
ALTER TABLE fleet_hosts ADD COLUMN current_enrollment_id TEXT;
ALTER TABLE fleet_hosts ADD COLUMN current_enrollment_epoch INTEGER
  CHECK (current_enrollment_epoch IS NULL OR current_enrollment_epoch > 0);

ALTER TABLE host_enrollments ADD COLUMN mode TEXT NOT NULL DEFAULT 'normal'
  CHECK (mode IN ('normal','recovery'));
ALTER TABLE host_enrollments ADD COLUMN expected_host_epoch INTEGER
  CHECK (expected_host_epoch IS NULL OR expected_host_epoch > 0);
ALTER TABLE host_enrollments ADD COLUMN expected_host_revision INTEGER
  CHECK (expected_host_revision IS NULL OR expected_host_revision > 0);
ALTER TABLE host_enrollments ADD COLUMN verifier_format TEXT NOT NULL DEFAULT 'sha256'
  CHECK (verifier_format = 'sha256');
ALTER TABLE host_enrollments ADD COLUMN idempotency_key TEXT;
ALTER TABLE host_enrollments ADD COLUMN request_hash TEXT;
CREATE UNIQUE INDEX host_enrollments_issue_idempotency
  ON host_enrollments(issued_by,idempotency_key)
  WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX host_enrollments_consumed_recovery_epoch
  ON host_enrollments(consumed_host_id,expected_host_epoch)
  WHERE mode='recovery' AND consumed_at IS NOT NULL;

ALTER TABLE host_credentials ADD COLUMN verifier_format TEXT NOT NULL DEFAULT 'sha256'
  CHECK (verifier_format = 'sha256');

-- Rotation rows carry the complete ownership proof for their old generation.
-- The composite key is deliberately redundant with the scalar credential FK:
-- SQLite otherwise permits a host to name another host's credential.
CREATE UNIQUE INDEX host_credentials_rotation_owner
  ON host_credentials(host_id,id,generation,host_epoch);

CREATE TABLE host_credential_rotations (
  id TEXT PRIMARY KEY NOT NULL,
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  old_credential_id TEXT NOT NULL REFERENCES host_credentials(id),
  old_generation INTEGER NOT NULL CHECK (old_generation > 0),
  old_host_epoch INTEGER NOT NULL CHECK (old_host_epoch > 0),
  next_generation INTEGER NOT NULL CHECK (next_generation > old_generation),
  idempotency_key TEXT NOT NULL,
  requested_by TEXT NOT NULL REFERENCES users(id),
  request_method TEXT NOT NULL CHECK (length(request_method) BETWEEN 1 AND 128),
  request_resource TEXT NOT NULL CHECK (length(request_resource) BETWEEN 1 AND 256),
  request_hash TEXT NOT NULL CHECK (length(request_hash) = 64
    AND request_hash NOT GLOB '*[^0-9a-f]*'),
  phase TEXT NOT NULL DEFAULT 'requested'
    CHECK (phase IN ('requested','exchanged','acknowledged','revoked')),
  proposed_verifier TEXT,
  requested_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL CHECK (expires_at > requested_at),
  exchanged_at INTEGER,
  overlap_expires_at INTEGER,
  acknowledged_at INTEGER,
  revoked_at INTEGER,
  FOREIGN KEY (host_id,old_credential_id,old_generation,old_host_epoch)
    REFERENCES host_credentials(host_id,id,generation,host_epoch),
  CHECK (proposed_verifier IS NULL OR
    (length(proposed_verifier) = 43 AND proposed_verifier NOT GLOB '*[^A-Za-z0-9_-]*')),
  CHECK (overlap_expires_at IS NULL OR
    (exchanged_at IS NOT NULL AND overlap_expires_at > exchanged_at)),
  CHECK (
    (phase = 'requested' AND proposed_verifier IS NULL AND exchanged_at IS NULL
      AND overlap_expires_at IS NULL AND acknowledged_at IS NULL AND revoked_at IS NULL)
    OR (phase = 'exchanged' AND proposed_verifier IS NOT NULL AND exchanged_at IS NOT NULL
      AND overlap_expires_at IS NOT NULL AND acknowledged_at IS NULL AND revoked_at IS NULL)
    OR (phase = 'acknowledged' AND proposed_verifier IS NOT NULL AND exchanged_at IS NOT NULL
      AND overlap_expires_at IS NOT NULL AND acknowledged_at IS NOT NULL AND revoked_at IS NULL)
    OR (phase = 'revoked' AND revoked_at IS NOT NULL AND acknowledged_at IS NULL
      AND ((proposed_verifier IS NULL AND exchanged_at IS NULL AND overlap_expires_at IS NULL)
        OR (proposed_verifier IS NOT NULL AND exchanged_at IS NOT NULL
          AND overlap_expires_at IS NOT NULL)))
  ),
  UNIQUE (host_id,idempotency_key)
) STRICT;
CREATE UNIQUE INDEX host_credential_rotations_active_host
  ON host_credential_rotations(host_id)
  WHERE revoked_at IS NULL AND acknowledged_at IS NULL
    AND phase IN ('requested','exchanged');
CREATE INDEX host_credential_rotations_active_idx
  ON host_credential_rotations(host_id,phase,expires_at);

CREATE TRIGGER host_enrollments_verifier_format_insert
BEFORE INSERT ON host_enrollments
WHEN NEW.verifier_format != 'sha256'
  OR length(NEW.code_verifier) != 43
  OR NEW.code_verifier GLOB '*[^A-Za-z0-9_-]*'
BEGIN SELECT RAISE(ABORT,'enrollment verifier must be sha256 base64url'); END;

CREATE TRIGGER host_enrollments_verifier_format_update
BEFORE UPDATE OF code_verifier,verifier_format ON host_enrollments
WHEN NEW.verifier_format != 'sha256'
  OR length(NEW.code_verifier) != 43
  OR NEW.code_verifier GLOB '*[^A-Za-z0-9_-]*'
BEGIN SELECT RAISE(ABORT,'enrollment verifier must be sha256 base64url'); END;

CREATE TRIGGER host_credentials_verifier_format_insert
BEFORE INSERT ON host_credentials
WHEN NEW.verifier_format != 'sha256'
  OR length(NEW.verifier) != 43
  OR NEW.verifier GLOB '*[^A-Za-z0-9_-]*'
BEGIN SELECT RAISE(ABORT,'credential verifier must be sha256 base64url'); END;

CREATE TRIGGER host_credentials_verifier_format_update
BEFORE UPDATE OF verifier,verifier_format ON host_credentials
WHEN NEW.verifier_format != 'sha256'
  OR length(NEW.verifier) != 43
  OR NEW.verifier GLOB '*[^A-Za-z0-9_-]*'
BEGIN SELECT RAISE(ABORT,'credential verifier must be sha256 base64url'); END;

-- Credential identity is immutable.  Rotation may only update overlap,
-- acknowledgement, and revocation state; changing ownership or verifier
-- material would resurrect a credential in a later host epoch.
CREATE TRIGGER host_credentials_identity_immutable
BEFORE UPDATE OF id,host_id,host_epoch,generation,verifier,verifier_format,created_at
ON host_credentials
WHEN NEW.id IS NOT OLD.id
  OR NEW.host_id IS NOT OLD.host_id
  OR NEW.host_epoch IS NOT OLD.host_epoch
  OR NEW.generation IS NOT OLD.generation
  OR NEW.verifier IS NOT OLD.verifier
  OR NEW.verifier_format IS NOT OLD.verifier_format
  OR NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT,'host credential identity is immutable'); END;

-- Issuance is a receipt: its scope, issuer, request binding, expiry, and
-- verifier identity never change. Consumption and revocation remain mutable
-- lifecycle fields handled by the service.
CREATE TRIGGER host_enrollments_issued_identity_immutable
BEFORE UPDATE OF id,code_verifier,intended_host_id,scope_json,issued_by,created_at,expires_at,
  mode,expected_host_epoch,expected_host_revision,verifier_format,idempotency_key,request_hash
ON host_enrollments
WHEN NEW.id IS NOT OLD.id
  OR NEW.code_verifier IS NOT OLD.code_verifier
  OR NEW.intended_host_id IS NOT OLD.intended_host_id
  OR NEW.scope_json IS NOT OLD.scope_json
  OR NEW.issued_by IS NOT OLD.issued_by
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.expires_at IS NOT OLD.expires_at
  OR NEW.mode IS NOT OLD.mode
  OR NEW.expected_host_epoch IS NOT OLD.expected_host_epoch
  OR NEW.expected_host_revision IS NOT OLD.expected_host_revision
  OR NEW.verifier_format IS NOT OLD.verifier_format
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.request_hash IS NOT OLD.request_hash
BEGIN SELECT RAISE(ABORT,'enrollment issuance identity is immutable'); END;

CREATE TRIGGER host_credential_rotation_request_identity_immutable
BEFORE UPDATE OF id,host_id,old_credential_id,old_generation,old_host_epoch,
  next_generation,idempotency_key,requested_by,request_method,request_resource,request_hash,
  requested_at,expires_at
ON host_credential_rotations
WHEN NEW.id IS NOT OLD.id
  OR NEW.host_id IS NOT OLD.host_id
  OR NEW.old_credential_id IS NOT OLD.old_credential_id
  OR NEW.old_generation IS NOT OLD.old_generation
  OR NEW.old_host_epoch IS NOT OLD.old_host_epoch
  OR NEW.next_generation IS NOT OLD.next_generation
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.requested_by IS NOT OLD.requested_by
  OR NEW.request_method IS NOT OLD.request_method
  OR NEW.request_resource IS NOT OLD.request_resource
  OR NEW.request_hash IS NOT OLD.request_hash
  OR NEW.requested_at IS NOT OLD.requested_at
  OR NEW.expires_at IS NOT OLD.expires_at
BEGIN SELECT RAISE(ABORT,'rotation request identity is immutable'); END;

-- Normal enrollments are for new hosts; recovery enrollments must bind an
-- exact existing host and both positive optimistic-concurrency proofs.
CREATE TRIGGER host_enrollments_mode_consistency_insert
BEFORE INSERT ON host_enrollments
WHEN (NEW.mode = 'normal' AND (
        NEW.intended_host_id IS NOT NULL
        OR NEW.expected_host_epoch IS NOT NULL
        OR NEW.expected_host_revision IS NOT NULL
      ))
   OR (NEW.mode = 'recovery' AND (
        NEW.intended_host_id IS NULL
        OR NEW.expected_host_epoch IS NULL
        OR NEW.expected_host_revision IS NULL
      ))
BEGIN SELECT RAISE(ABORT,'enrollment mode and host proof are inconsistent'); END;

CREATE TRIGGER host_enrollments_mode_consistency_update
BEFORE UPDATE OF mode,intended_host_id,expected_host_epoch,expected_host_revision
ON host_enrollments
WHEN (NEW.mode = 'normal' AND (
        NEW.intended_host_id IS NOT NULL
        OR NEW.expected_host_epoch IS NOT NULL
        OR NEW.expected_host_revision IS NOT NULL
      ))
   OR (NEW.mode = 'recovery' AND (
        NEW.intended_host_id IS NULL
        OR NEW.expected_host_epoch IS NULL
        OR NEW.expected_host_revision IS NULL
      ))
BEGIN SELECT RAISE(ABORT,'enrollment mode and host proof are inconsistent'); END;

CREATE TRIGGER host_enrollments_recovery_lineage_insert
BEFORE INSERT ON host_enrollments
WHEN NEW.mode = 'recovery' AND NEW.consumed_at IS NOT NULL
  AND NEW.consumed_host_id IS NOT NEW.intended_host_id
BEGIN SELECT RAISE(ABORT,'recovery enrollment consumed host mismatch'); END;

CREATE TRIGGER host_enrollments_recovery_lineage_update
BEFORE UPDATE OF consumed_at,consumed_host_id,intended_host_id,mode
ON host_enrollments
WHEN NEW.mode = 'recovery' AND NEW.consumed_at IS NOT NULL
  AND NEW.consumed_host_id IS NOT NEW.intended_host_id
BEGIN SELECT RAISE(ABORT,'recovery enrollment consumed host mismatch'); END;

-- Once consumed, enrollment identity and epoch proof are immutable.  The
-- service may fill consumption fields once, but cannot rewrite the lineage
-- after it has become the host's current epoch proof.
CREATE TRIGGER host_enrollments_consumed_identity_immutable
BEFORE UPDATE OF id,mode,intended_host_id,expected_host_epoch,expected_host_revision,
  consumed_at,consumed_host_id,scope_json,issued_by,idempotency_key,request_hash,
  code_verifier,verifier_format,created_at,expires_at
ON host_enrollments
WHEN OLD.consumed_at IS NOT NULL AND (
  NEW.id IS NOT OLD.id
  OR NEW.mode IS NOT OLD.mode
  OR NEW.intended_host_id IS NOT OLD.intended_host_id
  OR NEW.expected_host_epoch IS NOT OLD.expected_host_epoch
  OR NEW.expected_host_revision IS NOT OLD.expected_host_revision
  OR NEW.consumed_at IS NOT OLD.consumed_at
  OR NEW.consumed_host_id IS NOT OLD.consumed_host_id
  OR NEW.scope_json IS NOT OLD.scope_json
  OR NEW.issued_by IS NOT OLD.issued_by
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.request_hash IS NOT OLD.request_hash
  OR NEW.code_verifier IS NOT OLD.code_verifier
  OR NEW.verifier_format IS NOT OLD.verifier_format
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.expires_at IS NOT OLD.expires_at
)
BEGIN SELECT RAISE(ABORT,'consumed enrollment lineage is immutable'); END;

CREATE TRIGGER fleet_hosts_current_enrollment_consistency
BEFORE UPDATE OF current_enrollment_id,current_enrollment_epoch,epoch
ON fleet_hosts
WHEN NEW.current_enrollment_id IS NOT NULL AND NOT EXISTS (
  SELECT 1 FROM host_enrollments e
   WHERE e.id=NEW.current_enrollment_id
     AND e.consumed_at IS NOT NULL
     AND e.consumed_host_id=NEW.id
     AND NEW.current_enrollment_epoch=NEW.epoch
     AND ((e.mode='normal' AND NEW.epoch=1 AND e.expected_host_epoch IS NULL)
       OR (e.mode='recovery' AND e.expected_host_epoch=NEW.epoch-1))
)
BEGIN SELECT RAISE(ABORT,'current enrollment lineage is inconsistent'); END;

CREATE TRIGGER fleet_hosts_current_enrollment_consistency_insert
BEFORE INSERT ON fleet_hosts
WHEN NEW.current_enrollment_id IS NOT NULL AND NOT EXISTS (
  SELECT 1 FROM host_enrollments e
   WHERE e.id=NEW.current_enrollment_id
     AND e.consumed_at IS NOT NULL
     AND e.consumed_host_id=NEW.id
     AND NEW.current_enrollment_epoch=NEW.epoch
     AND ((e.mode='normal' AND NEW.epoch=1 AND e.expected_host_epoch IS NULL)
       OR (e.mode='recovery' AND e.expected_host_epoch=NEW.epoch-1))
)
BEGIN SELECT RAISE(ABORT,'current enrollment lineage is inconsistent'); END;

CREATE TRIGGER fleet_hosts_current_enrollment_immutable
BEFORE UPDATE OF current_enrollment_id,current_enrollment_epoch
ON fleet_hosts
WHEN OLD.current_enrollment_id IS NOT NULL
  AND (NEW.current_enrollment_id IS NOT OLD.current_enrollment_id
    OR NEW.current_enrollment_epoch IS NOT OLD.current_enrollment_epoch)
  AND NOT (NEW.epoch > OLD.epoch AND NEW.current_enrollment_epoch=NEW.epoch)
BEGIN SELECT RAISE(ABORT,'current enrollment lineage is immutable'); END;
