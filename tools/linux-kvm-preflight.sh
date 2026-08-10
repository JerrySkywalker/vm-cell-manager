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

require_safe_text() {
  value=$1
  label=$2
  [ -n "$value" ] || fail "$label: value is empty"
  if printf '%s' "$value" | LC_ALL=C grep -q '[[:cntrl:]]'; then
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
[ -z "$(git -C "$repository_path" status --porcelain=v1 --untracked-files=all 2>/dev/null)" ] || fail 'preflight.candidate_dirty: tracked or untracked changes were present'
case "$receipt_parent/" in "$repository_path/"*) fail 'preflight.receipt_invalid: receipt must be outside the source worktree' ;; esac

effective_uid=$(id -u)
effective_gid=$(id -g)
state_uid=$(stat -Lc '%u' -- "$state_path")
state_mode=$(stat -Lc '%a' -- "$state_path")
[ "$state_uid" = "$effective_uid" ] || fail 'preflight.state_root_invalid: state root is not owned by the effective identity'
[ "$state_mode" = 700 ] || fail 'preflight.state_root_invalid: state root mode must be 0700'
state_identity=$(stat -Lc '%d:%i' -- "$state_path")

case "$base_path" in *.qcow2) ;; *) fail 'preflight.image_variant_incompatible: prepared base must use .qcow2' ;; esac
[ -z "$(find "$base_path" -maxdepth 0 -perm /222 -print)" ] || fail 'preflight.image_not_immutable: base image has a write bit set'
base_sha256=$(sha256sum -- "$base_path" | awk '{print $1}')
base_size=$(stat -Lc '%s' -- "$base_path")
base_mode=$(stat -Lc '%a' -- "$base_path")

qmp_path="$state_path/runtime/$owned_namespace/qemu/qmp.sock"
qga_path="$state_path/runtime/$owned_namespace/qemu/qga.sock"
[ "$(printf '%s' "$qmp_path" | wc -c)" -le 96 ] && [ "$(printf '%s' "$qga_path" | wc -c)" -le 96 ] || fail 'preflight.qmp_namespace_invalid: Unix control socket path exceeds 96 bytes'
for endpoint in "$qmp_path" "$qga_path"; do
  [ ! -e "$endpoint" ] && [ ! -L "$endpoint" ] || fail 'preflight.runtime_collision: exact-owned QMP/QGA path already exists'
done

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

  qemu_system_path=$(canonical_file "$qemu_system" 'preflight.qemu_system_absent')
  qemu_img_path=$(canonical_file "$qemu_img" 'preflight.qemu_img_absent')
  [ -x "$qemu_system_path" ] || fail 'preflight.qemu_system_absent: executable bit is missing'
  [ -x "$qemu_img_path" ] || fail 'preflight.qemu_img_absent: executable bit is missing'
  qemu_system_sha256=$(sha256sum -- "$qemu_system_path" | awk '{print $1}')
  qemu_img_sha256=$(sha256sum -- "$qemu_img_path" | awk '{print $1}')
  qemu_system_identity=$(stat -Lc '%d:%i:%s' -- "$qemu_system_path")
  qemu_img_identity=$(stat -Lc '%d:%i:%s' -- "$qemu_img_path")
  qemu_system_output=$("$qemu_system_path" --version 2>/dev/null) || fail 'preflight.qemu_system_probe_failed: version probe failed'
  qemu_img_output=$("$qemu_img_path" --version 2>/dev/null) || fail 'preflight.qemu_img_probe_failed: version probe failed'
  [ "$(printf '%s' "$qemu_system_output" | wc -c)" -le 65536 ] || fail 'preflight.qemu_system_probe_failed: output exceeded 65536 bytes'
  [ "$(printf '%s' "$qemu_img_output" | wc -c)" -le 65536 ] || fail 'preflight.qemu_img_probe_failed: output exceeded 65536 bytes'
  qemu_system_version=$(printf '%s\n' "$qemu_system_output" | sed -n '1p')
  qemu_img_version=$(printf '%s\n' "$qemu_img_output" | sed -n '1p')
  case "$qemu_system_version" in 'QEMU emulator version '*) ;; *) fail 'preflight.qemu_system_probe_failed: unrecognized version' ;; esac
  case "$qemu_img_version" in 'qemu-img version '*) ;; *) fail 'preflight.qemu_img_probe_failed: unrecognized version' ;; esac
  accel_output=$("$qemu_system_path" -accel help 2>/dev/null) || fail 'preflight.kvm_unavailable: accelerator probe failed'
  printf '%s\n' "$accel_output" | grep -Eq '^[[:space:]]*kvm[[:space:]]*$' || fail 'preflight.kvm_unavailable: QEMU did not advertise KVM'
  image_info=$("$qemu_img_path" info --output=json "$base_path" 2>/dev/null) || fail 'preflight.image_variant_incompatible: qemu-img info failed'
  [ "$(printf '%s' "$image_info" | wc -c)" -le 65536 ] || fail 'preflight.image_variant_incompatible: qemu-img output exceeded 65536 bytes'
  printf '%s' "$image_info" | grep -Eq '"format"[[:space:]]*:[[:space:]]*"qcow2"' || fail 'preflight.image_variant_incompatible: image format was not qcow2'
  if printf '%s' "$image_info" | grep -Eq '"(full-)?backing-filename"[[:space:]]*:[[:space:]]*"'; then
    fail 'preflight.image_variant_incompatible: immutable base already has a backing parent'
  fi

  [ -c /dev/kvm ] && [ ! -L /dev/kvm ] || fail 'preflight.kvm_missing: /dev/kvm is missing or not an ordinary character device'
  kvm_before=$(stat -Lc '%d:%i:%t:%T' -- /dev/kvm)
  if ! (exec 9<>/dev/kvm) 2>/dev/null; then
    fail 'preflight.kvm_permission_denied: /dev/kvm is not read-write usable by the current identity'
  fi
  kvm_after=$(stat -Lc '%d:%i:%t:%T' -- /dev/kvm)
  [ "$kvm_before" = "$kvm_after" ] || fail 'preflight.kvm_identity_drift: /dev/kvm identity changed during admission'
  kvm_identity=$kvm_after
  kvm_status=read-write-usable

  [ "$(sha256sum -- "$qemu_system_path" | awk '{print $1}')" = "$qemu_system_sha256" ] || fail 'preflight.executable_drift: qemu-system changed during preflight'
  [ "$(sha256sum -- "$qemu_img_path" | awk '{print $1}')" = "$qemu_img_sha256" ] || fail 'preflight.executable_drift: qemu-img changed during preflight'
  [ "$(stat -Lc '%d:%i:%s' -- "$qemu_system_path")" = "$qemu_system_identity" ] || fail 'preflight.executable_drift: qemu-system identity changed during preflight'
  [ "$(stat -Lc '%d:%i:%s' -- "$qemu_img_path")" = "$qemu_img_identity" ] || fail 'preflight.executable_drift: qemu-img identity changed during preflight'
  [ "$(sha256sum -- "$base_path" | awk '{print $1}')" = "$base_sha256" ] || fail 'preflight.image_drift: immutable base changed during preflight'
  [ "$(stat -Lc '%s' -- "$base_path")" = "$base_size" ] || fail 'preflight.image_drift: immutable base size changed during preflight'

  foreign_rows=$(ps -eo pid=,uid=,comm= | awk '$3 ~ /^qemu-system-/ {print $1 "|" $2 "|" $3}' | sort)
  foreign_qemu_count=$(printf '%s\n' "$foreign_rows" | awk 'NF {count++} END {print count+0}')
  foreign_qemu_fingerprint=$(sha256_text "$foreign_rows")
  network_rows=$(find /sys/class/net -mindepth 1 -maxdepth 1 -printf '%f\n' 2>/dev/null | sort)
  network_count=$(printf '%s\n' "$network_rows" | awk 'NF {count++} END {print count+0}')
  network_fingerprint=$(sha256_text "$network_rows")
fi

for value_and_label in \
  "$host_fingerprint|preflight.host_fingerprint_invalid" \
  "$qemu_system_sha256|preflight.qemu_system_hash_invalid" \
  "$qemu_img_sha256|preflight.qemu_img_hash_invalid" \
  "$foreign_qemu_fingerprint|preflight.foreign_prestate_invalid" \
  "$network_fingerprint|preflight.network_prestate_invalid"; do
  value=${value_and_label%%|*}
  label=${value_and_label#*|}
  require_sha256 "$value" "$label"
done
for count in "$foreign_qemu_count" "$network_count"; do
  case "$count" in ''|*[!0-9]*) fail 'preflight.fixture_invalid: counts must be non-negative integers' ;; esac
done
for text_and_label in \
  "$qemu_system_version|preflight.qemu_system_version_invalid" \
  "$qemu_img_version|preflight.qemu_img_version_invalid" \
  "$kvm_identity|preflight.kvm_identity_invalid"; do
  value=${text_and_label%%|*}
  label=${text_and_label#*|}
  require_safe_text "$value" "$label"
  [ "${#value}" -le 512 ] || fail "$label: value exceeded 512 characters"
done

umask 077
receipt_temp=$(mktemp "$receipt_parent/.vmcell-linux-kvm-preflight.XXXXXX") || fail 'preflight.receipt_write_failed: temporary receipt could not be created'
cleanup_temp() { [ ! -e "$receipt_temp" ] || rm -f -- "$receipt_temp"; }
trap cleanup_temp EXIT HUP INT TERM

repository_json=$(json_escape "$repository_path")
state_json=$(json_escape "$state_path")
base_json=$(json_escape "$base_path")
system_path_json=$(json_escape "$qemu_system_path")
image_path_json=$(json_escape "$qemu_img_path")
system_version_json=$(json_escape "$qemu_system_version")
image_version_json=$(json_escape "$qemu_img_version")
kvm_identity_json=$(json_escape "$kvm_identity")
qmp_json=$(json_escape "$qmp_path")
qga_json=$(json_escape "$qga_path")
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
  "  \"immutable_base\": {\"canonical_path\": \"$base_json\", \"format\": \"qcow2\", \"size\": $base_size, \"mode\": \"$base_mode\", \"sha256\": \"$base_sha256\", \"backing_parent\": null}," \
  '  "qga": {"guest_assumption": "prepared-linux-x86_64-qga-enabled", "readiness": "not-exercised"},' \
  "  \"control_namespace\": {\"owned_namespace\": \"$namespace_json\", \"qmp_path\": \"$qmp_json\", \"qga_path\": \"$qga_json\", \"prestate_absent\": true}," \
  "  \"foreign_qemu_prestate\": {\"count\": $foreign_qemu_count, \"fingerprint_sha256\": \"$foreign_qemu_fingerprint\"}," \
  "  \"network_prestate\": {\"count\": $network_count, \"fingerprint_sha256\": \"$network_fingerprint\"}," \
  "  \"writer_exclusivity\": {\"evidence_id\": \"$writer_json\", \"proof_kind\": \"external-attestation\"}," \
  '  "cleanup": {"policy": "exact-owned-only", "rollback_evidence": "pending-real-platform-gate"},' \
  '  "result": "PREFLIGHT_ONLY"' \
  '}' > "$receipt_temp"

[ "$(stat -Lc '%a' -- "$receipt_temp")" = 600 ] || fail 'preflight.receipt_write_failed: temporary receipt mode was not 0600'
mv -- "$receipt_temp" "$receipt_path"
trap - EXIT HUP INT TERM
printf 'Linux KVM preflight receipt written: %s\n' "$receipt_path"
