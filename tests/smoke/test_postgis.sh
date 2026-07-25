#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${POSTGRES_USER:=map}" "${POSTGRES_DB:=map}"

q() { docker compose exec -T postgis psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "$1" 2>/dev/null; }

assert_eq "1" "$(q 'SELECT 1')" "postgis reachable"
assert_eq "t" "$(q "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='postgis')")" "postgis extension installed"

# Category counts the agent actually depends on. Floors are deliberately loose —
# they catch an empty or half-finished import, not OSM data drift.
for pair in "volcano:50" "beach:100"; do
  tag="${pair%%:*}"; floor="${pair##*:}"
  n=$(q "SELECT count(*) FROM osm_poi WHERE tags->>'natural' = '${tag}'")
  n=${n:-0}
  if (( n < floor )); then
    echo "  FAIL: only ${n} ${tag}s, expected >= ${floor}" >&2; FAILED=1
  else
    echo "  ok: ${n} ${tag}s in osm_poi"
  fi
done

places=$(q "SELECT count(*) FROM osm_place"); places=${places:-0}
if (( places < 1000 )); then
  echo "  FAIL: only ${places} rows in osm_place, expected >= 1000" >&2; FAILED=1
else
  echo "  ok: ${places} rows in osm_place"
fi

# The exact query shape search_poi will issue must hit the GiST index. A seq scan
# here still returns correct rows, so only EXPLAIN catches the regression.
plan=$(q "EXPLAIN SELECT * FROM osm_poi WHERE geom && ST_MakeEnvelope(112,-8,113,-7,4326) AND tags ? 'natural'")
assert_contains "$plan" "Index" "bbox+tag query uses an index"

finish
