#!/bin/zsh
set -euo pipefail

env_file="${1:-.env.local}"
token_file="${GRIDOPS_TART_AGENT_TOKEN_FILE:-${HOME}/.gridops/tart-agent/token}"
agent_url="${GRIDOPS_TART_AGENT_URL:-http://host.docker.internal:8790}"
runner_image="${GRIDOPS_TART_RUNNER_IMAGE:-gridops-macos-tahoe-base}"

if [[ ! -f "${env_file}" ]]; then
  print -u2 "Environment file not found: ${env_file}"
  exit 1
fi
if [[ ! -f "${token_file}" ]]; then
  print -u2 "Tart agent token file not found: ${token_file}"
  exit 1
fi
if (( $(wc -c < "${token_file}") < 33 )); then
  print -u2 "The Tart agent token is too short."
  exit 1
fi

file_mode="$(stat -f '%Lp' "${env_file}")"
temporary="$(mktemp "${env_file}.tmp.XXXXXX")"
trap 'rm -f "${temporary}"' EXIT INT TERM

awk -v token_file="${token_file}" -v agent_url="${agent_url}" -v runner_image="${runner_image}" '
  BEGIN {
    getline agent_token < token_file
    close(token_file)
  }
  /^GRIDOPS_TART_RUNNER_IMAGE=/ {
    print "GRIDOPS_TART_RUNNER_IMAGE=" runner_image
    image_seen = 1
    next
  }
  /^GRIDOPS_TART_AGENT_URL=/ {
    print "GRIDOPS_TART_AGENT_URL=" agent_url
    url_seen = 1
    next
  }
  /^GRIDOPS_TART_AGENT_TOKEN=/ {
    print "GRIDOPS_TART_AGENT_TOKEN=" agent_token
    token_seen = 1
    next
  }
  { print }
  END {
    if (!image_seen) print "GRIDOPS_TART_RUNNER_IMAGE=" runner_image
    if (!url_seen) print "GRIDOPS_TART_AGENT_URL=" agent_url
    if (!token_seen) print "GRIDOPS_TART_AGENT_TOKEN=" agent_token
  }
' "${env_file}" > "${temporary}"

chmod "${file_mode}" "${temporary}"
mv "${temporary}" "${env_file}"
trap - EXIT INT TERM
print "Configured the Tart manager connection in ${env_file}; the token was not printed."
