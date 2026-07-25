#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/region.sh"
BASE="${PHOTON_URL:-http://127.0.0.1:2322}"

body=$(curl -s --max-time 15 "$BASE/api?q=$(echo "$GEO_QUERY" | tr ' ' '+')&limit=3" || echo '')
assert_contains "$body" "FeatureCollection" "geocode returns a FeatureCollection"

n=$(echo "$body" | jq '.features | length' 2>/dev/null || echo 0)
if (( n < 1 )); then
  echo "  FAIL: geocode '${GEO_QUERY}' returned 0 features" >&2; FAILED=1
else
  echo "  ok: geocode '${GEO_QUERY}' returned ${n} features"
fi

# The result must land inside the served region — catches an index that loaded
# fine but is the wrong region, which a mere 200 response would not.
lon=$(echo "$body" | jq -r '.features[0].geometry.coordinates[0]' 2>/dev/null || echo 0)
lat=$(echo "$body" | jq -r '.features[0].geometry.coordinates[1]' 2>/dev/null || echo 0)
in_box=$(awk -v lo="$lon" -v la="$lat" \
  -v w="$GEO_MIN_LON" -v s="$GEO_MIN_LAT" -v e="$GEO_MAX_LON" -v nn="$GEO_MAX_LAT" \
  'BEGIN{print (lo>w && lo<e && la>s && la<nn) ? "yes":"no"}')
assert_eq "yes" "$in_box" "top result inside ${REGION_NAME} bbox (${lat},${lon})"

# Fuzzy, case-insensitive handling is the whole reason Photon is here rather than
# a trigram query — assert it actually does the thing.
fuzzy=$(curl -s --max-time 15 "$BASE/api?q=$(echo "$GEO_QUERY_2" | tr ' ' '+')&limit=1" || echo '')
fn=$(echo "$fuzzy" | jq '.features | length' 2>/dev/null || echo 0)
if (( fn < 1 )); then
  echo "  FAIL: lowercase query '${GEO_QUERY_2}' returned nothing" >&2; FAILED=1
else
  echo "  ok: lowercase query '${GEO_QUERY_2}' resolves"
fi

finish
