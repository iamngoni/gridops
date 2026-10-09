-- Durable host observation sessions and typed inventory ownership.
-- Observation leases never grant work authority.  Agent supplied names and
-- capacities are kept separate from administrator policy columns.

CREATE TABLE fleet_observation_sessions (
  session_id TEXT PRIMARY KEY NOT NULL,
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  boot_id TEXT NOT NULL CHECK (length(boot_id) BETWEEN 1 AND 128),
  control_plane_incarnation TEXT NOT NULL,
  negotiated_protocol INTEGER NOT NULL CHECK (negotiated_protocol = 1),
  initial_credential_id TEXT NOT NULL,
  initial_credential_generation INTEGER NOT NULL CHECK (initial_credential_generation > 0),
  current_credential_id TEXT NOT NULL,
  current_credential_generation INTEGER NOT NULL CHECK (current_credential_generation > 0),
  initial_request_id TEXT NOT NULL,
  initial_body_digest TEXT NOT NULL CHECK (length(initial_body_digest) = 43),
  state TEXT NOT NULL CHECK (state IN ('observing','reconciling','ready','conflicted')),
  created_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  ended_at INTEGER,
  lease_expires_at INTEGER NOT NULL,
  current_inventory_sequence INTEGER NOT NULL DEFAULT 0 CHECK (current_inventory_sequence >= 0),
  current_inventory_digest TEXT,
  current_inventory_received_at INTEGER,
  current_heartbeat_sequence INTEGER NOT NULL DEFAULT 0 CHECK (current_heartbeat_sequence >= 0),
  current_heartbeat_digest TEXT,
  current_heartbeat_received_at INTEGER,
  handshake_received_at INTEGER NOT NULL,
  handshake_lease_expires_at INTEGER NOT NULL,
  handshake_state TEXT NOT NULL CHECK (handshake_state IN ('observing','reconciling','ready','conflicted')),
  current_heartbeat_lease_expires_at INTEGER,
  current_heartbeat_state TEXT CHECK (current_heartbeat_state IS NULL OR current_heartbeat_state IN ('observing','reconciling','ready','conflicted')),
  UNIQUE (host_id,session_id),
  FOREIGN KEY (host_id,initial_credential_id,initial_credential_generation,host_epoch)
    REFERENCES host_credentials(host_id,id,generation,host_epoch),
  FOREIGN KEY (host_id,current_credential_id,current_credential_generation,host_epoch)
    REFERENCES host_credentials(host_id,id,generation,host_epoch),
  CHECK (ended_at IS NULL OR ended_at >= created_at),
  CHECK (lease_expires_at >= last_seen_at)
) STRICT;
CREATE UNIQUE INDEX fleet_observation_sessions_active_host
  ON fleet_observation_sessions(host_id) WHERE ended_at IS NULL;
CREATE INDEX fleet_observation_sessions_history_idx
  ON fleet_observation_sessions(host_id,created_at,session_id);

CREATE TABLE fleet_observed_domains (
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  local_key TEXT NOT NULL CHECK (length(local_key) BETWEEN 1 AND 128),
  domain_id TEXT NOT NULL,
  parent_local_key TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('physical','virtual_machine','container')),
  observed_name TEXT NOT NULL CHECK (length(observed_name) BETWEEN 1 AND 128),
  runtime_identity TEXT,
  cpu_millis INTEGER CHECK (cpu_millis IS NULL OR cpu_millis >= 0),
  memory_mib INTEGER CHECK (memory_mib IS NULL OR memory_mib >= 0),
  disk_bytes INTEGER CHECK (disk_bytes IS NULL OR disk_bytes >= 0),
  session_id TEXT NOT NULL,
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  control_plane_incarnation TEXT NOT NULL,
  inventory_sequence INTEGER NOT NULL CHECK (inventory_sequence > 0),
  received_at INTEGER NOT NULL,
  PRIMARY KEY (host_id,local_key),
  UNIQUE (host_id,domain_id),
  FOREIGN KEY (domain_id,host_id) REFERENCES host_resource_domains(id,host_id),
  FOREIGN KEY (host_id,session_id) REFERENCES fleet_observation_sessions(host_id,session_id),
  CHECK (parent_local_key IS NULL OR parent_local_key != local_key)
) STRICT;
CREATE INDEX fleet_observed_domains_parent_idx
  ON fleet_observed_domains(host_id,parent_local_key);

CREATE TABLE fleet_observed_backends (
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  local_key TEXT NOT NULL CHECK (length(local_key) BETWEEN 1 AND 128),
  backend_id TEXT NOT NULL,
  domain_local_key TEXT NOT NULL,
  runtime_kind TEXT NOT NULL CHECK (runtime_kind IN ('docker','tart_vm','native_process')),
  execution_os TEXT NOT NULL CHECK (execution_os IN ('linux','macos','windows')),
  architecture TEXT NOT NULL CHECK (architecture IN ('x64','arm64')),
  capabilities_json TEXT NOT NULL CHECK (json_valid(capabilities_json) AND json_type(capabilities_json) = 'object'),
  observed_readiness TEXT NOT NULL CHECK (observed_readiness IN ('ready','reconciling','unavailable','unsupported','unknown')),
  reason TEXT,
  runtime_identity TEXT,
  session_id TEXT NOT NULL,
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  control_plane_incarnation TEXT NOT NULL,
  inventory_sequence INTEGER NOT NULL CHECK (inventory_sequence > 0),
  received_at INTEGER NOT NULL,
  PRIMARY KEY (host_id,local_key),
  UNIQUE (host_id,backend_id),
  FOREIGN KEY (backend_id,host_id) REFERENCES host_backends(id,host_id),
  FOREIGN KEY (host_id,session_id) REFERENCES fleet_observation_sessions(host_id,session_id),
  FOREIGN KEY (host_id,domain_local_key) REFERENCES fleet_observed_domains(host_id,local_key)
) STRICT;

CREATE TABLE fleet_observed_interactive_sessions (
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  local_key TEXT NOT NULL CHECK (length(local_key) BETWEEN 1 AND 128),
  session_id TEXT NOT NULL,
  os_user_id TEXT NOT NULL CHECK (length(os_user_id) BETWEEN 1 AND 128),
  observed_helper_state TEXT NOT NULL CHECK (observed_helper_state IN ('ready','unavailable','unknown')),
  authority_session_id TEXT NOT NULL,
  boot_id TEXT NOT NULL CHECK (length(boot_id) BETWEEN 1 AND 128),
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  control_plane_incarnation TEXT NOT NULL,
  inventory_sequence INTEGER NOT NULL CHECK (inventory_sequence > 0),
  received_at INTEGER NOT NULL,
  PRIMARY KEY (host_id,local_key),
  UNIQUE (host_id,session_id),
  FOREIGN KEY (session_id,host_id) REFERENCES host_sessions(id,host_id)
  ,FOREIGN KEY (host_id,authority_session_id) REFERENCES fleet_observation_sessions(host_id,session_id)
) STRICT;

CREATE TABLE fleet_inventory_snapshots (
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  authority_session_id TEXT NOT NULL,
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  control_plane_incarnation TEXT NOT NULL,
  inventory_sequence INTEGER NOT NULL CHECK (inventory_sequence > 0),
  digest TEXT NOT NULL CHECK (length(digest) = 43),
  snapshot_json TEXT NOT NULL CHECK (json_valid(snapshot_json) AND json_type(snapshot_json) = 'object'),
  received_at INTEGER NOT NULL,
  receipt_lease_expires_at INTEGER NOT NULL,
  receipt_state TEXT NOT NULL CHECK (receipt_state IN ('observing','reconciling','ready','conflicted')),
  inventory_revision INTEGER NOT NULL CHECK (inventory_revision > 0),
  domain_mappings_json TEXT NOT NULL CHECK (json_valid(domain_mappings_json) AND json_type(domain_mappings_json) = 'array'),
  backend_mappings_json TEXT NOT NULL CHECK (json_valid(backend_mappings_json) AND json_type(backend_mappings_json) = 'array'),
  interactive_mappings_json TEXT NOT NULL CHECK (json_valid(interactive_mappings_json) AND json_type(interactive_mappings_json) = 'array'),
  PRIMARY KEY (host_id,authority_session_id,inventory_sequence),
  UNIQUE (host_id,authority_session_id,inventory_sequence,digest),
  FOREIGN KEY (host_id,authority_session_id) REFERENCES fleet_observation_sessions(host_id,session_id)
) STRICT;

ALTER TABLE host_samples ADD COLUMN authority_session_id TEXT;
ALTER TABLE host_samples ADD COLUMN boot_id TEXT;
ALTER TABLE host_samples ADD COLUMN control_plane_incarnation TEXT;
CREATE TRIGGER host_samples_provenance_is_complete
BEFORE INSERT ON host_samples
WHEN (NEW.authority_session_id IS NOT NULL OR NEW.boot_id IS NOT NULL OR NEW.control_plane_incarnation IS NOT NULL)
 AND (NEW.authority_session_id IS NULL OR NEW.boot_id IS NULL OR NEW.control_plane_incarnation IS NULL)
BEGIN SELECT RAISE(ABORT,'sample provenance must be complete'); END;
CREATE TRIGGER host_samples_provenance_is_owned
BEFORE INSERT ON host_samples
WHEN NEW.authority_session_id IS NOT NULL
 AND NOT EXISTS (
   SELECT 1 FROM fleet_observation_sessions s
    WHERE s.host_id=NEW.host_id AND s.session_id=NEW.authority_session_id
      AND s.host_epoch=NEW.host_epoch AND s.boot_id=NEW.boot_id
      AND s.control_plane_incarnation=NEW.control_plane_incarnation
 )
BEGIN SELECT RAISE(ABORT,'sample provenance is not owned by the host session'); END;

CREATE TRIGGER host_samples_provenance_update_is_owned
BEFORE UPDATE OF host_id,host_epoch,authority_session_id,boot_id,control_plane_incarnation ON host_samples
WHEN NEW.authority_session_id IS NOT NULL
 AND NOT EXISTS (
   SELECT 1 FROM fleet_observation_sessions s
    WHERE s.host_id=NEW.host_id AND s.session_id=NEW.authority_session_id
      AND s.host_epoch=NEW.host_epoch AND s.boot_id=NEW.boot_id
      AND s.control_plane_incarnation=NEW.control_plane_incarnation
 )
BEGIN SELECT RAISE(ABORT,'sample provenance is not owned by the host session'); END;

CREATE TRIGGER host_samples_provenance_update_is_complete
BEFORE UPDATE OF authority_session_id,boot_id,control_plane_incarnation ON host_samples
WHEN (NEW.authority_session_id IS NOT NULL OR NEW.boot_id IS NOT NULL OR NEW.control_plane_incarnation IS NOT NULL)
 AND (NEW.authority_session_id IS NULL OR NEW.boot_id IS NULL OR NEW.control_plane_incarnation IS NULL)
BEGIN SELECT RAISE(ABORT,'sample provenance must be complete'); END;

CREATE TRIGGER host_samples_provenance_identity_immutable
BEFORE UPDATE OF host_id,domain_id,host_epoch,authority_session_id,boot_id,control_plane_incarnation ON host_samples
WHEN (OLD.authority_session_id IS NOT NULL OR OLD.boot_id IS NOT NULL OR OLD.control_plane_incarnation IS NOT NULL)
 AND (NEW.host_id IS NOT OLD.host_id OR NEW.domain_id IS NOT OLD.domain_id
   OR NEW.host_epoch IS NOT OLD.host_epoch
   OR NEW.authority_session_id IS NOT OLD.authority_session_id
   OR NEW.boot_id IS NOT OLD.boot_id
   OR NEW.control_plane_incarnation IS NOT OLD.control_plane_incarnation)
BEGIN SELECT RAISE(ABORT,'sample provenance identity is immutable'); END;

CREATE TRIGGER fleet_observation_heartbeat_receipt_insert
BEFORE INSERT ON fleet_observation_sessions
WHEN NOT (
  (NEW.current_heartbeat_sequence = 0
    AND NEW.current_heartbeat_digest IS NULL
    AND NEW.current_heartbeat_received_at IS NULL
    AND NEW.current_heartbeat_lease_expires_at IS NULL
    AND NEW.current_heartbeat_state IS NULL)
  OR (NEW.current_heartbeat_sequence > 0
    AND NEW.current_heartbeat_digest IS NOT NULL
    AND NEW.current_heartbeat_received_at IS NOT NULL
    AND NEW.current_heartbeat_lease_expires_at IS NOT NULL
    AND NEW.current_heartbeat_state IS NOT NULL)
)
BEGIN SELECT RAISE(ABORT,'heartbeat receipt must be complete'); END;

CREATE TRIGGER fleet_observation_heartbeat_receipt_update
BEFORE UPDATE OF current_heartbeat_sequence,current_heartbeat_digest,current_heartbeat_received_at,current_heartbeat_lease_expires_at,current_heartbeat_state ON fleet_observation_sessions
WHEN NOT (
  (NEW.current_heartbeat_sequence = 0
    AND NEW.current_heartbeat_digest IS NULL
    AND NEW.current_heartbeat_received_at IS NULL
    AND NEW.current_heartbeat_lease_expires_at IS NULL
    AND NEW.current_heartbeat_state IS NULL)
  OR (NEW.current_heartbeat_sequence > 0
    AND NEW.current_heartbeat_digest IS NOT NULL
    AND NEW.current_heartbeat_received_at IS NOT NULL
    AND NEW.current_heartbeat_lease_expires_at IS NOT NULL
    AND NEW.current_heartbeat_state IS NOT NULL)
)
BEGIN SELECT RAISE(ABORT,'heartbeat receipt must be complete'); END;

CREATE TRIGGER fleet_observation_session_identity_immutable
BEFORE UPDATE OF host_id,session_id,host_epoch,boot_id,control_plane_incarnation,negotiated_protocol,initial_credential_id,initial_credential_generation,initial_request_id,initial_body_digest,handshake_received_at,handshake_lease_expires_at,handshake_state ON fleet_observation_sessions
WHEN NEW.host_id IS NOT OLD.host_id OR NEW.session_id IS NOT OLD.session_id
 OR NEW.host_epoch IS NOT OLD.host_epoch OR NEW.boot_id IS NOT OLD.boot_id
 OR NEW.control_plane_incarnation IS NOT OLD.control_plane_incarnation
 OR NEW.negotiated_protocol IS NOT OLD.negotiated_protocol
 OR NEW.initial_credential_id IS NOT OLD.initial_credential_id
 OR NEW.initial_credential_generation IS NOT OLD.initial_credential_generation
 OR NEW.initial_request_id IS NOT OLD.initial_request_id
 OR NEW.initial_body_digest IS NOT OLD.initial_body_digest
 OR NEW.handshake_received_at IS NOT OLD.handshake_received_at
 OR NEW.handshake_lease_expires_at IS NOT OLD.handshake_lease_expires_at
 OR NEW.handshake_state IS NOT OLD.handshake_state
BEGIN SELECT RAISE(ABORT,'observation session identity is immutable'); END;

CREATE TRIGGER fleet_observation_session_current_credential_guard
BEFORE UPDATE OF current_credential_id,current_credential_generation ON fleet_observation_sessions
WHEN (NEW.current_credential_id IS NOT OLD.current_credential_id
   OR NEW.current_credential_generation IS NOT OLD.current_credential_generation)
 AND NOT EXISTS (
   SELECT 1 FROM host_credential_rotations r
    WHERE r.host_id=NEW.host_id
      AND r.old_credential_id=OLD.current_credential_id
      AND r.old_generation=OLD.current_credential_generation
      AND r.old_host_epoch=OLD.host_epoch
      AND r.next_generation=NEW.current_credential_generation
      AND r.phase IN ('exchanged','acknowledged')
      AND r.revoked_at IS NULL
 )
BEGIN SELECT RAISE(ABORT,'observation session credential advancement is not acknowledged'); END;

CREATE TRIGGER fleet_observed_domain_provenance_is_owned
BEFORE INSERT ON fleet_observed_domains
WHEN NOT EXISTS (
  SELECT 1 FROM fleet_observation_sessions s
   WHERE s.host_id=NEW.host_id AND s.session_id=NEW.session_id
     AND s.host_epoch=NEW.host_epoch
     AND s.control_plane_incarnation=NEW.control_plane_incarnation
)
BEGIN SELECT RAISE(ABORT,'observed domain provenance is not owned'); END;

CREATE TRIGGER fleet_observed_domain_owner_immutable
BEFORE UPDATE OF host_id,local_key,domain_id,parent_local_key,kind,runtime_identity ON fleet_observed_domains
WHEN NEW.host_id IS NOT OLD.host_id OR NEW.local_key IS NOT OLD.local_key OR NEW.domain_id IS NOT OLD.domain_id
 OR NEW.parent_local_key IS NOT OLD.parent_local_key OR NEW.kind IS NOT OLD.kind
 OR NEW.runtime_identity IS NOT OLD.runtime_identity
BEGIN SELECT RAISE(ABORT,'observed domain ownership is immutable'); END;

CREATE TRIGGER fleet_observed_domain_provenance_update_is_owned
BEFORE UPDATE OF host_id,session_id,host_epoch,control_plane_incarnation ON fleet_observed_domains
WHEN NOT EXISTS (
  SELECT 1 FROM fleet_observation_sessions s
   WHERE s.host_id=NEW.host_id AND s.session_id=NEW.session_id
     AND s.host_epoch=NEW.host_epoch
     AND s.control_plane_incarnation=NEW.control_plane_incarnation
)
BEGIN SELECT RAISE(ABORT,'observed domain provenance is not owned'); END;

CREATE TRIGGER fleet_observed_backend_provenance_is_owned
BEFORE INSERT ON fleet_observed_backends
WHEN NOT EXISTS (
  SELECT 1 FROM fleet_observation_sessions s
   WHERE s.host_id=NEW.host_id AND s.session_id=NEW.session_id
     AND s.host_epoch=NEW.host_epoch
     AND s.control_plane_incarnation=NEW.control_plane_incarnation
)
BEGIN SELECT RAISE(ABORT,'observed backend provenance is not owned'); END;

CREATE TRIGGER fleet_observed_backend_owner_immutable
BEFORE UPDATE OF host_id,local_key,backend_id,domain_local_key,runtime_kind,execution_os,architecture,runtime_identity ON fleet_observed_backends
WHEN NEW.host_id IS NOT OLD.host_id OR NEW.local_key IS NOT OLD.local_key OR NEW.backend_id IS NOT OLD.backend_id
 OR NEW.domain_local_key IS NOT OLD.domain_local_key OR NEW.runtime_kind IS NOT OLD.runtime_kind
 OR NEW.execution_os IS NOT OLD.execution_os OR NEW.architecture IS NOT OLD.architecture
 OR NEW.runtime_identity IS NOT OLD.runtime_identity
BEGIN SELECT RAISE(ABORT,'observed backend ownership is immutable'); END;

CREATE TRIGGER fleet_observed_backend_provenance_update_is_owned
BEFORE UPDATE OF host_id,session_id,host_epoch,control_plane_incarnation ON fleet_observed_backends
WHEN NOT EXISTS (
  SELECT 1 FROM fleet_observation_sessions s
   WHERE s.host_id=NEW.host_id AND s.session_id=NEW.session_id
     AND s.host_epoch=NEW.host_epoch
     AND s.control_plane_incarnation=NEW.control_plane_incarnation
)
BEGIN SELECT RAISE(ABORT,'observed backend provenance is not owned'); END;

CREATE TRIGGER fleet_observed_helper_provenance_is_owned
BEFORE INSERT ON fleet_observed_interactive_sessions
WHEN NOT EXISTS (
  SELECT 1 FROM fleet_observation_sessions s
   WHERE s.host_id=NEW.host_id AND s.session_id=NEW.authority_session_id
     AND s.host_epoch=NEW.host_epoch AND s.boot_id=NEW.boot_id
     AND s.control_plane_incarnation=NEW.control_plane_incarnation
)
BEGIN SELECT RAISE(ABORT,'observed helper provenance is not owned'); END;

CREATE TRIGGER fleet_observed_helper_owner_immutable
BEFORE UPDATE OF host_id,local_key,session_id,os_user_id ON fleet_observed_interactive_sessions
WHEN NEW.host_id IS NOT OLD.host_id OR NEW.local_key IS NOT OLD.local_key
 OR NEW.session_id IS NOT OLD.session_id OR NEW.os_user_id IS NOT OLD.os_user_id
BEGIN SELECT RAISE(ABORT,'observed helper ownership is immutable'); END;

CREATE TRIGGER fleet_observed_helper_provenance_update_is_owned
BEFORE UPDATE OF host_id,authority_session_id,host_epoch,boot_id,control_plane_incarnation ON fleet_observed_interactive_sessions
WHEN NOT EXISTS (
  SELECT 1 FROM fleet_observation_sessions s
   WHERE s.host_id=NEW.host_id AND s.session_id=NEW.authority_session_id
     AND s.host_epoch=NEW.host_epoch AND s.boot_id=NEW.boot_id
     AND s.control_plane_incarnation=NEW.control_plane_incarnation
)
BEGIN SELECT RAISE(ABORT,'observed helper provenance is not owned'); END;

CREATE TRIGGER fleet_inventory_snapshot_provenance_is_owned
BEFORE INSERT ON fleet_inventory_snapshots
WHEN NOT EXISTS (
  SELECT 1 FROM fleet_observation_sessions s
   WHERE s.host_id=NEW.host_id AND s.session_id=NEW.authority_session_id
     AND s.host_epoch=NEW.host_epoch
     AND s.control_plane_incarnation=NEW.control_plane_incarnation
)
BEGIN SELECT RAISE(ABORT,'inventory snapshot provenance is not owned'); END;

CREATE TRIGGER fleet_inventory_snapshot_immutable
BEFORE UPDATE ON fleet_inventory_snapshots
WHEN NEW.host_id IS NOT OLD.host_id OR NEW.authority_session_id IS NOT OLD.authority_session_id
 OR NEW.host_epoch IS NOT OLD.host_epoch OR NEW.control_plane_incarnation IS NOT OLD.control_plane_incarnation
 OR NEW.inventory_sequence IS NOT OLD.inventory_sequence OR NEW.digest IS NOT OLD.digest
 OR NEW.snapshot_json IS NOT OLD.snapshot_json OR NEW.received_at IS NOT OLD.received_at
 OR NEW.receipt_lease_expires_at IS NOT OLD.receipt_lease_expires_at
 OR NEW.receipt_state IS NOT OLD.receipt_state OR NEW.inventory_revision IS NOT OLD.inventory_revision
 OR NEW.domain_mappings_json IS NOT OLD.domain_mappings_json
 OR NEW.backend_mappings_json IS NOT OLD.backend_mappings_json
 OR NEW.interactive_mappings_json IS NOT OLD.interactive_mappings_json
BEGIN SELECT RAISE(ABORT,'inventory snapshot history is immutable'); END;

CREATE TRIGGER fleet_inventory_snapshot_update_provenance_is_owned
BEFORE UPDATE OF host_id,authority_session_id,host_epoch,control_plane_incarnation ON fleet_inventory_snapshots
WHEN NOT EXISTS (
  SELECT 1 FROM fleet_observation_sessions s
   WHERE s.host_id=NEW.host_id AND s.session_id=NEW.authority_session_id
     AND s.host_epoch=NEW.host_epoch
     AND s.control_plane_incarnation=NEW.control_plane_incarnation
)
BEGIN SELECT RAISE(ABORT,'inventory snapshot provenance is not owned'); END;
