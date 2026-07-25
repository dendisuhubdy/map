#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${DATA_DIR:=/data}" "${REGION_SLUG:=indonesia}"
PBF="$DATA_DIR/osm/$REGION_SLUG-latest.osm.pbf"

assert_file_min_size "$PBF" 500000000 "PBF present and >= 500MB"

# An OSM PBF opens with a 4-byte big-endian BlobHeader length followed by a
# protobuf whose type string is "OSMHeader". Cheapest possible integrity check
# that the file is a PBF and not an HTML error page.
assert_contains "$(head -c 32 "$PBF" | tr -dc '[:print:]')" "OSMHeader" "PBF has valid OSMHeader magic"

# The checksum recorded at download time must still match what is on disk.
MD5F="$DATA_DIR/osm/$REGION_SLUG-latest.osm.pbf.md5"
if [[ -f "$MD5F" ]]; then
  expected=$(awk '{print $1}' "$MD5F")
  actual=$(md5sum "$PBF" | awk '{print $1}')
  assert_eq "$expected" "$actual" "PBF matches recorded MD5"
else
  echo "  FAIL: no .md5 alongside the PBF" >&2; FAILED=1
fi

finish
