-- Durable fleet identity and admission. This migration is additive: legacy
-- providers keep their original constraints until the guarded fleet cutover.
-- Ownership history uses restrictive FKs; retirement is a state, not deletion.

CREATE TABLE fleet_control_plane (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  incarnation TEXT NOT NULL,
  readiness TEXT NOT NULL CHECK (readiness IN ('legacy','paused','active','restoring')),
  schema_version INTEGER NOT NULL CHECK (schema_version > 0),
  protocol_min INTEGER NOT NULL CHECK (protocol_min > 0),
  protocol_max INTEGER NOT NULL CHECK (protocol_max >= protocol_min),
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE fleet_hosts (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
  host_os TEXT NOT NULL CHECK (host_os IN ('linux','macos','windows')),
  architecture TEXT NOT NULL CHECK (architecture IN ('x64','arm64')),
  enrollment_state TEXT NOT NULL DEFAULT 'pending' CHECK (enrollment_state IN ('pending','approved','revoked')),
  lifecycle_state TEXT NOT NULL DEFAULT 'paused' CHECK (lifecycle_state IN ('active','paused','draining','maintenance','retired')),
  integrity_state TEXT NOT NULL DEFAULT 'unverified' CHECK (integrity_state IN ('unverified','verified','quarantined')),
  epoch INTEGER NOT NULL DEFAULT 1 CHECK (epoch > 0),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
  boot_id TEXT,
  agent_session_id TEXT,
  agent_version TEXT,
  hardware_fingerprint TEXT,
  tags_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(tags_json) AND json_type(tags_json) = 'object'),
  last_heartbeat_at INTEGER,
  last_inventory_at INTEGER,
  authority_expires_at INTEGER,
  inventory_complete INTEGER NOT NULL DEFAULT 0 CHECK (inventory_complete IN (0,1)),
  pressure_state TEXT NOT NULL DEFAULT 'unknown' CHECK (pressure_state IN ('normal','pressured','unknown')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  retired_at INTEGER,
  CHECK (lifecycle_state != 'active' OR enrollment_state = 'approved')
) STRICT;
CREATE INDEX fleet_hosts_inventory_idx ON fleet_hosts (enrollment_state,lifecycle_state,last_heartbeat_at);
CREATE INDEX fleet_hosts_hardware_idx ON fleet_hosts (hardware_fingerprint);

CREATE TABLE host_resource_domains (
  id TEXT PRIMARY KEY NOT NULL,
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  parent_domain_id TEXT,
  name TEXT NOT NULL,
  cpu_millis INTEGER NOT NULL CHECK (cpu_millis >= 0),
  memory_mib INTEGER NOT NULL CHECK (memory_mib >= 0),
  disk_bytes INTEGER NOT NULL CHECK (disk_bytes >= 0),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
  mount_identity TEXT,
  UNIQUE (id,host_id),
  FOREIGN KEY (parent_domain_id,host_id) REFERENCES host_resource_domains(id,host_id),
  CHECK (parent_domain_id IS NULL OR parent_domain_id != id)
) STRICT;
CREATE UNIQUE INDEX host_resource_domains_root_unique ON host_resource_domains(host_id) WHERE parent_domain_id IS NULL;
CREATE INDEX host_resource_domains_parent_idx ON host_resource_domains(parent_domain_id,host_id);

CREATE TABLE host_backends (
  id TEXT PRIMARY KEY NOT NULL,
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  domain_id TEXT NOT NULL,
  runtime_kind TEXT NOT NULL CHECK (runtime_kind IN ('docker','tart_vm','native_process')),
  execution_os TEXT NOT NULL CHECK (execution_os IN ('linux','macos','windows')),
  architecture TEXT NOT NULL CHECK (architecture IN ('x64','arm64')),
  readiness TEXT NOT NULL DEFAULT 'unknown' CHECK (readiness IN ('ready','reconciling','unavailable','unsupported','unknown')),
  reason_code TEXT,
  enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
  capabilities_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(capabilities_json) AND json_type(capabilities_json) = 'object'),
  config_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(config_json) AND json_type(config_json) = 'object'),
  last_probed_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (id,host_id),
  FOREIGN KEY (domain_id,host_id) REFERENCES host_resource_domains(id,host_id)
) STRICT;
CREATE INDEX host_backends_eligibility_idx ON host_backends (host_id,enabled,readiness,runtime_kind,execution_os,architecture);

CREATE TABLE fleet_ci_targets (
  id TEXT PRIMARY KEY NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('github_repository','github_organization','bitbucket_workspace','bitbucket_repository')),
  canonical_key TEXT NOT NULL UNIQUE,
  installation_id INTEGER REFERENCES installations(id),
  repository_id INTEGER REFERENCES repositories(id),
  organization_id INTEGER,
  bitbucket_connection_id TEXT REFERENCES bitbucket_connections(id),
  workspace_uuid TEXT,
  repository_uuid TEXT,
  CHECK (
    (kind = 'github_repository' AND installation_id IS NOT NULL AND installation_id > 0 AND repository_id IS NOT NULL AND repository_id > 0 AND organization_id IS NULL AND bitbucket_connection_id IS NULL AND workspace_uuid IS NULL AND repository_uuid IS NULL)
    OR (kind = 'github_organization' AND installation_id IS NOT NULL AND installation_id > 0 AND organization_id IS NOT NULL AND organization_id > 0 AND repository_id IS NULL AND bitbucket_connection_id IS NULL AND workspace_uuid IS NULL AND repository_uuid IS NULL)
    OR (kind = 'bitbucket_workspace' AND bitbucket_connection_id IS NOT NULL AND workspace_uuid IS NOT NULL AND repository_uuid IS NULL AND installation_id IS NULL AND repository_id IS NULL AND organization_id IS NULL)
    OR (kind = 'bitbucket_repository' AND bitbucket_connection_id IS NOT NULL AND workspace_uuid IS NOT NULL AND repository_uuid IS NOT NULL AND installation_id IS NULL AND repository_id IS NULL AND organization_id IS NULL)
  )
) STRICT;

CREATE TABLE host_target_grants (
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  target_id TEXT NOT NULL REFERENCES fleet_ci_targets(id),
  allow_schedule INTEGER NOT NULL DEFAULT 0 CHECK (allow_schedule IN (0,1)),
  allow_native INTEGER NOT NULL DEFAULT 0 CHECK (allow_native IN (0,1)),
  allow_interactive INTEGER NOT NULL DEFAULT 0 CHECK (allow_interactive IN (0,1)),
  allow_docker_socket INTEGER NOT NULL DEFAULT 0 CHECK (allow_docker_socket IN (0,1)),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
  revoked_at INTEGER,
  granted_by TEXT REFERENCES users(id),
  PRIMARY KEY (host_id,target_id)
) STRICT;
CREATE TABLE host_user_grants (
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  user_id TEXT NOT NULL REFERENCES users(id),
  permission TEXT NOT NULL CHECK (permission IN ('read','operator','admin')),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
  revoked_at INTEGER,
  PRIMARY KEY (host_id,user_id)
) STRICT;
CREATE TABLE bitbucket_user_grants (
  connection_id TEXT NOT NULL REFERENCES bitbucket_connections(id),
  user_id TEXT NOT NULL REFERENCES users(id),
  permission TEXT NOT NULL CHECK (permission IN ('read','admin')),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
  revoked_at INTEGER,
  PRIMARY KEY (connection_id,user_id)
) STRICT;
CREATE TABLE host_sessions (
  id TEXT PRIMARY KEY NOT NULL,
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  session_key TEXT NOT NULL,
  os_user_id TEXT NOT NULL,
  helper_state TEXT NOT NULL CHECK (helper_state IN ('ready','unavailable','unknown')),
  authorized INTEGER NOT NULL DEFAULT 0 CHECK (authorized IN (0,1)),
  max_jobs INTEGER NOT NULL DEFAULT 1 CHECK (max_jobs BETWEEN 1 AND 100),
  last_seen_at INTEGER,
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
  UNIQUE (id,host_id),
  UNIQUE (host_id,session_key)
) STRICT;
CREATE TABLE host_session_target_grants (
  session_id TEXT NOT NULL REFERENCES host_sessions(id),
  target_id TEXT NOT NULL REFERENCES fleet_ci_targets(id),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
  revoked_at INTEGER,
  PRIMARY KEY (session_id,target_id)
) STRICT;

CREATE TABLE pool_execution_profiles (
  id TEXT PRIMARY KEY NOT NULL,
  pool_id TEXT NOT NULL REFERENCES runner_pools(id),
  name TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
  enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
  runtime_kind TEXT NOT NULL CHECK (runtime_kind IN ('docker','tart_vm','native_process')),
  execution_os TEXT NOT NULL CHECK (execution_os IN ('linux','macos','windows')),
  architecture TEXT NOT NULL CHECK (architecture IN ('x64','arm64')),
  mode TEXT NOT NULL CHECK (mode IN ('ephemeral','persistent')),
  requires_interactive INTEGER NOT NULL DEFAULT 0 CHECK (requires_interactive IN (0,1)),
  requires_docker_socket INTEGER NOT NULL DEFAULT 0 CHECK (requires_docker_socket IN (0,1)),
  all_authorized_hosts INTEGER NOT NULL DEFAULT 0 CHECK (all_authorized_hosts IN (0,1)),
  preference INTEGER NOT NULL DEFAULT 0,
  image TEXT NOT NULL,
  labels_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(labels_json) AND json_type(labels_json) = 'array'),
  config_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(config_json) AND json_type(config_json) = 'object'),
  cpu_millis INTEGER NOT NULL CHECK (cpu_millis > 0),
  memory_mib INTEGER NOT NULL CHECK (memory_mib > 0),
  disk_bytes INTEGER NOT NULL CHECK (disk_bytes >= 0),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (id,pool_id)
) STRICT;
CREATE INDEX pool_execution_profiles_pool_idx ON pool_execution_profiles(pool_id,enabled,id);
CREATE TABLE profile_ci_targets (
  profile_id TEXT NOT NULL REFERENCES pool_execution_profiles(id),
  target_id TEXT NOT NULL REFERENCES fleet_ci_targets(id),
  enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
  revoked_at INTEGER,
  PRIMARY KEY (profile_id,target_id)
) STRICT;
CREATE TABLE profile_host_selectors (
  id INTEGER PRIMARY KEY,
  profile_id TEXT NOT NULL REFERENCES pool_execution_profiles(id),
  kind TEXT NOT NULL CHECK (kind IN ('host','tag')),
  host_id TEXT REFERENCES fleet_hosts(id),
  tag_key TEXT,
  tag_value TEXT,
  CHECK ((kind = 'host' AND host_id IS NOT NULL AND tag_key IS NULL AND tag_value IS NULL)
    OR (kind = 'tag' AND host_id IS NULL AND tag_key IS NOT NULL AND tag_value IS NOT NULL))
) STRICT;
CREATE INDEX profile_host_selectors_profile_idx ON profile_host_selectors(profile_id);

CREATE TABLE fleet_operations (
  id TEXT PRIMARY KEY NOT NULL,
  parent_id TEXT REFERENCES fleet_operations(id),
  kind TEXT NOT NULL CHECK (kind IN ('submit_workload','edit_host','pause_host','resume_host','drain_host','maintain_host','retire_host','revoke_host','upgrade_agent','adopt_runner','enroll_host','recover_host')),
  host_id TEXT REFERENCES fleet_hosts(id),
  requested_by_user_id TEXT REFERENCES users(id),
  principal_kind TEXT NOT NULL CHECK (principal_kind IN ('user','reconciler','autoscaler','fix_agent')),
  principal_scope TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','running','succeeded','failed','cancelled','blocked')),
  reason_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (principal_scope,idempotency_key),
  CHECK ((principal_kind = 'user' AND requested_by_user_id IS NOT NULL) OR (principal_kind != 'user' AND requested_by_user_id IS NULL)),
  CHECK (parent_id IS NULL OR parent_id != id)
) STRICT;
CREATE INDEX fleet_operations_host_idx ON fleet_operations(host_id,created_at,id);

CREATE TABLE workload_intents (
  id TEXT PRIMARY KEY NOT NULL REFERENCES fleet_operations(id),
  workload_id TEXT NOT NULL,
  generation INTEGER NOT NULL CHECK (generation > 0),
  kind TEXT NOT NULL CHECK (kind IN ('ci_runner','fix_sandbox')),
  pool_id TEXT NOT NULL REFERENCES runner_pools(id),
  profile_id TEXT NOT NULL,
  target_id TEXT NOT NULL REFERENCES fleet_ci_targets(id),
  agent_run_id TEXT REFERENCES agent_runs(id),
  expected_profile_revision INTEGER NOT NULL CHECK (expected_profile_revision > 0),
  status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','admitted','cancelled','rejected')),
  reason_code TEXT,
  explanation_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(explanation_json) AND json_type(explanation_json) = 'object'),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (workload_id,generation),
  UNIQUE (id,workload_id,generation),
  UNIQUE (id,workload_id,generation,target_id),
  FOREIGN KEY (profile_id,pool_id) REFERENCES pool_execution_profiles(id,pool_id),
  FOREIGN KEY (profile_id,target_id) REFERENCES profile_ci_targets(profile_id,target_id),
  CHECK ((kind = 'ci_runner' AND agent_run_id IS NULL) OR (kind = 'fix_sandbox' AND agent_run_id IS NOT NULL))
) STRICT;
CREATE INDEX workload_intents_queue_idx ON workload_intents(status,pool_id,profile_id,created_at,id);

CREATE TABLE workload_placements (
  id TEXT PRIMARY KEY NOT NULL,
  intent_id TEXT NOT NULL UNIQUE,
  workload_id TEXT NOT NULL,
  generation INTEGER NOT NULL CHECK (generation > 0),
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  backend_id TEXT NOT NULL,
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  session_id TEXT,
  profile_revision INTEGER NOT NULL CHECK (profile_revision > 0),
  target_id TEXT NOT NULL REFERENCES fleet_ci_targets(id),
  config_snapshot_json TEXT NOT NULL CHECK (json_valid(config_snapshot_json) AND json_type(config_snapshot_json) = 'object'),
  cpu_millis INTEGER NOT NULL CHECK (cpu_millis > 0),
  memory_mib INTEGER NOT NULL CHECK (memory_mib > 0),
  disk_bytes INTEGER NOT NULL CHECK (disk_bytes >= 0),
  state TEXT NOT NULL CHECK (state IN ('reserved','preparing','registering','starting','running','draining','stopping','stopped','cleaned','retryable','failed','uncertain')),
  runner_id TEXT REFERENCES runners(id),
  agent_run_id TEXT REFERENCES agent_runs(id),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  local_absence_proven_at INTEGER,
  delayed_starts_excluded_at INTEGER,
  upstream_cleanup_state TEXT NOT NULL DEFAULT 'not_registered' CHECK (upstream_cleanup_state IN ('not_registered','pending','uncertain','removed')),
  UNIQUE (workload_id,generation),
  UNIQUE (id,host_id,backend_id,host_epoch),
  UNIQUE (id,host_id,target_id),
  UNIQUE (id,generation),
  FOREIGN KEY (intent_id,workload_id,generation,target_id) REFERENCES workload_intents(id,workload_id,generation,target_id),
  FOREIGN KEY (backend_id,host_id) REFERENCES host_backends(id,host_id),
  FOREIGN KEY (session_id,host_id) REFERENCES host_sessions(id,host_id)
) STRICT;
CREATE INDEX workload_placements_host_idx ON workload_placements(host_id,state);
CREATE INDEX workload_placements_session_idx ON workload_placements(session_id,state);
CREATE INDEX workload_placements_runner_idx ON workload_placements(runner_id);

CREATE TABLE capacity_allocations (
  id TEXT PRIMARY KEY NOT NULL,
  placement_id TEXT NOT NULL,
  host_id TEXT NOT NULL,
  backend_id TEXT NOT NULL,
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  domain_id TEXT NOT NULL,
  policy_revision INTEGER NOT NULL CHECK (policy_revision > 0),
  cpu_millis INTEGER NOT NULL CHECK (cpu_millis > 0),
  memory_mib INTEGER NOT NULL CHECK (memory_mib > 0),
  disk_bytes INTEGER NOT NULL CHECK (disk_bytes >= 0),
  state TEXT NOT NULL DEFAULT 'reserved' CHECK (state IN ('reserved','committed','releasing','uncertain','released')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  released_at INTEGER,
  UNIQUE (placement_id,domain_id),
  FOREIGN KEY (placement_id,host_id,backend_id,host_epoch) REFERENCES workload_placements(id,host_id,backend_id,host_epoch),
  FOREIGN KEY (domain_id,host_id) REFERENCES host_resource_domains(id,host_id),
  CHECK ((state = 'released' AND released_at IS NOT NULL) OR (state != 'released' AND released_at IS NULL))
) STRICT;
CREATE INDEX capacity_allocations_domain_idx ON capacity_allocations(domain_id,state);

CREATE TABLE host_commands (
  cursor INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  operation_id TEXT NOT NULL REFERENCES fleet_operations(id),
  scope TEXT NOT NULL CHECK (scope IN ('host','placement')),
  placement_id TEXT,
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  backend_id TEXT,
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  generation INTEGER CHECK (generation > 0),
  kind TEXT NOT NULL CHECK (kind IN ('prepare_environment','start_runner','inspect','drain','stop','cleanup','upgrade_agent','execute_sandbox','sandbox_file','sandbox_artifact')),
  state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','delivered','accepted','running','succeeded','failed','uncertain','cancelled')),
  protocol_version INTEGER NOT NULL CHECK (protocol_version = 1),
  request_hash TEXT NOT NULL,
  payload_json TEXT NOT NULL CHECK (json_valid(payload_json) AND json_type(payload_json) = 'object'),
  expected_revision INTEGER NOT NULL CHECK (expected_revision > 0),
  issued_at INTEGER NOT NULL,
  deadline_at INTEGER NOT NULL CHECK (deadline_at > issued_at),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 10),
  idempotency_key TEXT NOT NULL,
  UNIQUE (host_id,idempotency_key),
  FOREIGN KEY (placement_id,host_id,backend_id,host_epoch) REFERENCES workload_placements(id,host_id,backend_id,host_epoch),
  FOREIGN KEY (placement_id,generation) REFERENCES workload_placements(id,generation),
  CHECK ((scope = 'host' AND placement_id IS NULL AND backend_id IS NULL AND generation IS NULL AND kind IN ('inspect','drain','upgrade_agent'))
    OR (scope = 'placement' AND placement_id IS NOT NULL AND backend_id IS NOT NULL AND generation IS NOT NULL AND kind != 'upgrade_agent'))
) STRICT;
CREATE INDEX host_commands_poll_idx ON host_commands(host_id,state,cursor);
CREATE TABLE host_command_attempts (
  id INTEGER PRIMARY KEY,
  command_id TEXT NOT NULL REFERENCES host_commands(id),
  attempt INTEGER NOT NULL CHECK (attempt BETWEEN 1 AND 10),
  state TEXT NOT NULL CHECK (state IN ('delivered','accepted','running','succeeded','failed','uncertain')),
  recorded_at INTEGER NOT NULL,
  reason_code TEXT,
  UNIQUE (command_id,attempt)
) STRICT;
CREATE TABLE host_command_results (
  id TEXT PRIMARY KEY NOT NULL,
  command_id TEXT NOT NULL REFERENCES host_commands(id),
  result_hash TEXT NOT NULL,
  disposition TEXT NOT NULL CHECK (disposition IN ('applied','stale','conflict')),
  result_json TEXT NOT NULL CHECK (json_valid(result_json) AND json_type(result_json) = 'object'),
  received_at INTEGER NOT NULL,
  UNIQUE (command_id,result_hash)
) STRICT;

CREATE TABLE host_enrollments (
  id TEXT PRIMARY KEY NOT NULL,
  code_verifier TEXT NOT NULL UNIQUE,
  intended_host_id TEXT REFERENCES fleet_hosts(id),
  scope_json TEXT NOT NULL CHECK (json_valid(scope_json) AND json_type(scope_json) = 'object'),
  issued_by TEXT NOT NULL REFERENCES users(id),
  created_at INTEGER NOT NULL,
  -- Default issuance is 10 minutes; the protocol permits a 1-hour hard cap.
  expires_at INTEGER NOT NULL CHECK (expires_at > created_at AND expires_at <= created_at + 3600000),
  consumed_at INTEGER,
  consumed_host_id TEXT REFERENCES fleet_hosts(id),
  revoked_at INTEGER,
  CHECK ((consumed_at IS NULL) = (consumed_host_id IS NULL))
) STRICT;
CREATE INDEX host_enrollments_expiry_idx ON host_enrollments(expires_at);
CREATE TABLE host_credentials (
  id TEXT PRIMARY KEY NOT NULL,
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  generation INTEGER NOT NULL CHECK (generation > 0),
  verifier TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  overlap_expires_at INTEGER,
  acknowledged_at INTEGER,
  revoked_at INTEGER,
  UNIQUE (host_id,generation)
) STRICT;
CREATE INDEX host_credentials_host_idx ON host_credentials(host_id,revoked_at);
CREATE TABLE host_agent_cursors (
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  stream TEXT NOT NULL CHECK (stream IN ('commands','events','logs','artifacts')),
  acknowledged_cursor INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged_cursor >= 0),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (host_id,host_epoch,stream)
) STRICT;
CREATE TABLE host_samples (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  host_id TEXT NOT NULL REFERENCES fleet_hosts(id),
  domain_id TEXT,
  host_epoch INTEGER NOT NULL CHECK (host_epoch > 0),
  observed_at INTEGER NOT NULL,
  received_at INTEGER NOT NULL,
  cpu_used_millis INTEGER CHECK (cpu_used_millis >= 0),
  memory_used_mib INTEGER CHECK (memory_used_mib >= 0),
  disk_free_bytes INTEGER CHECK (disk_free_bytes >= 0),
  uptime_seconds INTEGER CHECK (uptime_seconds >= 0),
  coverage TEXT NOT NULL CHECK (coverage IN ('complete','partial','unknown')),
  FOREIGN KEY (domain_id,host_id) REFERENCES host_resource_domains(id,host_id)
) STRICT;
CREATE INDEX host_samples_host_time_idx ON host_samples(host_id,observed_at,id);
CREATE TABLE fleet_events (
  cursor INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  host_id TEXT REFERENCES fleet_hosts(id),
  placement_id TEXT REFERENCES workload_placements(id),
  target_id TEXT REFERENCES fleet_ci_targets(id),
  actor_user_id TEXT REFERENCES users(id),
  kind TEXT NOT NULL,
  detail_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(detail_json) AND json_type(detail_json) = 'object'),
  created_at INTEGER NOT NULL,
  FOREIGN KEY (placement_id,host_id,target_id) REFERENCES workload_placements(id,host_id,target_id),
  CHECK (placement_id IS NULL OR (host_id IS NOT NULL AND target_id IS NOT NULL))
) STRICT;
CREATE INDEX fleet_events_host_cursor_idx ON fleet_events(host_id,cursor);
CREATE INDEX fleet_events_target_cursor_idx ON fleet_events(target_id,cursor);
CREATE TABLE external_runner_observations (
  id TEXT PRIMARY KEY NOT NULL,
  target_id TEXT NOT NULL REFERENCES fleet_ci_targets(id),
  host_id TEXT REFERENCES fleet_hosts(id),
  upstream_id TEXT NOT NULL,
  source TEXT NOT NULL CHECK (source IN ('upstream','local','correlated')),
  confidence TEXT NOT NULL CHECK (confidence IN ('proven','unknown','conflicting')),
  freshness TEXT NOT NULL CHECK (freshness IN ('fresh','stale','unreachable','unknown')),
  owner_kind TEXT NOT NULL CHECK (owner_kind IN ('gridops','external','conflicting','unknown')),
  evidence_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(evidence_json) AND json_type(evidence_json) = 'object'),
  observed_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (target_id,upstream_id,source),
  CHECK (confidence != 'proven' OR host_id IS NOT NULL)
) STRICT;
CREATE INDEX external_runner_observations_host_idx ON external_runner_observations(host_id,updated_at);
CREATE TABLE adoption_operations (
  id TEXT PRIMARY KEY NOT NULL REFERENCES fleet_operations(id),
  observation_id TEXT NOT NULL REFERENCES external_runner_observations(id),
  profile_id TEXT NOT NULL REFERENCES pool_execution_profiles(id),
  requested_by TEXT NOT NULL REFERENCES users(id),
  fleet_approved_by TEXT REFERENCES users(id),
  target_approved_by TEXT REFERENCES users(id),
  state TEXT NOT NULL CHECK (state IN ('preflight','awaiting_approval','quiescing','retiring','registering','succeeded','blocked','rolled_back','failed')),
  replacement_placement_id TEXT REFERENCES workload_placements(id),
  evidence_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(evidence_json) AND json_type(evidence_json) = 'object'),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX adoption_operations_active_unique ON adoption_operations(observation_id)
  WHERE state NOT IN ('succeeded','rolled_back','failed');

-- Ownership and original charges cannot be rewritten by a stale host result.
CREATE TRIGGER workload_placements_immutable_owner
BEFORE UPDATE OF intent_id,workload_id,generation,host_id,backend_id,host_epoch,session_id,profile_revision,target_id,config_snapshot_json,cpu_millis,memory_mib,disk_bytes ON workload_placements
BEGIN SELECT RAISE(ABORT,'placement ownership and snapshot are immutable'); END;
CREATE TRIGGER capacity_allocations_immutable_owner
BEFORE UPDATE OF placement_id,host_id,backend_id,host_epoch,domain_id,policy_revision,cpu_millis,memory_mib,disk_bytes ON capacity_allocations
BEGIN SELECT RAISE(ABORT,'allocation ownership and charge are immutable'); END;
CREATE TRIGGER capacity_allocations_release_requires_proof
BEFORE UPDATE OF state ON capacity_allocations
WHEN NEW.state = 'released' AND NOT EXISTS (
  SELECT 1 FROM workload_placements p WHERE p.id = NEW.placement_id
  AND p.local_absence_proven_at IS NOT NULL AND p.delayed_starts_excluded_at IS NOT NULL
)
BEGIN SELECT RAISE(ABORT,'local absence and delayed start exclusion are required'); END;
CREATE TRIGGER capacity_allocations_insert_release_requires_proof
BEFORE INSERT ON capacity_allocations
WHEN NEW.state = 'released' AND NOT EXISTS (
  SELECT 1 FROM workload_placements p WHERE p.id = NEW.placement_id
  AND p.local_absence_proven_at IS NOT NULL AND p.delayed_starts_excluded_at IS NOT NULL
)
BEGIN SELECT RAISE(ABORT,'local absence and delayed start exclusion are required'); END;
CREATE TRIGGER host_resource_domains_immutable_owner
BEFORE UPDATE OF host_id,parent_domain_id ON host_resource_domains
BEGIN SELECT RAISE(ABORT,'resource domain topology is immutable'); END;
CREATE TRIGGER host_backends_immutable_owner
BEFORE UPDATE OF host_id,domain_id ON host_backends
BEGIN SELECT RAISE(ABORT,'backend ownership is immutable'); END;
CREATE TRIGGER fleet_ci_targets_immutable_identity
BEFORE UPDATE ON fleet_ci_targets
BEGIN SELECT RAISE(ABORT,'CI target identity is immutable'); END;
CREATE TRIGGER host_sessions_immutable_owner
BEFORE UPDATE OF host_id,session_key,os_user_id ON host_sessions
BEGIN SELECT RAISE(ABORT,'session ownership is immutable'); END;
CREATE TRIGGER workload_intents_immutable_identity
BEFORE UPDATE OF id,workload_id,generation,kind,pool_id,profile_id,target_id,agent_run_id,expected_profile_revision ON workload_intents
BEGIN SELECT RAISE(ABORT,'workload intent identity is immutable'); END;
CREATE TRIGGER fleet_operations_immutable_request
BEFORE UPDATE OF id,parent_id,kind,host_id,requested_by_user_id,principal_kind,principal_scope,idempotency_key,request_hash,created_at ON fleet_operations
BEGIN SELECT RAISE(ABORT,'operation request is immutable'); END;
CREATE TRIGGER host_commands_immutable_envelope
BEFORE UPDATE OF id,operation_id,scope,placement_id,host_id,backend_id,host_epoch,generation,kind,protocol_version,request_hash,payload_json,expected_revision,issued_at,deadline_at,idempotency_key ON host_commands
BEGIN SELECT RAISE(ABORT,'command envelope is immutable'); END;
CREATE TRIGGER capacity_allocations_delete_requires_release
BEFORE DELETE ON capacity_allocations WHEN OLD.state != 'released'
BEGIN SELECT RAISE(ABORT,'charged allocation cannot be deleted'); END;
CREATE TRIGGER capacity_allocations_released_is_terminal
BEFORE UPDATE OF state,released_at ON capacity_allocations
WHEN OLD.state = 'released' AND (NEW.state != OLD.state OR NEW.released_at IS NOT OLD.released_at)
BEGIN SELECT RAISE(ABORT,'released allocation is terminal'); END;
CREATE TRIGGER workload_placements_preserve_absence_evidence
BEFORE UPDATE OF local_absence_proven_at,delayed_starts_excluded_at ON workload_placements
WHEN (OLD.local_absence_proven_at IS NOT NULL AND NEW.local_absence_proven_at IS NOT OLD.local_absence_proven_at)
  OR (OLD.delayed_starts_excluded_at IS NOT NULL AND NEW.delayed_starts_excluded_at IS NOT OLD.delayed_starts_excluded_at)
BEGIN SELECT RAISE(ABORT,'absence evidence is immutable'); END;
CREATE TRIGGER workload_placements_match_intent
BEFORE INSERT ON workload_placements WHEN NOT EXISTS (
  SELECT 1 FROM workload_intents i WHERE i.id = NEW.intent_id
  AND i.agent_run_id IS NEW.agent_run_id
  AND ((i.kind = 'ci_runner' AND NEW.agent_run_id IS NULL) OR (i.kind = 'fix_sandbox' AND NEW.runner_id IS NULL))
)
BEGIN SELECT RAISE(ABORT,'placement workload must match intent'); END;
CREATE TRIGGER workload_placements_match_intent_update
BEFORE UPDATE OF agent_run_id,runner_id ON workload_placements WHEN NOT EXISTS (
  SELECT 1 FROM workload_intents i WHERE i.id = NEW.intent_id
  AND i.agent_run_id IS NEW.agent_run_id
  AND ((i.kind = 'ci_runner' AND NEW.agent_run_id IS NULL) OR (i.kind = 'fix_sandbox' AND NEW.runner_id IS NULL))
)
BEGIN SELECT RAISE(ABORT,'placement workload must match intent'); END;
