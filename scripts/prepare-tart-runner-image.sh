#!/bin/zsh
set -euo pipefail

source_image="${1:-ghcr.io/cirruslabs/macos-tahoe-base:latest}"
base_name="${2:-gridops-macos-tahoe-base}"
tart_binary="${GRIDOPS_TART_BINARY:-$(command -v tart || true)}"
runner_root="${GRIDOPS_TART_RUNNER_ROOT:-/Users/admin/actions-runner}"
pull_concurrency="${GRIDOPS_TART_PULL_CONCURRENCY:-8}"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  print -u2 "A Tart macOS runner image must be prepared on Apple Silicon macOS."
  exit 1
fi
if [[ -z "${tart_binary}" || ! -x "${tart_binary}" ]]; then
  print -u2 "Tart is not installed. Install it with: brew install openai/tools/tart"
  exit 1
fi
if ! command -v gh >/dev/null 2>&1 || ! command -v jq >/dev/null 2>&1; then
  print -u2 "Preparing the image requires gh and jq on the host."
  exit 1
fi
if [[ ! "${base_name}" =~ '^[A-Za-z0-9._-]+$' ]]; then
  print -u2 "The local Tart base name contains unsupported characters."
  exit 1
fi
if [[ ! "${pull_concurrency}" =~ '^[1-9][0-9]*$' ]] || (( pull_concurrency > 32 )); then
  print -u2 "GRIDOPS_TART_PULL_CONCURRENCY must be between 1 and 32."
  exit 1
fi

vm_state="$(${tart_binary} list --source local --format json | jq -r --arg name "${base_name}" '.[] | select(.Name == $name) | .State' | head -n 1)"
if [[ -z "${vm_state}" ]]; then
  print "Cloning ${source_image} to ${base_name}; the first pull is approximately 25 GB…"
  "${tart_binary}" clone "${source_image}" "${base_name}" --concurrency "${pull_concurrency}"
elif [[ "${vm_state}" != "stopped" ]]; then
  print -u2 "${base_name} is ${vm_state}; stop it before preparing the base image."
  exit 1
else
  print "Updating the existing stopped base image ${base_name}."
fi

release_json="$(gh api repos/actions/runner/releases/latest)"
runner_version="$(print -r -- "${release_json}" | jq -r '.tag_name | ltrimstr("v")')"
asset_name="actions-runner-osx-arm64-${runner_version}.tar.gz"
runner_url="$(print -r -- "${release_json}" | jq -r --arg name "${asset_name}" '.assets[] | select(.name == $name) | .browser_download_url')"
runner_digest="$(print -r -- "${release_json}" | jq -r --arg name "${asset_name}" '.assets[] | select(.name == $name) | .digest // empty' | sed 's/^sha256://')"
if [[ -z "${runner_url}" || -z "${runner_digest}" ]]; then
  print -u2 "GitHub did not publish a download URL and SHA-256 digest for ${asset_name}."
  exit 1
fi

vm_pid=""
cleanup() {
  "${tart_binary}" stop "${base_name}" >/dev/null 2>&1 || true
  if [[ -n "${vm_pid}" ]]; then
    wait "${vm_pid}" 2>/dev/null || true
    vm_pid=""
  fi
}
trap cleanup EXIT INT TERM

"${tart_binary}" set "${base_name}" --cpu 4 --memory 8192
"${tart_binary}" run --no-graphics --no-audio --no-clipboard "${base_name}" >/tmp/gridops-tart-image-preparation.log 2>&1 &
vm_pid=$!

print "Waiting for the Tart guest agent…"
deadline=$((SECONDS + 240))
until "${tart_binary}" exec "${base_name}" /usr/bin/true >/dev/null 2>&1; do
  if (( SECONDS >= deadline )); then
    print -u2 "The Tart guest agent did not become ready. See /tmp/gridops-tart-image-preparation.log."
    exit 1
  fi
  sleep 2
done

print "Installing GitHub Actions runner ${runner_version} in the base VM…"
"${tart_binary}" exec -i "${base_name}" /bin/bash -s -- "${runner_root}" "${runner_url}" "${runner_digest}" <<'GUEST_SCRIPT'
set -euo pipefail
runner_root="$1"
runner_url="$2"
runner_digest="$3"
archive="/tmp/$(basename "$runner_url")"
rm -rf "$runner_root"
mkdir -p "$runner_root"
curl --fail --location --retry 4 --output "$archive" "$runner_url"
printf '%s  %s\n' "$runner_digest" "$archive" | shasum -a 256 --check
tar -xzf "$archive" -C "$runner_root"
rm -f "$archive"
test -x "$runner_root/run.sh"
sync
GUEST_SCRIPT

cleanup
trap - EXIT INT TERM
print "Prepared stopped Tart base VM ${base_name}."
