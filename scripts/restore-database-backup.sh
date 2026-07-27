#!/bin/sh

# Restores a GridOps SQLite backup into a new database path after verifying its
# integrity. Stop GridOps first, then atomically point GRIDOPS_DATABASE_PATH at
# the restored file and start the services again.
set -eu

if [ "$#" -ne 2 ]; then
  echo "Usage: $0 <backup.sqlite> <new-database.sqlite>" >&2
  exit 64
fi

backup=$1
destination=$2

if [ ! -r "$backup" ]; then
  echo "Backup is not readable: $backup" >&2
  exit 66
fi

if [ -e "$destination" ]; then
  echo "Refusing to overwrite existing database: $destination" >&2
  exit 73
fi

if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "sqlite3 is required to verify the backup before restoring it." >&2
  exit 69
fi

destination_dir=$(dirname "$destination")
mkdir -p "$destination_dir"
temporary=$(mktemp "$destination_dir/.gridops-restore.XXXXXX")

cleanup() {
  rm -f "$temporary"
}
trap cleanup EXIT HUP INT TERM

cp "$backup" "$temporary"
integrity=$(sqlite3 "$temporary" "PRAGMA integrity_check;")
if [ "$integrity" != "ok" ]; then
  echo "Backup integrity check failed: $integrity" >&2
  exit 65
fi

chmod 600 "$temporary"
mv "$temporary" "$destination"
trap - EXIT HUP INT TERM

echo "Verified and restored $backup to $destination"
echo "Keep the GRIDOPS_ENCRYPTION_KEY used to create the backup; encrypted GitHub credentials require it."
