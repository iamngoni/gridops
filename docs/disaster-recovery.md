# Disaster recovery

GridOps keeps operational state, encrypted GitHub credentials, audit history,
and retained runner metadata in SQLite. A database backup is only useful when
its matching `GRIDOPS_ENCRYPTION_KEY` is retained separately.

## Create and verify a backup

Download a backup from **Settings**, store it in an encrypted location, and
verify it before an incident:

```sh
sqlite3 gridops-backup-20260728.sqlite 'PRAGMA integrity_check;'
```

The expected output is `ok`. Run this check after each scheduled backup and
periodically restore it into a disposable GridOps environment.

## Restore into a replacement instance

Do not overwrite a live database. Stop the API, reconciler, and manager first,
then restore to a new path using the guarded helper:

```sh
./scripts/restore-database-backup.sh \
  ./gridops-backup-20260728.sqlite \
  ./data/recovered-gridops.sqlite
```

The helper refuses to overwrite a database, copies the backup to the target
filesystem, runs SQLite's integrity check, and writes the restored database
with mode `0600`.

Set `GRIDOPS_DATABASE_PATH` to the new path and provide the exact
`GRIDOPS_ENCRYPTION_KEY` that protected the original instance. Start the API,
manager, and reconciler, then verify:

```sh
curl --fail http://127.0.0.1:8080/api/health
```

Sign in, confirm installations and runner pools are visible, and check the
audit log before returning traffic to the restored instance. Rotate the session
secret and any manager or Tart-agent bearer tokens if the recovery event may
have exposed the previous deployment configuration.

## Recovery drill

At least quarterly, restore the newest backup into an isolated test directory.
Confirm integrity, sign-in, settings visibility, pool configuration, and a
read-only audit-log view. Do not attach the recovery drill to production
Docker, Tart, or GitHub runner credentials.
