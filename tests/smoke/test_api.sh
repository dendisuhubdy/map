#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
BASE="${API_URL:-http://127.0.0.1:8000}"

assert_http_ok "$BASE/healthz" "api healthz"

health=$(curl -s --max-time 10 "$BASE/api/health" || echo '{}')
assert_eq "true" "$(echo "$health" | jq -r '.postgis')" "api reached postgis"

# Design spec §13 layer 1: each tool against its real backing service, no mocking.
tool() {
  curl -s --max-time 90 -X POST "$BASE/api/tool" \
    -H 'Content-Type: application/json' -d "$1"
}

# --- geocode / Photon -------------------------------------------------------
g=$(tool '{"name":"geocode","input":{"query":"Bromo","bbox":null}}')
n=$(echo "$g" | jq -r '.result.candidates | length' 2>/dev/null || echo 0)
if [[ "$n" -ge 1 ]]; then
  echo "  ok: geocode returned ${n} candidates"
else
  echo "  FAIL: geocode returned no candidates — $(echo "$g" | head -c 200)" >&2; FAILED=1
fi
lat=$(echo "$g" | jq -r '.result.candidates[0].lat')
lon=$(echo "$g" | jq -r '.result.candidates[0].lon')
in_box=$(awk -v lo="$lon" -v la="$lat" 'BEGIN{print (lo>94.5 && lo<141.1 && la>-11.1 && la<6.1) ? "yes":"no"}')
assert_eq "yes" "$in_box" "geocode result inside Indonesia (${lat},${lon})"

# --- search_poi / PostGIS ---------------------------------------------------
p=$(tool '{"name":"search_poi","input":{"tags":["natural=volcano"],"bbox":[112.0,-8.5,114.0,-7.0],"limit":20}}')
c=$(echo "$p" | jq -r '.result.count' 2>/dev/null || echo 0)
if [[ "$c" -ge 1 ]]; then
  echo "  ok: search_poi found ${c} volcanoes in East Java"
else
  echo "  FAIL: search_poi found none — $(echo "$p" | head -c 200)" >&2; FAILED=1
fi

# A tag that matches nothing is an empty result, NOT an error — the agent widens
# the search rather than treating it as a failure (spec §12).
z=$(tool '{"name":"search_poi","input":{"tags":["natural=glacier"],"bbox":[112.0,-8.5,113.0,-7.0],"limit":10}}')
assert_eq "true" "$(echo "$z" | jq -r '.ok')" "empty POI result is not an error"

# --- route / GraphHopper ----------------------------------------------------
r=$(tool '{"name":"route","input":{"waypoints":[[112.7521,-7.2575],[112.6304,-7.9666]],"profile":"car","custom_model":null}}')
d=$(echo "$r" | jq -r '.result.distance_m // 0')
if awk -v d="$d" 'BEGIN{exit !(d>50000 && d<300000)}'; then
  echo "  ok: route Surabaya->Malang ${d}m"
else
  echo "  FAIL: route distance ${d} out of range — $(echo "$r" | head -c 200)" >&2; FAILED=1
fi
asc=$(echo "$r" | jq -r '.result.ascend_m // 0')
if awk -v a="$asc" 'BEGIN{exit !(a>0)}'; then
  echo "  ok: route carries elevation (ascend ${asc}m)"
else
  echo "  FAIL: route ascend is ${asc} — elevation not requested?" >&2; FAILED=1
fi

# THE assertion the LM preparation exists for: a runtime custom_model is accepted.
cm=$(tool '{"name":"route","input":{"waypoints":[[112.7521,-7.2575],[112.6304,-7.9666]],"profile":"car","custom_model":{"priority":[{"if":"road_class == PRIMARY","multiply_by":0.02}]}}}')
assert_eq "true" "$(echo "$cm" | jq -r '.ok')" "route accepts a runtime custom_model"
d2=$(echo "$cm" | jq -r '.result.distance_m // 0')
if awk -v a="$d" -v b="$d2" 'BEGIN{exit !(a>0 && b>0 && (b>a*1.005 || b<a*0.995))}'; then
  echo "  ok: custom_model changed the route (${d}m -> ${d2}m)"
else
  echo "  FAIL: custom_model was accepted but did not alter the route" >&2; FAILED=1
fi

# --- elevation_profile ------------------------------------------------------
e=$(tool '{"name":"elevation_profile","input":{"waypoints":[[112.7521,-7.2575],[112.6304,-7.9666]],"samples":12}}')
s=$(echo "$e" | jq -r '.result.samples | length' 2>/dev/null || echo 0)
assert_eq "12" "$s" "elevation_profile returned 12 samples"
mx=$(echo "$e" | jq -r '.result.max_elevation_m // 0')
if awk -v m="$mx" 'BEGIN{exit !(m>50)}'; then
  echo "  ok: elevation profile peaks at ${mx}m"
else
  echo "  FAIL: max elevation ${mx} is implausible for this route" >&2; FAILED=1
fi

# --- error handling ---------------------------------------------------------
bad=$(tool '{"name":"route","input":{"waypoints":[[-7.25,112.75],[-7.96,112.63]],"profile":"car","custom_model":null}}')
assert_eq "false" "$(echo "$bad" | jq -r '.ok')" "reversed lat/lon is rejected"

unk=$(tool '{"name":"teleport","input":{}}')
assert_eq "false" "$(echo "$unk" | jq -r '.ok')" "unknown tool is an error, not a crash"

finish
