#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/region.sh"
: "${DATA_DIR:=/data}"
DEM="$DATA_DIR/dem"

count=$(find "$DEM" -name '*.hgt.gz' 2>/dev/null | wc -l | tr -d ' ')
if (( count < DEM_MIN_TILES )); then
  echo "  FAIL: only ${count} DEM tiles, expected >= ${DEM_MIN_TILES}" >&2; FAILED=1
else
  echo "  ok: ${count} DEM tiles present"
fi

# A high-relief land tile inside the region — the routing smoke test drives near it.
assert_eq "0" "$(test -f "$DEM/$DEM_BAND/$DEM_TILE" && echo 0 || echo 1)" "$DEM_TILE present"
assert_file_min_size "$DEM/$DEM_BAND/$DEM_TILE" 100000 "$DEM_TILE is a real tile, not an error page"

# Every file must be real gzip, not a truncated download or an HTML 404 body.
bad=$(find "$DEM" -name '*.hgt.gz' -size -1k 2>/dev/null | wc -l | tr -d ' ')
assert_eq "0" "$bad" "no truncated tiles"
assert_eq "0" "$(find "$DEM" -name '*.part' 2>/dev/null | wc -l | tr -d ' ')" "no leftover .part files"

finish
