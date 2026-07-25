#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
BASE="${PHOTON_URL:-http://127.0.0.1:2322}"

body=$(curl -s --max-time 15 "$BASE/api?q=Bromo&limit=3" || echo '')
assert_contains "$body" "FeatureCollection" "geocode returns a FeatureCollection"

n=$(echo "$body" | jq '.features | length' 2>/dev/null || echo 0)
if (( n < 1 )); then
  echo "  FAIL: geocode 'Bromo' returned 0 features" >&2; FAILED=1
else
  echo "  ok: geocode 'Bromo' returned ${n} features"
fi

# Result must land inside Indonesia — catches an index that loaded but is the
# wrong region, which a mere 200 response would not.
lon=$(echo "$body" | jq -r '.features[0].geometry.coordinates[0]' 2>/dev/null || echo 0)
lat=$(echo "$body" | jq -r '.features[0].geometry.coordinates[1]' 2>/dev/null || echo 0)
in_box=$(awk -v lo="$lon" -v la="$lat" \
  'BEGIN{print (lo>94.5 && lo<141.1 && la>-11.1 && la<6.1) ? "yes":"no"}')
assert_eq "yes" "$in_box" "top result inside Indonesia bbox (${lat},${lon})"

# Fuzzy/multilingual handling is the whole reason Photon is here rather than a
# trigram query — assert it actually does the thing.
fuzzy=$(curl -s --max-time 15 "$BASE/api?q=gunung%20bromo&limit=1" || echo '')
assert_contains "$(echo "$fuzzy" | jq -r '.features[0].properties.name' 2>/dev/null)" \
  "Bromo" "lowercase local-language query resolves"

finish
