#!/usr/bin/env sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
test_root=$(mktemp -d)
case "$test_root" in
  /tmp/*) ;;
  *) echo 'campaign fixture root escaped /tmp' >&2; exit 1 ;;
esac
trap 'rm -rf -- "$test_root"' EXIT HUP INT TERM

fake_bin="$test_root/bin"
runner_temp="$test_root/runner-temp"
mkdir -m 700 -- "$fake_bin" "$runner_temp"

cat >"$fake_bin/cargo" <<'SH'
#!/usr/bin/env sh
set -eu

test_name=$6
case " $* " in
  *' --list '*)
    if [ "${VMCELL_FIXTURE_OVERFLOW:-0}" = 1 ]; then
      python3 - <<'PY'
import sys
sys.stdout.write("x" * 1048577)
PY
    else
      printf '%s: test\n\n1 test, 0 benchmarks\n' "$test_name"
    fi
    ;;
  *)
    printf 'running 1 test\n'
    printf 'test %s ... ok\n\n' "$test_name"
    printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n'
    ;;
esac
SH
chmod 700 "$fake_bin/cargo"

source_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
receipt="$runner_temp/pass-receipt.json"
PATH="$fake_bin:$PATH" RUNNER_TEMP="$runner_temp" \
  sh "$repository_root/tools/check-reliability-campaign.sh" \
    --manifest "$repository_root/tools/reliability-campaign.json" \
    --source-sha "$source_sha" \
    --receipt "$receipt" >/dev/null

python3 - "$receipt" "$source_sha" <<'PY'
import json
import sys

path, source_sha = sys.argv[1:]
value = json.load(open(path, "r", encoding="utf-8"))
assert value["contract"] == "vmcell.reliability-extended-receipt.v1"
assert value["source_sha"] == source_sha
assert value["campaign"]["case_count"] == 5
assert value["result"] == "PASS"
PY

overflow_receipt="$runner_temp/overflow-receipt.json"
if PATH="$fake_bin:$PATH" RUNNER_TEMP="$runner_temp" VMCELL_FIXTURE_OVERFLOW=1 \
    sh "$repository_root/tools/check-reliability-campaign.sh" \
      --manifest "$repository_root/tools/reliability-campaign.json" \
      --source-sha "$source_sha" \
      --receipt "$overflow_receipt" >/dev/null 2>&1; then
  echo 'campaign fixture accepted output beyond its one-MiB capture bound' >&2
  exit 1
fi
[ ! -e "$overflow_receipt" ] || {
  echo 'campaign fixture published a receipt after bounded-output rejection' >&2
  exit 1
}

printf 'Reliability campaign bounded-capture fixture passed\n'
