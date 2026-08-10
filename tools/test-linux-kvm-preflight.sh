#!/usr/bin/env sh
set -eu

root=$(mktemp -d)
cleanup() { rm -rf -- "$root"; }
trap cleanup EXIT HUP INT TERM

repository="$root/repository"
state="$root/state"
receipts="$root/receipts"
base="$root/linux-qga.qcow2"
fixture="$root/evidence.txt"
mkdir -m 700 "$repository" "$state" "$receipts"
printf 'fixture base\n' > "$base"
chmod 400 "$base"

git -C "$repository" init -q
git -C "$repository" config user.name vmcell-test
git -C "$repository" config user.email vmcell-test.invalid
git -C "$repository" remote add origin https://github.com/JerrySkywalker/vm-cell-manager.git
printf 'fixture\n' > "$repository/tracked.txt"
git -C "$repository" add tracked.txt
git -C "$repository" commit -qm fixture
candidate=$(git -C "$repository" rev-parse HEAD)

hash=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
{
  printf 'host_fingerprint_sha256=%s\n' "$hash"
  printf 'qemu_system_version=QEMU emulator version fixture\n'
  printf 'qemu_system_sha256=%s\n' "$hash"
  printf 'qemu_img_version=qemu-img version fixture\n'
  printf 'qemu_img_sha256=%s\n' "$hash"
  printf 'kvm_identity=1:2:10:e8\n'
  printf 'foreign_qemu_count=0\n'
  printf 'foreign_qemu_fingerprint_sha256=%s\n' "$hash"
  printf 'network_count=1\n'
  printf 'network_fingerprint_sha256=%s\n' "$hash"
} > "$fixture"

preflight=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/linux-kvm-preflight.sh
receipt="$receipts/preflight.json"
sh "$preflight" \
  --repository-root "$repository" \
  --candidate-sha "$candidate" \
  --state-root "$state" \
  --base-image "$base" \
  --owned-namespace vmcell-linux-kvm-acceptance-001 \
  --writer-exclusivity-evidence fixture-window-001 \
  --receipt "$receipt" \
  --fixture-evidence "$fixture" >/dev/null

[ -f "$receipt" ] && [ "$(stat -c '%a' "$receipt")" = 600 ]
grep -F '"contract": "vmcell.linux-kvm-preflight.v1"' "$receipt" >/dev/null
grep -F '"authorizing": false' "$receipt" >/dev/null
grep -F '"evidence_source": "fixture"' "$receipt" >/dev/null
grep -F '"real_platform_acceptance": false' "$receipt" >/dev/null
grep -F '"support_status": "untested"' "$receipt" >/dev/null
grep -F '"status": "fixture-declared-usable"' "$receipt" >/dev/null
python3 -c 'import json,sys; json.load(open(sys.argv[1], encoding="utf-8"))' "$receipt"

printf 'dirty\n' > "$repository/untracked.txt"
dirty_receipt="$receipts/dirty.json"
if sh "$preflight" \
  --repository-root "$repository" \
  --candidate-sha "$candidate" \
  --state-root "$state" \
  --base-image "$base" \
  --owned-namespace vmcell-linux-kvm-acceptance-002 \
  --writer-exclusivity-evidence fixture-window-002 \
  --receipt "$dirty_receipt" \
  --fixture-evidence "$fixture" >/dev/null 2>&1; then
  printf '%s\n' 'dirty candidate was accepted' >&2
  exit 1
fi
[ ! -e "$dirty_receipt" ]
rm "$repository/untracked.txt"

chmod 755 "$state"
mode_receipt="$receipts/mode.json"
if sh "$preflight" \
  --repository-root "$repository" \
  --candidate-sha "$candidate" \
  --state-root "$state" \
  --base-image "$base" \
  --owned-namespace vmcell-linux-kvm-acceptance-003 \
  --writer-exclusivity-evidence fixture-window-003 \
  --receipt "$mode_receipt" \
  --fixture-evidence "$fixture" >/dev/null 2>&1; then
  printf '%s\n' 'non-private state root was accepted' >&2
  exit 1
fi
[ ! -e "$mode_receipt" ]

chmod 700 "$state"
linked_state="$root/linked-state"
ln -s "$state" "$linked_state"
link_receipt="$receipts/link.json"
if sh "$preflight" \
  --repository-root "$repository" \
  --candidate-sha "$candidate" \
  --state-root "$linked_state" \
  --base-image "$base" \
  --owned-namespace vmcell-linux-kvm-acceptance-004 \
  --writer-exclusivity-evidence fixture-window-004 \
  --receipt "$link_receipt" \
  --fixture-evidence "$fixture" >/dev/null 2>&1; then
  printf '%s\n' 'symlinked state root was accepted' >&2
  exit 1
fi
[ ! -e "$link_receipt" ]

printf '%s\n' 'Linux KVM preflight fixture contract passed'
