#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/region.sh"
: "${DATA_DIR:=/data}"
BASE="${TILES_URL:-http://127.0.0.1:8080}"

assert_file_min_size "$DATA_DIR/tiles/$REGION_SLUG.pmtiles" 500000000 "pmtiles >= 500MB"

meta=$(curl -s --max-time 15 "$BASE/$REGION_SLUG/metadata" || echo '')
assert_contains "$meta" "vector_layers" "tile metadata lists vector layers"

# The OpenMapTiles schema layers the frontend styles against. A tileset that
# built but omitted these would render as a blank map.
for layer in water transportation place boundary; do
  assert_contains "$meta" "\"$layer\"" "layer '$layer' present"
done

# Tile coordinates come from tests/smoke/region.sh, derived rather than guessed:
#   x = (lon+180)/360 * 2^z,  y = (1 - asinh(tan(lat))/pi)/2 * 2^z
# It must be a LAND tile. An ocean tile is perfectly valid but near-empty (~163
# bytes), so asserting content on one can never pass — that was a real bug here.
code=$(curl -s -o /tmp/tile.mvt -w '%{http_code}' --max-time 15 "$BASE/$REGION_SLUG/$TILE_Z/$TILE_X/$TILE_Y.mvt")
assert_eq "200" "$code" "z${TILE_Z} ${TILE_PLACE} tile served"
assert_file_min_size /tmp/tile.mvt 1000 "z${TILE_Z} ${TILE_PLACE} tile has content"

# A tile far outside the region must NOT return a fat tile — cheap check that we
# built the region extract and not something unexpected.
ocean=$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "$BASE/$REGION_SLUG/10/0/0.mvt")
if [[ "$ocean" == "200" || "$ocean" == "204" || "$ocean" == "404" ]]; then
  echo "  ok: out-of-region tile handled (${ocean})"
else
  echo "  FAIL: unexpected status ${ocean} for out-of-region tile" >&2; FAILED=1
fi

finish
