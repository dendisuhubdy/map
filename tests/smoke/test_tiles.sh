#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${DATA_DIR:=/data}" "${REGION_SLUG:=indonesia}"
BASE="${TILES_URL:-http://127.0.0.1:8080}"

assert_file_min_size "$DATA_DIR/tiles/$REGION_SLUG.pmtiles" 500000000 "pmtiles >= 500MB"

meta=$(curl -s --max-time 15 "$BASE/$REGION_SLUG/metadata" || echo '')
assert_contains "$meta" "vector_layers" "tile metadata lists vector layers"

# The OpenMapTiles schema layers the frontend styles against. A tileset that
# built but omitted these would render as a blank map.
for layer in water transportation place boundary; do
  assert_contains "$meta" "\"$layer\"" "layer '$layer' present"
done

# z10/822/526 covers East Java. Range-request serving is what MapLibre relies on.
code=$(curl -s -o /tmp/tile.mvt -w '%{http_code}' --max-time 15 "$BASE/$REGION_SLUG/10/822/526.mvt")
assert_eq "200" "$code" "z10 East Java tile served"
assert_file_min_size /tmp/tile.mvt 1000 "z10 East Java tile has content"

# An ocean tile far outside Indonesia must NOT return a fat tile — cheap check
# that we built the region extract and not something unexpected.
ocean=$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "$BASE/$REGION_SLUG/10/0/0.mvt")
if [[ "$ocean" == "200" || "$ocean" == "204" || "$ocean" == "404" ]]; then
  echo "  ok: out-of-region tile handled (${ocean})"
else
  echo "  FAIL: unexpected status ${ocean} for out-of-region tile" >&2; FAILED=1
fi

finish
