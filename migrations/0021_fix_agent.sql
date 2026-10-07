-- Fix agent: AI provider connections and agent runs.
--
-- A connection's credential (API key or subscription tokens) is sealed in
-- runtime_secrets under `credential_key`; this table holds only what the
-- settings page shows. Providers are validated in code rather than with a
-- CHECK, so adding one later does not mean rebuilding the table.

CREATE TABLE ai_connections (
  id TEXT PRIMARY KEY NOT NULL,
  provider TEXT NOT NULL,
  auth_method TEXT NOT NULL CHECK (auth_method IN ('subscription_oauth','api_key')),
  credential_key TEXT NOT NULL,
  account_label TEXT,
  fingerprint TEXT,
  status TEXT NOT NULL DEFAULT 'ready' CHECK (status IN ('ready','needs_reconnect','error')),
  status_detail TEXT,
  created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX ai_connections_provider_unique ON ai_connections (provider);
CREATE UNIQUE INDEX ai_connections_credential_key_unique ON ai_connections (credential_key);

-- A pending "Sign in with ChatGPT/Claude": the PKCE verifier waits here,
-- sealed, until the person pastes back what the provider showed them.
CREATE TABLE ai_oauth_attempts (
  id TEXT PRIMARY KEY NOT NULL,
  provider TEXT NOT NULL,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  state TEXT NOT NULL,
  verifier_sealed TEXT NOT NULL,
  callback_port INTEGER,
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE INDEX ai_oauth_attempts_expiry_idx ON ai_oauth_attempts (expires_at);

CREATE TABLE agent_runs (
  id TEXT PRIMARY KEY NOT NULL,
  job_id INTEGER NOT NULL REFERENCES workflow_jobs(id) ON DELETE CASCADE,
  run_id INTEGER NOT NULL,
  repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
  trigger TEXT NOT NULL CHECK (trigger IN ('manual','automatic')),
  status TEXT NOT NULL DEFAULT 'queued'
    CHECK (status IN ('queued','running','succeeded','failed','cancelled')),
  stage TEXT,
  outcome TEXT CHECK (outcome IN ('pull_request','diagnosis')),
  title TEXT,
  summary TEXT,
  details TEXT,
  verification TEXT,
  pull_request_url TEXT,
  pull_request_number INTEGER,
  branch TEXT,
  error TEXT,
  connection_id TEXT REFERENCES ai_connections(id) ON DELETE SET NULL,
  provider TEXT,
  model TEXT,
  reasoning_effort TEXT,
  sandbox_id TEXT,
  turns INTEGER NOT NULL DEFAULT 0,
  tool_calls INTEGER NOT NULL DEFAULT 0,
  cancel_requested INTEGER NOT NULL DEFAULT 0,
  -- When a queued run waiting for host capacity may try again.
  retry_at INTEGER,
  requested_by TEXT REFERENCES users(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  started_at INTEGER,
  completed_at INTEGER,
  updated_at INTEGER NOT NULL
);

CREATE INDEX agent_runs_job_idx ON agent_runs (job_id, created_at);
CREATE INDEX agent_runs_status_idx ON agent_runs (status, created_at);
CREATE INDEX agent_runs_run_idx ON agent_runs (run_id);
-- One active run per job, enforced by the database as well as the API.
CREATE UNIQUE INDEX agent_runs_active_job_unique ON agent_runs (job_id)
  WHERE status IN ('queued','running');

CREATE TABLE agent_run_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_run_id TEXT NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  detail TEXT,
  created_at INTEGER NOT NULL
);

CREATE INDEX agent_run_events_run_idx ON agent_run_events (agent_run_id, id);
