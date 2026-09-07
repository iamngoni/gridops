-- Native macOS runner runtime.
--
-- The Tart agent gains a second execution strategy: instead of cloning a
-- copy-on-write VM per job, it can run the GitHub Actions runner directly on
-- the macOS host. Native execution is what makes TCC-dependent jobs
-- (accessibility, screen recording) possible, because those grants are bound to
-- a real logged-in user session that a disposable VM clone cannot carry.
--
-- `provider` deliberately stays 'tart' for both strategies. It identifies the
-- macOS host agent, not the virtualisation technology, so label matching in
-- gridops-core continues to resolve macOS/ARM64 for either runtime. Widening
-- the existing CHECK (provider IN ('docker','tart')) would require rebuilding
-- `runners`, and because foreign keys are enabled (gridops-core/src/db.rs) and
-- `runner_events.runner_id` cascades on delete, DROP TABLE would take the
-- entire runner event history with it. These additive columns avoid that
-- entirely.

ALTER TABLE runner_pools ADD COLUMN macos_runtime TEXT NOT NULL DEFAULT 'vm'
  CHECK (macos_runtime IN ('vm','native'));

ALTER TABLE runners ADD COLUMN runtime TEXT NOT NULL DEFAULT 'vm'
  CHECK (runtime IN ('vm','native'));
