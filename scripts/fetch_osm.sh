#!/usr/bin/env bash
# Download the Geofabrik extract for $OSM_REGION and verify its MD5.
set -euo pipefail
: "${OSM_REGION:?}" "${REGION_SLUG:?}" "${DATA_DIR:?}"

DEST="$DATA_DIR/osm"
BASE="https://download.geofabrik.de/${OSM_REGION}-latest.osm.pbf"
PBF="$DEST/${REGION_SLUG}-latest.osm.pbf"
MD5F="${PBF}.md5"
mkdir -p "$DEST"

# Always refresh the checksum first — it is tiny and tells us whether the
# local copy is still current. Geofabrik regenerates extracts daily.
curl -fL --retry 3 -sS -o "$MD5F.new" "${BASE}.md5"
EXPECTED=$(awk '{print $1}' "$MD5F.new")

if [[ -f "$PBF" ]]; then
  ACTUAL=$(md5sum "$PBF" | awk '{print $1}')
  if [[ "$EXPECTED" == "$ACTUAL" ]]; then
    mv "$MD5F.new" "$MD5F"
    echo "Already current: $PBF ($(du -h "$PBF" | cut -f1))"
    exit 0
  fi
  echo "Local copy is stale — re-downloading"
fi

echo "Downloading ${BASE}"
curl -fL --retry 3 --progress-bar -o "${PBF}.part" "$BASE"

ACTUAL=$(md5sum "${PBF}.part" | awk '{print $1}')
if [[ "$EXPECTED" != "$ACTUAL" ]]; then
  echo "Checksum mismatch: expected $EXPECTED got $ACTUAL" >&2
  rm -f "${PBF}.part" "$MD5F.new"
  exit 1
fi

mv "${PBF}.part" "$PBF"
mv "$MD5F.new" "$MD5F"
echo "OK: $PBF ($(du -h "$PBF" | cut -f1))"
