#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/region.sh"
: "${POSTGRES_USER:=map}" "${POSTGRES_DB:=map}"

q() { docker compose exec -T postgis psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "$1" 2>/dev/null; }

assert_eq "1" "$(q 'SELECT 1')" "postgis reachable"
assert_eq "t" "$(q "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='postgis')")" "postgis extension installed"

# Category counts the agent actually depends on. Floors are deliberately loose —
# they catch an empty or half-finished import, not OSM data drift.
for spec in "$POI_KEY_A:$POI_VAL_A:$POI_MIN_A" "$POI_KEY_B:$POI_VAL_B:$POI_MIN_B"; do
  key="${spec%%:*}"; rest="${spec#*:}"; val="${rest%%:*}"; floor="${rest##*:}"
  n=$(q "SELECT count(*) FROM osm_poi WHERE tags->>'${key}' = '${val}'")
  n=${n:-0}
  if (( n < floor )); then
    echo "  FAIL: only ${n} ${key}=${val}, expected >= ${floor}" >&2; FAILED=1
  else
    echo "  ok: ${n} ${key}=${val} in osm_poi"
  fi
done

places=$(q "SELECT count(*) FROM osm_place"); places=${places:-0}
if (( places < 1000 )); then
  echo "  FAIL: only ${places} rows in osm_place, expected >= 1000" >&2; FAILED=1
else
  echo "  ok: ${places} rows in osm_place"
fi

# The exact query shape search_poi will issue must hit the GiST index. A seq scan
# here still returns correct rows, so only EXPLAIN catches the regression. The
# envelope is a 1-degree box inside the served region.
plan=$(q "EXPLAIN SELECT * FROM osm_poi WHERE geom && ST_MakeEnvelope(${GEO_MIN_LON}, ${GEO_MIN_LAT}, $(awk -v v="$GEO_MIN_LON" 'BEGIN{print v+1}'), $(awk -v v="$GEO_MIN_LAT" 'BEGIN{print v+1}'), 4326) AND tags ? '${POI_KEY_A}'")
assert_contains "$plan" "Index" "bbox+tag query uses an index"

finish
