#!/usr/bin/env sh
set -eu

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

usage() {
  printf '%s\n' 'usage: linux-kvm-preflight.sh --repository-root PATH --candidate-sha SHA --state-root PATH --base-image PATH --owned-namespace NAME --writer-exclusivity-evidence ID --receipt PATH [--qemu-system PATH --qemu-img PATH | --fixture-evidence PATH]'
}

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

sha256_text() {
  printf '%s' "$1" | sha256sum | awk '{print $1}'
}

sha256_file() {
  hash_output=$(sha256sum -- "$1") || fail "$2: SHA-256 could not be computed"
  hash_value=${hash_output%% *}
  require_sha256 "$hash_value" "$2"
  printf '%s' "$hash_value"
}

publish_receipt_noreplace() {
  python3 -c '
import ctypes
import os
import sys

libc = ctypes.CDLL(None, use_errno=True)
renameat2 = libc.renameat2
renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
renameat2.restype = ctypes.c_int
if renameat2(-100, os.fsencode(sys.argv[1]), -100, os.fsencode(sys.argv[2]), 1) != 0:
    error = ctypes.get_errno()
    raise OSError(error, os.strerror(error))
' "$1" "$2"
}

require_safe_text() {
  value=$1
  label=$2
  [ -n "$value" ] || fail "$label: value is empty"
  original_bytes=$(printf '%s' "$value" | wc -c)
  safe_bytes=$(printf '%s' "$value" | LC_ALL=C tr -d '\001-\037\177' | wc -c)
  if [ "$original_bytes" -ne "$safe_bytes" ]; then
    fail "$label: control characters are not allowed"
  fi
}

require_sha256() {
  [ "${#1}" -eq 64 ] || fail "$2: expected one lowercase SHA-256 value"
  case "$1" in *[!0-9a-f]*) fail "$2: expected one lowercase SHA-256 value" ;; esac
}

canonical_directory() {
  [ -d "$1" ] && [ ! -L "$1" ] || fail "$2: expected an ordinary directory"
  case "$1" in /*) ;; *) fail "$2: path must be absolute and canonical" ;; esac
  canonical=$(readlink -f -- "$1") || fail "$2: path could not be canonicalized"
  [ "${1%/}" = "$canonical" ] || fail "$2: symlinked or non-canonical path was rejected"
  printf '%s\n' "$canonical"
}

canonical_file() {
  [ -f "$1" ] && [ ! -L "$1" ] || fail "$2: expected an ordinary file"
  case "$1" in /*) ;; *) fail "$2: path must be absolute and canonical" ;; esac
  canonical=$(readlink -f -- "$1") || fail "$2: path could not be canonicalized"
  [ "$1" = "$canonical" ] || fail "$2: symlinked or non-canonical path was rejected"
  printf '%s\n' "$canonical"
}

fixture_value() {
  key=$1
  count=$(grep -c "^${key}=" "$fixture_path" || true)
  [ "$count" -eq 1 ] || fail "preflight.fixture_invalid: $key must occur exactly once"
  sed -n "s/^${key}=//p" "$fixture_path"
}

collect_runtime_rows() {
  if [ ! -e "$runtime_root" ] && [ ! -L "$runtime_root" ]; then
    return 0
  fi
  runtime_path=$(canonical_directory "$runtime_root" 'preflight.runtime_invalid')
  [ "$(stat -Lc '%u' -- "$runtime_path")" = "$effective_uid" ] || fail 'preflight.runtime_invalid: runtime root is not owned by the effective identity'
  [ "$(stat -Lc '%a' -- "$runtime_path")" = 700 ] || fail 'preflight.runtime_invalid: runtime root mode must be 0700'
  unsorted_rows=$(find "$runtime_path" -xdev -printf '%P|%y|%D:%i|%u|%m\n') || fail 'preflight.runtime_invalid: runtime prestate could not be enumerated'
  rows=$(printf '%s' "$unsorted_rows" | LC_ALL=C sort) || fail 'preflight.runtime_invalid: runtime prestate could not be sorted'
  [ "$(printf '%s' "$rows" | wc -c)" -le 65536 ] || fail 'preflight.runtime_invalid: runtime prestate exceeded 65536 bytes'
  [ "$(printf '%s\n' "$rows" | awk 'NF {count++} END {print count+0}')" -le 4096 ] || fail 'preflight.runtime_invalid: runtime prestate exceeded 4096 entries'
  if printf '%s\n' "$rows" | grep -F '|l|' >/dev/null; then
    fail 'preflight.runtime_invalid: runtime prestate contains a symlink'
  fi
  printf '%s' "$rows"
}

cleanup_temporary_paths() {
  [ -z "${probe_temp:-}" ] || [ ! -e "$probe_temp" ] || rm -f -- "$probe_temp"
  [ -z "${receipt_temp:-}" ] || [ ! -e "$receipt_temp" ] || rm -f -- "$receipt_temp"
}

run_bounded_probe() {
  label=$1
  shift
  probe_temp=$(mktemp "$receipt_parent/.vmcell-linux-probe.XXXXXX") || fail "$label: probe file could not be created"
  if ! (ulimit -f 128; timeout -k 1s 10s "$@" > "$probe_temp" 2>/dev/null); then
    fail "$label: bounded probe failed or timed out"
  fi
  [ "$(stat -Lc '%s' -- "$probe_temp")" -le 65536 ] || fail "$label: output exceeded 65536 bytes"
  probe_output=$(cat -- "$probe_temp")
  rm -f -- "$probe_temp"
  probe_temp=
  bounded_probe_output=$probe_output
}

repository_root=
candidate_sha=
state_root=
base_image=
owned_namespace=
writer_evidence=
receipt_path=
qemu_system=
qemu_img=
fixture_evidence=

while [ "$#" -gt 0 ]; do
  option=$1
  shift
  case "$option" in
    --repository-root) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing repository root'; repository_root=$1; shift ;;
    --candidate-sha) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing candidate SHA'; candidate_sha=$1; shift ;;
    --state-root) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing state root'; state_root=$1; shift ;;
    --base-image) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing base image'; base_image=$1; shift ;;
    --owned-namespace) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing owned namespace'; owned_namespace=$1; shift ;;
    --writer-exclusivity-evidence) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing writer evidence'; writer_evidence=$1; shift ;;
    --receipt) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing receipt path'; receipt_path=$1; shift ;;
    --qemu-system) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing qemu-system path'; qemu_system=$1; shift ;;
    --qemu-img) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing qemu-img path'; qemu_img=$1; shift ;;
    --fixture-evidence) [ "$#" -gt 0 ] || fail 'preflight.arguments_invalid: missing fixture evidence'; fixture_evidence=$1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) fail "preflight.arguments_invalid: unknown option $option" ;;
  esac
done

for required in "$repository_root" "$candidate_sha" "$state_root" "$base_image" "$owned_namespace" "$writer_evidence" "$receipt_path"; do
  [ -n "$required" ] || { usage >&2; fail 'preflight.arguments_invalid: required option was omitted'; }
done
[ "${#candidate_sha}" -eq 40 ] || fail 'preflight.candidate_invalid: expected one lowercase 40-hex SHA'
case "$candidate_sha" in *[!0-9a-f]*) fail 'preflight.candidate_invalid: expected one lowercase 40-hex SHA' ;; esac
case "$owned_namespace" in vmcell-*) namespace_suffix=${owned_namespace#vmcell-} ;; *) namespace_suffix= ;; esac
case "$namespace_suffix" in ''|*[!a-z0-9-]*|*-) fail 'preflight.namespace_invalid: expected vmcell- followed by lowercase letters, digits, or internal hyphens' ;; esac
case "$namespace_suffix" in [a-z0-9]*) ;; *) fail 'preflight.namespace_invalid: namespace must begin with a lowercase letter or digit' ;; esac
[ "${#owned_namespace}" -le 64 ] || fail 'preflight.namespace_invalid: namespace exceeds 64 characters'
case "$writer_evidence" in
  *[!A-Za-z0-9._:-]*|'') fail 'preflight.writer_evidence_invalid: use 1-128 safe identifier characters' ;;
esac
[ "${#writer_evidence}" -le 128 ] || fail 'preflight.writer_evidence_invalid: value exceeds 128 characters'

if [ -n "$fixture_evidence" ]; then
  [ -z "$qemu_system" ] && [ -z "$qemu_img" ] || fail 'preflight.arguments_invalid: fixture and live QEMU evidence are mutually exclusive'
  evidence_source=fixture
else
  [ -n "$qemu_system" ] && [ -n "$qemu_img" ] || fail 'preflight.arguments_invalid: live mode requires qemu-system and qemu-img'
  evidence_source=live-read-only
fi
command -v python3 >/dev/null 2>&1 || fail 'preflight.host_invalid: python3 is required for strict JSON validation'

repository_path=$(canonical_directory "$repository_root" 'preflight.repository_invalid')
state_path=$(canonical_directory "$state_root" 'preflight.state_root_invalid')
base_path=$(canonical_file "$base_image" 'preflight.image_variant_incompatible')
receipt_parent=$(canonical_directory "$(dirname -- "$receipt_path")" 'preflight.receipt_parent_invalid')
receipt_name=$(basename -- "$receipt_path")
require_safe_text "$receipt_name" 'preflight.receipt_invalid'
case "$receipt_name" in .|..|'') fail 'preflight.receipt_invalid: invalid filename' ;; esac
receipt_path="$receipt_parent/$receipt_name"
[ ! -e "$receipt_path" ] && [ ! -L "$receipt_path" ] || fail 'preflight.receipt_exists: refusing to replace an existing path'

top_level=$(git -C "$repository_path" rev-parse --show-toplevel 2>/dev/null) || fail 'preflight.repository_invalid: git worktree unavailable'
[ "$(readlink -f -- "$top_level")" = "$repository_path" ] || fail 'preflight.repository_invalid: root was not the exact worktree'
head_sha=$(git -C "$repository_path" rev-parse HEAD 2>/dev/null) || fail 'preflight.repository_invalid: HEAD unavailable'
[ "$head_sha" = "$candidate_sha" ] || fail 'preflight.candidate_drift: HEAD did not match candidate SHA'
origin=$(git -C "$repository_path" remote get-url origin 2>/dev/null) || fail 'preflight.repository_invalid: origin unavailable'
case "$origin" in
  https://github.com/JerrySkywalker/vm-cell-manager|https://github.com/JerrySkywalker/vm-cell-manager.git|git@github.com:JerrySkywalker/vm-cell-manager|git@github.com:JerrySkywalker/vm-cell-manager.git|ssh://git@github.com/JerrySkywalker/vm-cell-manager|ssh://git@github.com/JerrySkywalker/vm-cell-manager.git) ;;
  *) fail 'preflight.repository_invalid: origin was not JerrySkywalker/vm-cell-manager' ;;
esac
repository_status=$(git -C "$repository_path" status --porcelain=v1 --untracked-files=all 2>/dev/null) || fail 'preflight.repository_invalid: worktree status was unavailable'
[ -z "$repository_status" ] || fail 'preflight.candidate_dirty: tracked or untracked changes were present'
case "$receipt_parent/" in "$repository_path/"*) fail 'preflight.receipt_invalid: receipt must be outside the source worktree' ;; esac
case "$receipt_parent/" in "$state_path/"*) fail 'preflight.receipt_invalid: receipt must be outside the vmcell state root' ;; esac

effective_uid=$(id -u)
effective_gid=$(id -g)
receipt_parent_uid=$(stat -Lc '%u' -- "$receipt_parent")
receipt_parent_mode=$(stat -Lc '%a' -- "$receipt_parent")
receipt_parent_identity=$(stat -Lc '%d:%i' -- "$receipt_parent")
[ "$receipt_parent_uid" = "$effective_uid" ] || fail 'preflight.receipt_parent_invalid: receipt parent is not owned by the effective identity'
[ "$receipt_parent_mode" = 700 ] || fail 'preflight.receipt_parent_invalid: receipt parent mode must be 0700'
probe_temp=
receipt_temp=
trap cleanup_temporary_paths EXIT HUP INT TERM
state_uid=$(stat -Lc '%u' -- "$state_path")
state_mode=$(stat -Lc '%a' -- "$state_path")
[ "$state_uid" = "$effective_uid" ] || fail 'preflight.state_root_invalid: state root is not owned by the effective identity'
[ "$state_mode" = 700 ] || fail 'preflight.state_root_invalid: state root mode must be 0700'
state_identity=$(stat -Lc '%d:%i' -- "$state_path")

case "$base_path" in *.qcow2) ;; *) fail 'preflight.image_variant_incompatible: prepared base must use .qcow2' ;; esac
base_writable=$(find "$base_path" -maxdepth 0 -perm /222 -print) || fail 'preflight.image_not_immutable: base image mode could not be checked'
[ -z "$base_writable" ] || fail 'preflight.image_not_immutable: base image has a write bit set'
base_sha256=$(sha256_file "$base_path" 'preflight.image_hash_invalid')
base_size=$(stat -Lc '%s' -- "$base_path")
base_mode=$(stat -Lc '%a' -- "$base_path")
base_identity=$(stat -Lc '%d:%i' -- "$base_path")

runtime_root="$state_path/runtime"
runtime_rows=$(collect_runtime_rows) || fail 'preflight.runtime_invalid: runtime prestate could not be collected'
runtime_entry_count=$(printf '%s\n' "$runtime_rows" | awk 'NF {count++} END {print count+0}')
runtime_fingerprint=$(sha256_text "$runtime_rows")
production_runtime_pattern="$state_path/runtime/<cell-id>/qemu"

if [ "$evidence_source" = fixture ]; then
  fixture_path=$(canonical_file "$fixture_evidence" 'preflight.fixture_invalid')
  allowed_fixture='host_fingerprint_sha256 qemu_system_version qemu_system_sha256 qemu_img_version qemu_img_sha256 kvm_identity foreign_qemu_count foreign_qemu_fingerprint_sha256 network_count network_fingerprint_sha256'
  while IFS= read -r line || [ -n "$line" ]; do
    key=${line%%=*}
    case " $allowed_fixture " in *" $key "*) ;; *) fail "preflight.fixture_invalid: unsupported key $key" ;; esac
  done < "$fixture_path"
  host_fingerprint=$(fixture_value host_fingerprint_sha256)
  qemu_system_version=$(fixture_value qemu_system_version)
  qemu_system_sha256=$(fixture_value qemu_system_sha256)
  qemu_img_version=$(fixture_value qemu_img_version)
  qemu_img_sha256=$(fixture_value qemu_img_sha256)
  kvm_identity=$(fixture_value kvm_identity)
  foreign_qemu_count=$(fixture_value foreign_qemu_count)
  foreign_qemu_fingerprint=$(fixture_value foreign_qemu_fingerprint_sha256)
  network_count=$(fixture_value network_count)
  network_fingerprint=$(fixture_value network_fingerprint_sha256)
  qemu_system_path='fixture://qemu-system-x86_64'
  qemu_img_path='fixture://qemu-img'
  host_proof=fixture-only
  kvm_status=fixture-declared-usable
else
  [ "$(uname -s)" = Linux ] || fail 'preflight.host_invalid: native Linux is required'
  [ "$(uname -m)" = x86_64 ] || fail 'preflight.architecture_mismatch: native x86_64 is required'
  if grep -Eqi 'microsoft|wsl' /proc/sys/kernel/osrelease /proc/version 2>/dev/null; then
    fail 'preflight.host_invalid: WSL is development evidence only'
  fi
  [ ! -e /.dockerenv ] && [ ! -e /run/.containerenv ] || fail 'preflight.host_invalid: a container is not a native acceptance host'
  if grep -Eqi '(docker|containerd|kubepods|lxc)' /proc/1/cgroup 2>/dev/null; then
    fail 'preflight.host_invalid: a container is not a native acceptance host'
  fi
  host_material="$(hostname)\n$(uname -srvm)\n$(cat /etc/machine-id 2>/dev/null || true)"
  host_fingerprint=$(sha256_text "$host_material")
  host_proof=native-linux-x86_64
  command -v timeout >/dev/null 2>&1 || fail 'preflight.host_invalid: bounded timeout command is unavailable'

  qemu_system_path=$(canonical_file "$qemu_system" 'preflight.qemu_system_absent')
  qemu_img_path=$(canonical_file "$qemu_img" 'preflight.qemu_img_absent')
  [ -x "$qemu_system_path" ] || fail 'preflight.qemu_system_absent: executable bit is missing'
  [ -x "$qemu_img_path" ] || fail 'preflight.qemu_img_absent: executable bit is missing'
  qemu_system_sha256=$(sha256_file "$qemu_system_path" 'preflight.qemu_system_hash_invalid')
  qemu_img_sha256=$(sha256_file "$qemu_img_path" 'preflight.qemu_img_hash_invalid')
  qemu_system_identity=$(stat -Lc '%d:%i:%s' -- "$qemu_system_path")
  qemu_img_identity=$(stat -Lc '%d:%i:%s' -- "$qemu_img_path")
  run_bounded_probe 'preflight.qemu_system_probe_failed' "$qemu_system_path" --version
  qemu_system_output=$bounded_probe_output
  run_bounded_probe 'preflight.qemu_img_probe_failed' "$qemu_img_path" --version
  qemu_img_output=$bounded_probe_output
  qemu_system_version=$(printf '%s\n' "$qemu_system_output" | sed -n '1p')
  qemu_img_version=$(printf '%s\n' "$qemu_img_output" | sed -n '1p')
  case "$qemu_system_version" in 'QEMU emulator version '*) ;; *) fail 'preflight.qemu_system_probe_failed: unrecognized version' ;; esac
  case "$qemu_img_version" in 'qemu-img version '*) ;; *) fail 'preflight.qemu_img_probe_failed: unrecognized version' ;; esac
  run_bounded_probe 'preflight.kvm_unavailable' "$qemu_system_path" -accel help
  accel_output=$bounded_probe_output
  printf '%s\n' "$accel_output" | grep -Eq '^[[:space:]]*kvm[[:space:]]*$' || fail 'preflight.kvm_unavailable: QEMU did not advertise KVM'
  run_bounded_probe 'preflight.image_variant_incompatible' "$qemu_img_path" info --output=json "$base_path"
  image_info=$bounded_probe_output
  python3 -c 'import json,sys; value=json.loads(sys.argv[1]); assert isinstance(value,dict); assert value.get("format")=="qcow2"; assert value.get("backing-filename") in (None, ""); assert value.get("full-backing-filename") in (None, "")' "$image_info" >/dev/null 2>&1 || fail 'preflight.image_variant_incompatible: qemu-img JSON did not describe one parentless qcow2 base'

  [ -c /dev/kvm ] && [ ! -L /dev/kvm ] || fail 'preflight.kvm_missing: /dev/kvm is missing or not an ordinary character device'
  kvm_before=$(stat -Lc '%d:%i:%t:%T' -- /dev/kvm)
  if ! exec 9<>/dev/kvm 2>/dev/null; then
    fail 'preflight.kvm_permission_denied: /dev/kvm is not read-write usable by the current identity'
  fi
  kvm_opened=$(stat -Lc '%d:%i:%t:%T' -- "/proc/$$/fd/9") || { exec 9>&-; fail 'preflight.kvm_identity_drift: opened /dev/kvm identity was unavailable'; }
  [ -c /dev/kvm ] && [ ! -L /dev/kvm ] || { exec 9>&-; fail 'preflight.kvm_identity_drift: current /dev/kvm path changed during admission'; }
  kvm_current=$(stat -Lc '%d:%i:%t:%T' -- /dev/kvm) || { exec 9>&-; fail 'preflight.kvm_identity_drift: current /dev/kvm identity was unavailable'; }
  exec 9>&-
  [ "$kvm_before" = "$kvm_opened" ] && [ "$kvm_before" = "$kvm_current" ] || fail 'preflight.kvm_identity_drift: /dev/kvm pre-open, opened-FD, and current-path identities differed'
  kvm_identity=$kvm_opened
  kvm_status=read-write-usable

  [ "$(sha256_file "$qemu_system_path" 'preflight.executable_drift')" = "$qemu_system_sha256" ] || fail 'preflight.executable_drift: qemu-system changed during preflight'
  [ "$(sha256_file "$qemu_img_path" 'preflight.executable_drift')" = "$qemu_img_sha256" ] || fail 'preflight.executable_drift: qemu-img changed during preflight'
  [ "$(stat -Lc '%d:%i:%s' -- "$qemu_system_path")" = "$qemu_system_identity" ] || fail 'preflight.executable_drift: qemu-system identity changed during preflight'
  [ "$(stat -Lc '%d:%i:%s' -- "$qemu_img_path")" = "$qemu_img_identity" ] || fail 'preflight.executable_drift: qemu-img identity changed during preflight'
  [ "$(sha256_file "$base_path" 'preflight.image_drift')" = "$base_sha256" ] || fail 'preflight.image_drift: immutable base changed during preflight'
  [ "$(stat -Lc '%s' -- "$base_path")" = "$base_size" ] || fail 'preflight.image_drift: immutable base size changed during preflight'

  process_rows=$(ps -eo pid=,uid=,comm=) || fail 'preflight.foreign_prestate_invalid: process prestate could not be enumerated'
  foreign_unsorted=$(printf '%s\n' "$process_rows" | awk '$3 ~ /^qemu-system-/ {print $1 "|" $2 "|" $3}') || fail 'preflight.foreign_prestate_invalid: process prestate could not be filtered'
  foreign_rows=$(printf '%s\n' "$foreign_unsorted" | LC_ALL=C sort) || fail 'preflight.foreign_prestate_invalid: process prestate could not be sorted'
  foreign_qemu_count=$(printf '%s\n' "$foreign_rows" | awk 'NF {count++} END {print count+0}')
  foreign_qemu_fingerprint=$(sha256_text "$foreign_rows")
  network_unsorted=$(find /sys/class/net -mindepth 1 -maxdepth 1 -printf '%f\n') || fail 'preflight.network_prestate_invalid: network prestate could not be enumerated'
  network_rows=$(printf '%s\n' "$network_unsorted" | LC_ALL=C sort) || fail 'preflight.network_prestate_invalid: network prestate could not be sorted'
  network_count=$(printf '%s\n' "$network_rows" | awk 'NF {count++} END {print count+0}')
  network_fingerprint=$(sha256_text "$network_rows")
fi

require_sha256 "$host_fingerprint" 'preflight.host_fingerprint_invalid'
require_sha256 "$qemu_system_sha256" 'preflight.qemu_system_hash_invalid'
require_sha256 "$qemu_img_sha256" 'preflight.qemu_img_hash_invalid'
require_sha256 "$foreign_qemu_fingerprint" 'preflight.foreign_prestate_invalid'
require_sha256 "$network_fingerprint" 'preflight.network_prestate_invalid'
require_sha256 "$runtime_fingerprint" 'preflight.runtime_prestate_invalid'
require_sha256 "$base_sha256" 'preflight.image_hash_invalid'
for count in "$foreign_qemu_count" "$network_count" "$runtime_entry_count"; do
  case "$count" in ''|*[!0-9]*) fail 'preflight.fixture_invalid: counts must be non-negative integers' ;; esac
done
require_safe_text "$repository_path" 'preflight.repository_path_invalid'
require_safe_text "$state_path" 'preflight.state_path_invalid'
require_safe_text "$base_path" 'preflight.base_path_invalid'
require_safe_text "$receipt_parent" 'preflight.receipt_parent_invalid'
require_safe_text "$receipt_path" 'preflight.receipt_invalid'
require_safe_text "$qemu_system_path" 'preflight.qemu_system_path_invalid'
require_safe_text "$qemu_img_path" 'preflight.qemu_img_path_invalid'
require_safe_text "$production_runtime_pattern" 'preflight.runtime_pattern_invalid'
require_safe_text "$host_proof" 'preflight.host_proof_invalid'
require_safe_text "$qemu_system_version" 'preflight.qemu_system_version_invalid'
require_safe_text "$qemu_img_version" 'preflight.qemu_img_version_invalid'
require_safe_text "$kvm_identity" 'preflight.kvm_identity_invalid'
[ "${#qemu_system_version}" -le 512 ] || fail 'preflight.qemu_system_version_invalid: value exceeded 512 characters'
[ "${#qemu_img_version}" -le 512 ] || fail 'preflight.qemu_img_version_invalid: value exceeded 512 characters'
[ "${#kvm_identity}" -le 512 ] || fail 'preflight.kvm_identity_invalid: value exceeded 512 characters'

final_head=$(git -C "$repository_path" rev-parse HEAD 2>/dev/null) || fail 'preflight.candidate_drift: HEAD could not be re-read'
[ "$final_head" = "$candidate_sha" ] || fail 'preflight.candidate_drift: HEAD changed during preflight'
final_status=$(git -C "$repository_path" status --porcelain=v1 --untracked-files=all 2>/dev/null) || fail 'preflight.candidate_drift: worktree status could not be re-read'
[ -z "$final_status" ] || fail 'preflight.candidate_drift: worktree changed during preflight'
[ "$(stat -Lc '%u:%a:%d:%i' -- "$state_path")" = "$state_uid:$state_mode:$state_identity" ] || fail 'preflight.state_root_drift: state root identity changed during preflight'
[ "$(stat -Lc '%u:%a:%d:%i' -- "$receipt_parent")" = "$receipt_parent_uid:$receipt_parent_mode:$receipt_parent_identity" ] || fail 'preflight.receipt_parent_drift: receipt parent identity changed during preflight'
[ "$(stat -Lc '%d:%i:%s:%a' -- "$base_path")" = "$base_identity:$base_size:$base_mode" ] || fail 'preflight.image_drift: immutable base identity changed during preflight'
[ "$(sha256_file "$base_path" 'preflight.image_drift')" = "$base_sha256" ] || fail 'preflight.image_drift: immutable base contents changed during preflight'
runtime_rows_final=$(collect_runtime_rows) || fail 'preflight.runtime_drift: runtime prestate could not be recollected'
[ "$runtime_rows_final" = "$runtime_rows" ] || fail 'preflight.runtime_drift: runtime tree changed during preflight'
[ ! -e "$receipt_path" ] && [ ! -L "$receipt_path" ] || fail 'preflight.receipt_exists: refusing to replace an existing path'

umask 077
receipt_temp=$(mktemp "$receipt_parent/.vmcell-linux-kvm-preflight.XXXXXX") || fail 'preflight.receipt_write_failed: temporary receipt could not be created'

repository_json=$(json_escape "$repository_path")
state_json=$(json_escape "$state_path")
base_json=$(json_escape "$base_path")
receipt_parent_json=$(json_escape "$receipt_parent")
receipt_path_json=$(json_escape "$receipt_path")
system_path_json=$(json_escape "$qemu_system_path")
image_path_json=$(json_escape "$qemu_img_path")
system_version_json=$(json_escape "$qemu_system_version")
image_version_json=$(json_escape "$qemu_img_version")
kvm_identity_json=$(json_escape "$kvm_identity")
runtime_pattern_json=$(json_escape "$production_runtime_pattern")
namespace_json=$(json_escape "$owned_namespace")
writer_json=$(json_escape "$writer_evidence")

printf '%s\n' \
  '{' \
  '  "schema_version": 1,' \
  '  "contract": "vmcell.linux-kvm-preflight.v1",' \
  '  "authorizing": false,' \
  '  "mutation_performed": false,' \
  '  "real_platform_acceptance": false,' \
  "  \"evidence_source\": \"$evidence_source\"," \
  '  "repository": {' \
  '    "slug": "JerrySkywalker/vm-cell-manager",' \
  "    \"canonical_path\": \"$repository_json\"," \
  "    \"candidate_sha\": \"$candidate_sha\"" \
  '  },' \
  '  "host": {' \
  '    "os": "linux",' \
  '    "architecture": "x86_64",' \
  "    \"proof\": \"$host_proof\"," \
  "    \"fingerprint_sha256\": \"$host_fingerprint\"," \
  "    \"effective_uid\": $effective_uid," \
  "    \"effective_gid\": $effective_gid" \
  '  },' \
  '  "provider_path": {' \
  '    "provider": "qemu",' \
  '    "accelerator": "kvm",' \
  '    "guest_os": "linux",' \
  '    "guest_architecture": "x86_64",' \
  '    "guest_transport": "qga",' \
  '    "support_status": "untested"' \
  '  },' \
  "  \"qemu_system\": {\"canonical_path\": \"$system_path_json\", \"version\": \"$system_version_json\", \"sha256\": \"$qemu_system_sha256\"}," \
  "  \"qemu_img\": {\"canonical_path\": \"$image_path_json\", \"version\": \"$image_version_json\", \"sha256\": \"$qemu_img_sha256\"}," \
  "  \"kvm\": {\"path\": \"/dev/kvm\", \"status\": \"$kvm_status\", \"open_mode\": \"read-write-no-ioctl\", \"identity\": \"$kvm_identity_json\"}," \
  "  \"state_root\": {\"canonical_path\": \"$state_json\", \"owner_uid\": $state_uid, \"mode\": \"$state_mode\", \"device_inode\": \"$state_identity\"}," \
  "  \"receipt_target\": {\"path\": \"$receipt_path_json\", \"canonical_parent\": \"$receipt_parent_json\", \"parent_owner_uid\": $receipt_parent_uid, \"parent_mode\": \"$receipt_parent_mode\", \"parent_device_inode\": \"$receipt_parent_identity\"}," \
  "  \"immutable_base\": {\"canonical_path\": \"$base_json\", \"format\": \"qcow2\", \"size\": $base_size, \"mode\": \"$base_mode\", \"sha256\": \"$base_sha256\", \"backing_parent\": null}," \
  '  "qga": {"guest_assumption": "prepared-linux-x86_64-qga-enabled", "readiness": "not-exercised"},' \
  "  \"control_namespace\": {\"acceptance_window_namespace\": \"$namespace_json\", \"production_runtime_pattern\": \"$runtime_pattern_json\", \"qmp_filename\": \"qmp.sock\", \"qga_filename\": \"qga.sock\", \"runtime_prestate_entry_count\": $runtime_entry_count, \"runtime_prestate_fingerprint_sha256\": \"$runtime_fingerprint\"}," \
  "  \"foreign_qemu_prestate\": {\"count\": $foreign_qemu_count, \"fingerprint_sha256\": \"$foreign_qemu_fingerprint\"}," \
  "  \"network_prestate\": {\"count\": $network_count, \"fingerprint_sha256\": \"$network_fingerprint\"}," \
  "  \"writer_exclusivity\": {\"evidence_id\": \"$writer_json\", \"proof_kind\": \"external-attestation\"}," \
  '  "cleanup": {"policy": "exact-owned-only", "rollback_evidence": "pending-real-platform-gate"},' \
  '  "result": "PREFLIGHT_ONLY"' \
  '}' > "$receipt_temp"

[ "$(stat -Lc '%a' -- "$receipt_temp")" = 600 ] || fail 'preflight.receipt_write_failed: temporary receipt mode was not 0600'
python3 -c 'import json,sys; value=json.load(open(sys.argv[1], "r", encoding="utf-8")); assert value.get("contract")=="vmcell.linux-kvm-preflight.v1"; assert value.get("authorizing") is False' "$receipt_temp" >/dev/null 2>&1 || fail 'preflight.receipt_write_failed: temporary receipt was not strict UTF-8 contract JSON'
[ "$(stat -Lc '%u:%a:%d:%i' -- "$receipt_parent")" = "$receipt_parent_uid:$receipt_parent_mode:$receipt_parent_identity" ] || fail 'preflight.receipt_parent_drift: receipt parent identity changed before publication'
[ ! -e "$receipt_path" ] && [ ! -L "$receipt_path" ] || fail 'preflight.receipt_exists: refusing to replace an existing path'
receipt_temp_identity=$(stat -Lc '%u:%a:%d:%i' -- "$receipt_temp") || fail 'preflight.receipt_write_failed: temporary receipt identity was unavailable'
case "$receipt_temp_identity" in "$effective_uid:600:"*) ;; *) fail 'preflight.receipt_write_failed: temporary receipt identity was invalid' ;; esac
publish_receipt_noreplace "$receipt_temp" "$receipt_path" || fail 'preflight.receipt_exists: atomic exact-target no-clobber publication failed'
receipt_temp=
trap - EXIT HUP INT TERM
printf 'Linux KVM preflight receipt written: %s\n' "$receipt_path" || :
exit 0
