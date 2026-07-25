#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${DATA_DIR:=/data}"
DEM="$DATA_DIR/dem"

count=$(find "$DEM" -name '*.hgt.gz' 2>/dev/null | wc -l | tr -d ' ')
if (( count < 200 )); then
  echo "  FAIL: only ${count} DEM tiles, expected >= 200" >&2; FAILED=1
else
  echo "  ok: ${count} DEM tiles present"
fi

# East Java, covering Bromo — the area the routing smoke test drives through.
assert_eq "0" "$(test -f "$DEM/S08/S08E112.hgt.gz" && echo 0 || echo 1)" "S08E112 (East Java) present"
assert_file_min_size "$DEM/S08/S08E112.hgt.gz" 100000 "S08E112 is a real tile, not an error page"

# Southern-hemisphere naming is the bug this guards. bbox min_lat is -11.1;
# awk int() truncates toward zero (-11), so a naive floor never reaches S12 and
# silently drops the southernmost row of tiles. Correct flooring yields -12.
assert_eq "0" "$(test -d "$DEM/S12" && echo 0 || echo 1)" "S12 band exists (negative-latitude floor correct)"

# Every file must be real gzip, not a truncated download or an HTML 404 body.
bad=$(find "$DEM" -name '*.hgt.gz' -size -1k 2>/dev/null | wc -l | tr -d ' ')
assert_eq "0" "$bad" "no truncated tiles"
assert_eq "0" "$(find "$DEM" -name '*.part' 2>/dev/null | wc -l | tr -d ' ')" "no leftover .part files"

finish
