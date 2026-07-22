#!/bin/zsh
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  print -u2 "gridops-tart-agent requires an Apple Silicon macOS host."
  exit 1
fi

script_dir="${0:A:h}"
repo_root="${script_dir:h}"
agent_home="${GRIDOPS_TART_AGENT_HOME:-${HOME}/.gridops/tart-agent}"
install_dir="${GRIDOPS_TART_AGENT_INSTALL_DIR:-${agent_home}/bin}"
binary="${install_dir}/gridops-tart-agent"
token_file="${agent_home}/token"
plist_source="${repo_root}/deploy/launchd/dev.gridops.tart-agent.plist"
plist_target="${HOME}/Library/LaunchAgents/dev.gridops.tart-agent.plist"
tart_binary="${GRIDOPS_TART_BINARY:-$(command -v tart || true)}"
bind="${GRIDOPS_TART_AGENT_BIND:-127.0.0.1:8790}"
network_mode="${GRIDOPS_TART_NETWORK_MODE:-softnet}"
runner_root="${GRIDOPS_TART_RUNNER_ROOT:-/Users/admin/actions-runner}"
cpu_budget="${GRIDOPS_TART_CPU_BUDGET:-4}"
memory_budget="${GRIDOPS_TART_MEMORY_BUDGET_MB:-6144}"
max_runners="${GRIDOPS_TART_MAX_RUNNERS:-1}"
min_free_disk="${GRIDOPS_TART_MIN_FREE_DISK_MB:-40960}"
agent_token="${GRIDOPS_TART_AGENT_TOKEN:-}"

if [[ -z "${tart_binary}" || ! -x "${tart_binary}" ]]; then
  print -u2 "Tart is not installed. Install it with: brew install openai/tools/tart"
  exit 1
fi
if [[ "${network_mode}" != "nat" && "${network_mode}" != "softnet" ]]; then
  print -u2 "GRIDOPS_TART_NETWORK_MODE must be nat or softnet."
  exit 1
fi
if [[ "${network_mode}" == "softnet" ]] && ! command -v softnet >/dev/null 2>&1; then
  print -u2 "Softnet mode was requested but softnet is not installed in PATH."
  print -u2 "Install and privilege openai/softnet, or explicitly use GRIDOPS_TART_NETWORK_MODE=nat."
  exit 1
fi
if [[ "${network_mode}" == "softnet" ]]; then
  softnet_binary="$(realpath "$(command -v softnet)")"
  if [[ "$(stat -f '%u' "${softnet_binary}")" != "0" || ! -u "${softnet_binary}" ]]; then
    print -u2 "Softnet is installed but is not owned by root with its SUID bit set."
    print -u2 "Run: sudo chown root:wheel ${softnet_binary}"
    print -u2 "Then: sudo chmod u+s ${softnet_binary}"
    exit 1
  fi
fi
if [[ ! "${bind}" =~ '^[A-Za-z0-9.:-]+$' ]]; then
  print -u2 "GRIDOPS_TART_AGENT_BIND contains unsupported characters."
  exit 1
fi
if (( ${#agent_token} < 32 )); then
  print -u2 "Set GRIDOPS_TART_AGENT_TOKEN to an independently generated token of at least 32 characters."
  exit 1
fi

mkdir -p "${agent_home}/logs" "${install_dir}" "${HOME}/Library/LaunchAgents"
chmod 700 "${agent_home}"

print "Building the native Tart agent…"
(cd "${repo_root}" && cargo build --release --locked -p gridops-tart-agent)
/usr/bin/install -m 0755 "${repo_root}/target/release/gridops-tart-agent" "${binary}"
umask 077
print -r -- "${agent_token}" > "${token_file}"
chmod 600 "${token_file}"

escape_sed() {
  print -nr -- "$1" | sed 's/[&|\\]/\\&/g'
}

sed \
  -e "s|__BINARY__|$(escape_sed "${binary}")|g" \
  -e "s|__TOKEN_FILE__|$(escape_sed "${token_file}")|g" \
  -e "s|__AGENT_HOME__|$(escape_sed "${agent_home}")|g" \
  -e "s|__BIND__|$(escape_sed "${bind}")|g" \
  -e "s|__TART_BINARY__|$(escape_sed "${tart_binary}")|g" \
  -e "s|__RUNNER_ROOT__|$(escape_sed "${runner_root}")|g" \
  -e "s|__NETWORK_MODE__|$(escape_sed "${network_mode}")|g" \
  -e "s|__CPU_BUDGET__|$(escape_sed "${cpu_budget}")|g" \
  -e "s|__MEMORY_BUDGET__|$(escape_sed "${memory_budget}")|g" \
  -e "s|__MAX_RUNNERS__|$(escape_sed "${max_runners}")|g" \
  -e "s|__MIN_FREE_DISK__|$(escape_sed "${min_free_disk}")|g" \
  -e "s|__STDOUT_LOG__|$(escape_sed "${agent_home}/logs/agent.log")|g" \
  -e "s|__STDERR_LOG__|$(escape_sed "${agent_home}/logs/agent.error.log")|g" \
  "${plist_source}" > "${plist_target}"
chmod 600 "${plist_target}"
plutil -lint "${plist_target}" >/dev/null

launchctl bootout "gui/${UID}/dev.gridops.tart-agent" 2>/dev/null || true
launchctl bootstrap "gui/${UID}" "${plist_target}"
launchctl kickstart -k "gui/${UID}/dev.gridops.tart-agent"

print "Installed gridops-tart-agent at ${binary}."
print "The bearer token is stored with mode 0600 at ${token_file}."
