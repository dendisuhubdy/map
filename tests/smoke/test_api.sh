#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/region.sh"
BASE="${API_URL:-http://127.0.0.1:8000}"

assert_http_ok "$BASE/healthz" "api healthz"

health=$(curl -s --max-time 10 "$BASE/api/health" || echo '{}')
assert_eq "true" "$(echo "$health" | jq -r '.postgis')" "api reached postgis"

# Design spec §13 layer 1: each tool against its real backing service, no mocking.
tool() {
  curl -s --max-time 120 -X POST "$BASE/api/tool" \
    -H 'Content-Type: application/json' -d "$1"
}

# --- geocode / Photon -------------------------------------------------------
g=$(tool "{\"name\":\"geocode\",\"input\":{\"query\":\"${GEO_QUERY}\",\"bbox\":null}}")
n=$(echo "$g" | jq -r '.result.candidates | length' 2>/dev/null || echo 0)
if [[ "$n" -ge 1 ]]; then
  echo "  ok: geocode '${GEO_QUERY}' returned ${n} candidates"
else
  echo "  FAIL: geocode returned no candidates — $(echo "$g" | head -c 200)" >&2; FAILED=1
fi
lat=$(echo "$g" | jq -r '.result.candidates[0].lat')
lon=$(echo "$g" | jq -r '.result.candidates[0].lon')
in_box=$(awk -v lo="$lon" -v la="$lat" \
  -v w="$GEO_MIN_LON" -v s="$GEO_MIN_LAT" -v e="$GEO_MAX_LON" -v nn="$GEO_MAX_LAT" \
  'BEGIN{print (lo>w && lo<e && la>s && la<nn) ? "yes":"no"}')
assert_eq "yes" "$in_box" "geocode result inside ${REGION_NAME} (${lat},${lon})"

# --- search_poi / PostGIS ---------------------------------------------------
p=$(tool "{\"name\":\"search_poi\",\"input\":{\"tags\":[\"${POI_TAG_A}\"],\"bbox\":[${GEO_MIN_LON},${GEO_MIN_LAT},${GEO_MAX_LON},${GEO_MAX_LAT}],\"limit\":20}}")
c=$(echo "$p" | jq -r '.result.count' 2>/dev/null || echo 0)
if [[ "$c" -ge 1 ]]; then
  echo "  ok: search_poi found ${c} ${POI_TAG_A}"
else
  echo "  FAIL: search_poi found none — $(echo "$p" | head -c 200)" >&2; FAILED=1
fi

# A tag that matches nothing is an empty result, NOT an error — the agent widens
# the search rather than treating it as a failure (spec §12).
z=$(tool "{\"name\":\"search_poi\",\"input\":{\"tags\":[\"natural=glacier_nonexistent\"],\"bbox\":[${GEO_MIN_LON},${GEO_MIN_LAT},${GEO_MAX_LON},${GEO_MAX_LAT}],\"limit\":10}}")
assert_eq "true" "$(echo "$z" | jq -r '.ok')" "empty POI result is not an error"

# --- route / GraphHopper ----------------------------------------------------
WP="[[${ROUTE_FROM_LON},${ROUTE_FROM_LAT}],[${ROUTE_TO_LON},${ROUTE_TO_LAT}]]"
r=$(tool "{\"name\":\"route\",\"input\":{\"waypoints\":${WP},\"profile\":\"car\",\"custom_model\":null}}")
d=$(echo "$r" | jq -r '.result.distance_m // 0')
if awk -v d="$d" -v lo="$ROUTE_MIN_M" -v hi="$ROUTE_MAX_M" 'BEGIN{exit !(d>lo && d<hi)}'; then
  echo "  ok: route ${d}m"
else
  echo "  FAIL: route distance ${d} outside [${ROUTE_MIN_M}, ${ROUTE_MAX_M}] — $(echo "$r" | head -c 200)" >&2; FAILED=1
fi
asc=$(echo "$r" | jq -r '.result.ascend_m // 0')
if awk -v a="$asc" 'BEGIN{exit !(a>0)}'; then
  echo "  ok: route carries elevation (ascend ${asc}m)"
else
  echo "  FAIL: route ascend is ${asc} — elevation not requested?" >&2; FAILED=1
fi

# THE assertion the LM preparation exists for: a runtime custom_model is accepted.
cm=$(tool "{\"name\":\"route\",\"input\":{\"waypoints\":${WP},\"profile\":\"car\",\"custom_model\":{\"priority\":[{\"if\":\"road_class == MOTORWAY\",\"else_if\":null,\"else\":null,\"multiply_by\":0.02,\"limit_to\":null}],\"speed\":null,\"distance_influence\":90}}}")
assert_eq "true" "$(echo "$cm" | jq -r '.ok')" "route accepts a runtime custom_model"
d2=$(echo "$cm" | jq -r '.result.distance_m // 0')
if awk -v a="$d" -v b="$d2" 'BEGIN{exit !(a>0 && b>0 && (b>a*1.005 || b<a*0.995))}'; then
  echo "  ok: custom_model changed the route (${d}m -> ${d2}m)"
else
  echo "  FAIL: custom_model was accepted but did not alter the route" >&2; FAILED=1
fi

# A distance_influence under the landmark floor must be clamped and reported,
# not rejected — GraphHopper cannot serve anything below it.
lowdi=$(tool "{\"name\":\"route\",\"input\":{\"waypoints\":${WP},\"profile\":\"car\",\"custom_model\":{\"priority\":null,\"speed\":null,\"distance_influence\":40}}}")
assert_eq "true" "$(echo "$lowdi" | jq -r '.ok')" "distance_influence below the floor still routes"
assert_contains "$(echo "$lowdi" | jq -r '.result.note // ""')" "90" "the clamp is reported back"

# --- elevation_profile ------------------------------------------------------
e=$(tool "{\"name\":\"elevation_profile\",\"input\":{\"waypoints\":${WP},\"samples\":12}}")
s=$(echo "$e" | jq -r '.result.samples | length' 2>/dev/null || echo 0)
assert_eq "12" "$s" "elevation_profile returned 12 samples"
mx=$(echo "$e" | jq -r '.result.max_elevation_m // 0')
if awk -v m="$mx" 'BEGIN{exit !(m>50)}'; then
  echo "  ok: elevation profile peaks at ${mx}m"
else
  echo "  FAIL: max elevation ${mx} is implausible for this route" >&2; FAILED=1
fi

# --- error handling ---------------------------------------------------------
bad=$(tool "{\"name\":\"route\",\"input\":{\"waypoints\":[[${ROUTE_FROM_LAT},${ROUTE_FROM_LON}],[${ROUTE_TO_LAT},${ROUTE_TO_LON}]],\"profile\":\"car\",\"custom_model\":null}}")
assert_eq "false" "$(echo "$bad" | jq -r '.ok')" "reversed lat/lon is rejected"

unk=$(tool '{"name":"teleport","input":{}}')
assert_eq "false" "$(echo "$unk" | jq -r '.ok')" "unknown tool is an error, not a crash"

finish
