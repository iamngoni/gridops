-- Discovery is evidence from one authenticated inventory session. Existing
-- backend capability JSON has no provenance and remains unverified until reprobed.
ALTER TABLE host_backends ADD COLUMN probe_epoch INTEGER CHECK (probe_epoch > 0);
ALTER TABLE host_backends ADD COLUMN probe_session_id TEXT;
ALTER TABLE host_backends ADD COLUMN probe_boot_id TEXT;
ALTER TABLE host_backends ADD COLUMN probe_incarnation TEXT;
