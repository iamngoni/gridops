#!/bin/zsh
set -euo pipefail

source_image="${1:-ghcr.io/cirruslabs/macos-tahoe-base:latest}"
base_name="${2:-gridops-macos-tahoe-base}"
parallel_downloads="${GRIDOPS_TART_DOWNLOAD_PARALLELISM:-4}"
connections_per_download="${GRIDOPS_TART_DOWNLOAD_CONNECTIONS:-8}"
tart_binary="${GRIDOPS_TART_BINARY:-$(command -v tart || true)}"
cache_root="${GRIDOPS_TART_IMAGE_CACHE_DIR:-${HOME}/Library/Caches/GridOps/tart-images/${base_name}}"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  print -u2 "Tart images can only be assembled on Apple Silicon macOS."
  exit 1
fi
if [[ "${source_image}" != ghcr.io/* ]]; then
  print -u2 "The resumable pull currently supports ghcr.io images only."
  exit 1
fi
if [[ -z "${tart_binary}" || ! -x "${tart_binary}" ]]; then
  print -u2 "Tart is not installed. Install it with: brew install openai/tools/tart"
  exit 1
fi
for dependency in aria2c compression_tool gh jq shasum; do
  if ! command -v "${dependency}" >/dev/null 2>&1; then
    print -u2 "Missing required command: ${dependency}"
    if [[ "${dependency}" == "aria2c" ]]; then
      print -u2 "Install the resumable downloader with: brew install aria2"
    fi
    exit 1
  fi
done
if [[ ! "${base_name}" =~ '^[A-Za-z0-9._-]+$' ]]; then
  print -u2 "The local Tart base name contains unsupported characters."
  exit 1
fi
if [[ ! "${parallel_downloads}" =~ '^[1-9][0-9]*$' ]] || (( parallel_downloads > 8 )); then
  print -u2 "GRIDOPS_TART_DOWNLOAD_PARALLELISM must be between 1 and 8."
  exit 1
fi
if [[ ! "${connections_per_download}" =~ '^[1-9][0-9]*$' ]] || (( connections_per_download > 16 )); then
  print -u2 "GRIDOPS_TART_DOWNLOAD_CONNECTIONS must be between 1 and 16."
  exit 1
fi

local_vm_dir="${HOME}/.tart/vms/${base_name}"
if [[ -e "${local_vm_dir}" ]]; then
  print -u2 "A local Tart VM named ${base_name} already exists."
  exit 1
fi

image_reference="${source_image#ghcr.io/}"
if [[ "${image_reference}" == *@sha256:* ]]; then
  repository="${image_reference%@sha256:*}"
  reference="sha256:${image_reference##*@sha256:}"
elif [[ "${image_reference##*/}" == *:* ]]; then
  repository="${image_reference%:*}"
  reference="${image_reference##*:}"
else
  repository="${image_reference}"
  reference="latest"
fi

mkdir -p "${cache_root}"
staging_dir="${cache_root}/vm"
manifest_file="${cache_root}/manifest.json"
completed_file="${cache_root}/completed-layers"
mkdir -p "${staging_dir}"
touch "${completed_file}"

github_user="$(gh api user --jq .login)"
github_token="$(gh auth token)"

registry_token() {
  curl --fail --silent --show-error \
    --user "${github_user}:${github_token}" \
    "https://ghcr.io/token?scope=repository:${repository}:pull" |
    jq -er '.token'
}

print "Resolving ${source_image}…"
token="$(registry_token)"
curl --fail --silent --show-error --location --retry 8 --retry-all-errors \
  -H "Authorization: Bearer ${token}" \
  -H 'Accept: application/vnd.oci.image.manifest.v1+json' \
  "https://ghcr.io/v2/${repository}/manifests/${reference}" \
  -o "${manifest_file}"

if [[ "$(jq -r '.mediaType // empty' "${manifest_file}")" != "application/vnd.oci.image.manifest.v1+json" ]]; then
  print -u2 "The resolved image is not a Tart OCI manifest."
  exit 1
fi

typeset -a raw_layers disk_layers
raw_layers=("${(@f)$(jq -r '
  .layers[]
  | select(.mediaType == "application/vnd.cirruslabs.tart.disk.v2")
  | [
      .digest,
      (.size | tostring),
      .annotations["org.cirruslabs.tart.uncompressed-size"],
      .annotations["org.cirruslabs.tart.uncompressed-content-digest"]
    ]
  | @tsv
' "${manifest_file}")}")
if (( ${#raw_layers[@]} == 0 )); then
  print -u2 "The image contains no Tart disk layers."
  exit 1
fi

offset=0
index=0
for layer in "${raw_layers[@]}"; do
  IFS=$'\t' read -r digest compressed_size uncompressed_size uncompressed_digest <<< "${layer}"
  if [[ -z "${digest}" || -z "${compressed_size}" || -z "${uncompressed_size}" || -z "${uncompressed_digest}" ]]; then
    print -u2 "A Tart disk layer is missing required size or digest metadata."
    exit 1
  fi
  disk_layers+=("${index}"$'\t'"${offset}"$'\t'"${digest}"$'\t'"${compressed_size}"$'\t'"${uncompressed_size}"$'\t'"${uncompressed_digest}")
  offset=$((offset + uncompressed_size))
  index=$((index + 1))
done
disk_size="${offset}"

disk_file="${staging_dir}/disk.img"
if [[ -e "${disk_file}" ]]; then
  if [[ "$(stat -f '%z' "${disk_file}")" != "${disk_size}" ]]; then
    print -u2 "The partial disk has the wrong size. Move ${cache_root} aside before retrying."
    exit 1
  fi
else
  mkfile -n "${disk_size}" "${disk_file}"
fi

download_batch() {
  local -a batch=("$@")
  local input_file
  input_file="$(mktemp "${cache_root}/aria2-input.XXXXXX")"
  token="$(registry_token)"

  for layer in "${batch[@]}"; do
    IFS=$'\t' read -r layer_index layer_offset digest compressed_size uncompressed_size uncompressed_digest <<< "${layer}"
    blob_name="${digest#sha256:}.blob"
    print -r -- "https://ghcr.io/v2/${repository}/blobs/${digest}" >> "${input_file}"
    print -r -- "  dir=${cache_root}" >> "${input_file}"
    print -r -- "  out=${blob_name}" >> "${input_file}"
  done

  print "Downloading ${#batch[@]} verified disk layer(s)…"
  aria2c \
    --input-file="${input_file}" \
    --continue=true \
    --max-concurrent-downloads="${#batch[@]}" \
    --max-connection-per-server="${connections_per_download}" \
    --split="${connections_per_download}" \
    --min-split-size=1M \
    --file-allocation=none \
    --max-tries=0 \
    --retry-wait=1 \
    --timeout=30 \
    --connect-timeout=15 \
    --summary-interval=15 \
    --console-log-level=warn \
    --header="Authorization: Bearer ${token}"

  for layer in "${batch[@]}"; do
    IFS=$'\t' read -r layer_index layer_offset digest compressed_size uncompressed_size uncompressed_digest <<< "${layer}"
    blob_file="${cache_root}/${digest#sha256:}.blob"
    raw_file="$(mktemp "${cache_root}/raw-layer.XXXXXX")"

    if [[ "$(stat -f '%z' "${blob_file}")" != "${compressed_size}" ]]; then
      print -u2 "Downloaded layer ${layer_index} has the wrong size."
      exit 1
    fi
    if [[ "$(shasum -a 256 "${blob_file}" | awk '{print $1}')" != "${digest#sha256:}" ]]; then
      print -u2 "Downloaded layer ${layer_index} failed its compressed SHA-256 check."
      exit 1
    fi

    compression_tool -decode -a lz4 -i "${blob_file}" -o "${raw_file}"
    if [[ "$(stat -f '%z' "${raw_file}")" != "${uncompressed_size}" ]]; then
      print -u2 "Decompressed layer ${layer_index} has the wrong size."
      exit 1
    fi
    if [[ "$(shasum -a 256 "${raw_file}" | awk '{print $1}')" != "${uncompressed_digest#sha256:}" ]]; then
      print -u2 "Decompressed layer ${layer_index} failed its SHA-256 check."
      exit 1
    fi
    if (( layer_offset % 4194304 != 0 )); then
      print -u2 "Layer ${layer_index} is not aligned to a 4 MiB disk boundary."
      exit 1
    fi

    dd if="${raw_file}" of="${disk_file}" bs=4194304 seek=$((layer_offset / 4194304)) conv=notrunc status=none
    print -r -- "${digest}" >> "${completed_file}"
    command rm -- "${raw_file}" "${blob_file}"
    [[ ! -e "${blob_file}.aria2" ]] || command rm -- "${blob_file}.aria2"
    print "Assembled disk layer $((layer_index + 1))/${#disk_layers[@]}."
  done

  command rm -- "${input_file}"
}

typeset -a batch
for layer in "${disk_layers[@]}"; do
  IFS=$'\t' read -r layer_index layer_offset digest compressed_size uncompressed_size uncompressed_digest <<< "${layer}"
  if grep -Fqx -- "${digest}" "${completed_file}"; then
    continue
  fi
  batch+=("${layer}")
  if (( ${#batch[@]} == parallel_downloads )); then
    download_batch "${batch[@]}"
    batch=()
  fi
done
if (( ${#batch[@]} > 0 )); then
  download_batch "${batch[@]}"
fi

download_auxiliary_layer() {
  local media_type="$1"
  local destination="$2"
  local descriptor digest expected_size blob_file
  descriptor="$(jq -cer --arg media_type "${media_type}" '.layers[] | select(.mediaType == $media_type)' "${manifest_file}")"
  digest="$(print -r -- "${descriptor}" | jq -r '.digest')"
  expected_size="$(print -r -- "${descriptor}" | jq -r '.size')"
  blob_file="${cache_root}/${digest#sha256:}.blob"
  token="$(registry_token)"

  aria2c \
    --continue=true \
    --max-connection-per-server="${connections_per_download}" \
    --split="${connections_per_download}" \
    --min-split-size=1M \
    --file-allocation=none \
    --max-tries=0 \
    --retry-wait=1 \
    --timeout=30 \
    --connect-timeout=15 \
    --console-log-level=warn \
    --header="Authorization: Bearer ${token}" \
    --dir="${cache_root}" \
    --out="${digest#sha256:}.blob" \
    "https://ghcr.io/v2/${repository}/blobs/${digest}"

  if [[ "$(stat -f '%z' "${blob_file}")" != "${expected_size}" ]] || \
     [[ "$(shasum -a 256 "${blob_file}" | awk '{print $1}')" != "${digest#sha256:}" ]]; then
    print -u2 "The ${media_type} layer failed verification."
    exit 1
  fi
  cp "${blob_file}" "${destination}"
  command rm -- "${blob_file}"
  [[ ! -e "${blob_file}.aria2" ]] || command rm -- "${blob_file}.aria2"
}

sync
download_auxiliary_layer "application/vnd.cirruslabs.tart.config.v1" "${staging_dir}/config.json"
download_auxiliary_layer "application/vnd.cirruslabs.tart.nvram.v1" "${staging_dir}/nvram.bin"
cp "${manifest_file}" "${staging_dir}/manifest.json"

if [[ ! -s "${staging_dir}/config.json" || ! -s "${staging_dir}/nvram.bin" ]]; then
  print -u2 "The assembled VM is missing required files."
  exit 1
fi

mkdir -p "${HOME}/.tart/vms"
mv "${staging_dir}" "${local_vm_dir}"
print "Assembled verified local Tart VM ${base_name}."
