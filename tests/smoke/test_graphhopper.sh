#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
BASE="${GH_URL:-http://127.0.0.1:8989}"

# Surabaya -> Malang, ~90km through East Java.
PTS='[[112.7521,-7.2575],[112.6304,-7.9666]]'

assert_http_ok "$BASE/health" "graphhopper health"

plain=$(curl -s --max-time 60 -X POST "$BASE/route" -H 'Content-Type: application/json' \
  -d "{\"points\":$PTS,\"profile\":\"car\",\"points_encoded\":false}")
dist=$(echo "$plain" | jq -r '.paths[0].distance // 0' 2>/dev/null || echo 0)
if awk -v d="$dist" 'BEGIN{exit !(d>50000 && d<300000)}'; then
  echo "  ok: plain route ${dist}m"
else
  echo "  FAIL: plain route distance ${dist}, expected 50km-300km" >&2; FAILED=1
fi

# THE assertion this task exists for. A CH-only graph rejects a runtime
# custom_model; only an LM-prepared profile serves it. If profiles_lm is ever
# dropped from config/graphhopper.yml, this is what catches it.
custom=$(curl -s --max-time 60 -X POST "$BASE/route" -H 'Content-Type: application/json' -d "{
  \"points\":$PTS, \"profile\":\"car\", \"points_encoded\":false, \"ch.disable\":true,
  \"custom_model\": {\"priority\":[{\"if\":\"road_class == MOTORWAY\",\"multiply_by\":0.05}]}
}")
if echo "$custom" | jq -e '.paths[0].distance' >/dev/null 2>&1; then
  echo "  ok: runtime custom_model accepted (LM prepared)"
else
  echo "  FAIL: runtime custom_model rejected — profiles_lm missing?" >&2
  echo "        $(echo "$custom" | jq -c '{message,hints}' 2>/dev/null || echo "$custom" | head -c 200)" >&2
  FAILED=1
fi

# Avoiding motorways must actually change the route, not just be accepted.
d_custom=$(echo "$custom" | jq -r '.paths[0].distance // 0' 2>/dev/null || echo 0)
if awk -v a="$dist" -v b="$d_custom" 'BEGIN{exit !(a>0 && b>0 && (b>a*1.01 || b<a*0.99))}'; then
  echo "  ok: custom_model changed the route (${dist}m -> ${d_custom}m)"
else
  echo "  WARN: custom_model produced an identical route — may be legitimate if no motorway on this pair"
fi

# Elevation is what makes slope-aware scenic routing possible later; a 2-ordinate
# coordinate means the Skadi DEM was not picked up at import time.
ncoord=$(echo "$plain" | jq '.paths[0].points.coordinates[0] | length' 2>/dev/null || echo 0)
assert_eq "3" "$ncoord" "coordinates carry elevation (3 ordinates)"

asc=$(echo "$plain" | jq -r '.paths[0].ascend // 0' 2>/dev/null || echo 0)
if awk -v a="$asc" 'BEGIN{exit !(a>0)}'; then
  echo "  ok: ascend reported (${asc}m)"
else
  echo "  FAIL: ascend is ${asc} — elevation data not loaded" >&2; FAILED=1
fi

finish
