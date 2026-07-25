#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")"
rc=0
for t in test_*.sh; do
  [[ -e "$t" ]] || continue
  echo "== ${t}"
  bash "$t" || rc=1
done
exit $rc
