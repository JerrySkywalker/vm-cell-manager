#!/usr/bin/env sh
set -eu

manifest=
source_sha=
receipt=

while [ "$#" -gt 0 ]; do
  case "$1" in
    --manifest)
      [ "$#" -ge 2 ] || { echo 'missing value for --manifest' >&2; exit 2; }
      manifest=$2
      shift 2
      ;;
    --source-sha)
      [ "$#" -ge 2 ] || { echo 'missing value for --source-sha' >&2; exit 2; }
      source_sha=$2
      shift 2
      ;;
    --receipt)
      [ "$#" -ge 2 ] || { echo 'missing value for --receipt' >&2; exit 2; }
      receipt=$2
      shift 2
      ;;
    *)
      echo 'unknown reliability campaign argument' >&2
      exit 2
      ;;
  esac
done

[ -n "$manifest" ] || { echo 'missing --manifest' >&2; exit 2; }
[ -n "$source_sha" ] || { echo 'missing --source-sha' >&2; exit 2; }
[ -n "$receipt" ] || { echo 'missing --receipt' >&2; exit 2; }

case "$source_sha" in
  *[!0123456789abcdef]*)
    echo 'source SHA must be exactly 40 lowercase hexadecimal characters' >&2
    exit 2
    ;;
esac
[ "${#source_sha}" -eq 40 ] || {
  echo 'source SHA must be exactly 40 lowercase hexadecimal characters' >&2
  exit 2
}

[ -n "${RUNNER_TEMP:-}" ] || { echo 'RUNNER_TEMP is required' >&2; exit 2; }
[ -f "$manifest" ] || { echo 'campaign manifest is missing' >&2; exit 2; }

runner_temp=$(realpath "$RUNNER_TEMP")
receipt_path=$(realpath -m "$receipt")
case "$receipt_path" in
  "$runner_temp"/*) ;;
  *)
    echo 'receipt must be inside RUNNER_TEMP' >&2
    exit 2
    ;;
esac
[ ! -e "$receipt_path" ] || { echo 'receipt target already exists' >&2; exit 2; }

campaign_dir=$(mktemp -d "$runner_temp/vmcell-reliability-campaign.XXXXXX")
campaign_rows="$campaign_dir/cases.tsv"
trap 'rm -f -- "$campaign_rows"; rmdir -- "$campaign_dir" 2>/dev/null || true' EXIT HUP INT TERM

manifest_sha256=$(python3 - "$manifest" "$campaign_rows" <<'PY'
import hashlib
import json
import sys

path, rows_path = sys.argv[1:]
raw = open(path, "rb").read()

def reject_duplicates(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate JSON key")
        value[key] = item
    return value

try:
    parsed = json.loads(raw.decode("utf-8"), object_pairs_hook=reject_duplicates)
except (OSError, UnicodeDecodeError, ValueError, json.JSONDecodeError):
    raise SystemExit("campaign manifest is not strict UTF-8 JSON")

expected = {
    "schema_version": 1,
    "contract": "vmcell.reliability-campaign.v1",
    "campaign_id": "r3-fixed-reliability-v1",
    "rust_toolchain": "1.85.0",
    "case_limit": 5,
    "case_timeout_seconds": 120,
    "campaign_timeout_seconds": 600,
    "cases": [
        {
            "target": "reliability_harness",
            "test": "seeded_lifecycle_cases_are_reproducible_and_disjoint_from_normal_ci",
            "seed": "6a09e667f3bcc909",
        },
        {
            "target": "reliability_harness",
            "test": "bounded_minimizer_returns_a_real_rejected_transition_as_serialized_input",
            "seed": "6a09e667f3bcc909",
        },
        {
            "target": "reliability_model_matrix",
            "test": "run_selection_matrix_has_stable_outcomes_and_never_implicitly_selects_tcg",
            "seed": "fixed-v1",
        },
        {
            "target": "reliability_model_matrix",
            "test": "job_spec_plan_and_result_metadata_bind_provenance_without_authority_or_secrets",
            "seed": "fixed-v1",
        },
        {
            "target": "reliability_model_matrix",
            "test": "durable_correlation_schema_fence_is_property_exact",
            "seed": "fixed-v1",
        },
    ],
}

if parsed != expected:
    raise SystemExit("campaign manifest does not match the fixed R3 allowlist")

with open(rows_path, "x", encoding="ascii", newline="\n") as rows:
    for index, item in enumerate(expected["cases"], start=1):
        rows.write(f"{index}\t{item['target']}\t{item['test']}\n")

print(hashlib.sha256(raw).hexdigest())
PY
)

started_at=$(date +%s)
case_limit=120
campaign_limit=600

run_case() {
  case_index=$1
  target=$2
  test_name=$3
  output=$(mktemp "$runner_temp/vmcell-reliability-case.XXXXXX")

  if ! (
    ulimit -f 2048
    exec timeout --kill-after=1s "$case_limit"s cargo test --locked --offline --test "$target" "$test_name" -- --list --ignored --exact
  ) >"$output" 2>&1 ||
    [ "$(grep -Fxc "$test_name: test" "$output")" -ne 1 ]; then
    rm -f -- "$output"
    printf 'reliability_case=%s status=failed\n' "$case_index" >&2
    exit 1
  fi
  rm -f -- "$output"

  output=$(mktemp "$runner_temp/vmcell-reliability-case.XXXXXX")
  if (
    ulimit -f 2048
    exec timeout --kill-after=1s "$case_limit"s cargo test --locked --offline --test "$target" "$test_name" -- --ignored --exact
  ) >"$output" 2>&1 &&
    grep -F 'running 1 test' "$output" >/dev/null &&
    grep -F 'test result: ok. 1 passed; 0 failed; 0 ignored;' "$output" >/dev/null; then
    rm -f -- "$output"
    printf 'reliability_case=%s status=passed\n' "$case_index"
    return
  fi

  rm -f -- "$output"
  printf 'reliability_case=%s status=failed\n' "$case_index" >&2
  exit 1
}

while IFS="$(printf '\t')" read -r case_index target test_name; do
  run_case "$case_index" "$target" "$test_name"
done < "$campaign_rows"
rm -f -- "$campaign_rows"
rmdir -- "$campaign_dir"
trap - EXIT HUP INT TERM

elapsed_seconds=$(( $(date +%s) - started_at ))
[ "$elapsed_seconds" -ge 0 ] && [ "$elapsed_seconds" -le "$campaign_limit" ] || {
  echo 'reliability campaign exceeded its bounded duration' >&2
  exit 1
}

umask 077
python3 - "$receipt_path" "$source_sha" "$manifest_sha256" "$elapsed_seconds" <<'PY'
import json
import os
import sys

path, source_sha, manifest_sha256, elapsed_seconds = sys.argv[1:]
receipt = {
    "schema_version": 1,
    "contract": "vmcell.reliability-extended-receipt.v1",
    "authorizing": False,
    "real_platform_acceptance": False,
    "support_promotion": "not_evaluated",
    "source_sha": source_sha,
    "rust_toolchain": "1.85.0",
    "campaign": {
        "contract": "vmcell.reliability-campaign.v1",
        "campaign_id": "r3-fixed-reliability-v1",
        "manifest_sha256": manifest_sha256,
        "case_count": 5,
        "elapsed_seconds": int(elapsed_seconds),
    },
    "result": "PASS",
}
encoded = (json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
try:
    with os.fdopen(descriptor, "wb", closefd=False) as handle:
        handle.write(encoded)
        handle.flush()
        os.fsync(handle.fileno())
finally:
    os.close(descriptor)
PY

python3 - "$receipt_path" "$source_sha" "$manifest_sha256" <<'PY'
import json
import sys

path, source_sha, manifest_sha256 = sys.argv[1:]
value = json.load(open(path, "r", encoding="utf-8"))
if value != {
    "schema_version": 1,
    "contract": "vmcell.reliability-extended-receipt.v1",
    "authorizing": False,
    "real_platform_acceptance": False,
    "support_promotion": "not_evaluated",
    "source_sha": source_sha,
    "rust_toolchain": "1.85.0",
    "campaign": {
        "contract": "vmcell.reliability-campaign.v1",
        "campaign_id": "r3-fixed-reliability-v1",
        "manifest_sha256": manifest_sha256,
        "case_count": 5,
        "elapsed_seconds": value["campaign"]["elapsed_seconds"],
    },
    "result": "PASS",
}:
    raise SystemExit("reliability receipt binding is invalid")
elapsed = value["campaign"]["elapsed_seconds"]
if not isinstance(elapsed, int) or elapsed < 0 or elapsed > 600:
    raise SystemExit("reliability receipt duration is invalid")
PY

printf 'contract=vmcell.reliability-extended-receipt.v1 source_sha=%s manifest_sha256=%s case_count=5 result=PASS\n' \
  "$source_sha" "$manifest_sha256"
