#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  cat >&2 <<'USAGE'
Usage: prepare-tart-bitbucket-runner-image.sh <source-image> <target-image> <runner-archive-url>

The archive URL comes from Bitbucket Cloud's macOS runner setup dialog. It is
not a credential and should point to the Atlassian runner tar.gz archive.
USAGE
  exit 64
fi

source_image=$1
target_image=$2
archive_url=$3
tart_binary=${GRIDOPS_TART_BINARY:-/opt/homebrew/bin/tart}
runner_root=${GRIDOPS_TART_BITBUCKET_RUNNER_ROOT:-/Users/admin/atlassian-bitbucket-pipelines-runner}

if [[ ! -x "$tart_binary" ]]; then
  printf 'Tart is unavailable at %s\n' "$tart_binary" >&2
  exit 69
fi
if [[ $runner_root != /* || $runner_root == *$'\n'* || $runner_root == *$'\r'* ]]; then
  printf 'GRIDOPS_TART_BITBUCKET_RUNNER_ROOT must be an absolute single-line guest path.\n' >&2
  exit 64
fi
if [[ $archive_url != https://* ]]; then
  printf 'The Bitbucket runner archive URL must use HTTPS.\n' >&2
  exit 64
fi

"$tart_binary" clone "$source_image" "$target_image"
cleanup() {
  "$tart_binary" stop "$target_image" >/dev/null 2>&1 || true
}
trap cleanup EXIT
"$tart_binary" run --no-graphics "$target_image" >/dev/null &

for _ in $(seq 1 90); do
  if "$tart_binary" exec "$target_image" /usr/bin/true >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
"$tart_binary" exec "$target_image" /usr/bin/true >/dev/null

quoted_root=$(printf '%q' "$runner_root")
quoted_url=$(printf '%q' "$archive_url")
"$tart_binary" exec "$target_image" /bin/bash -lc "
set -euo pipefail
command -v java >/dev/null
java -version >&2
command -v git >/dev/null
rm -rf ${quoted_root}
mkdir -p ${quoted_root}
curl --fail --location --proto '=https' --tlsv1.2 ${quoted_url} --output /tmp/gridops-bitbucket-runner.tar.gz
tar -xzf /tmp/gridops-bitbucket-runner.tar.gz -C ${quoted_root}
rm -f /tmp/gridops-bitbucket-runner.tar.gz
test -x ${quoted_root}/bin/start.sh
"

printf 'Prepared %s with the Bitbucket runner at %s.\n' "$target_image" "$runner_root"
