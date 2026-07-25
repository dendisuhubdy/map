#!/usr/bin/env bash
# Fetch the Photon geocoding index and the pinned Photon JAR.
#
# Version is pinned to 0.7.4 (the Elasticsearch build) — see
# docs/decisions/photon-index-source.md. The published country extract is an
# Elasticsearch index and does NOT load on the 1.x/OpenSearch line.
set -euo pipefail
: "${PHOTON_COUNTRY:?}" "${DATA_DIR:?}"

PHOTON_VERSION="${PHOTON_VERSION:-0.7.4}"
PHOTON_DUMP_DATE="${PHOTON_DUMP_DATE:-250720}"

DEST="$DATA_DIR/photon"
DUMP="photon-db-${PHOTON_COUNTRY}-${PHOTON_DUMP_DATE}.tar.bz2"
DUMP_URL="https://download1.graphhopper.com/public/extracts/by-country-code/${PHOTON_COUNTRY}/${DUMP}"
JAR="photon-${PHOTON_VERSION}.jar"
JAR_URL="https://github.com/komoot/photon/releases/download/${PHOTON_VERSION}/${JAR}"

mkdir -p "$DEST"
cd "$DEST"

if [[ -f "$JAR" ]]; then
  echo "JAR already present: $JAR"
else
  echo "Downloading $JAR_URL"
  curl -fL --retry 3 --no-progress-meter -o "${JAR}.part" "$JAR_URL"
  mv "${JAR}.part" "$JAR"
fi

if [[ -d photon_data ]]; then
  echo "Index already present: $DEST/photon_data ($(du -sh photon_data | cut -f1))"
  exit 0
fi

echo "Downloading $DUMP_URL"
curl -fL --retry 3 --no-progress-meter -O "$DUMP_URL"
curl -fL --retry 3 --no-progress-meter -O "${DUMP_URL}.md5"
md5sum -c "${DUMP}.md5"

echo "Extracting (pbzip2)"
pbzip2 -cd "$DUMP" | tar x
rm -f "$DUMP" "${DUMP}.md5"

# The 0.7.x extract must present an Elasticsearch index; anything else means the
# upstream dump changed shape and the pinned JAR will not load it.
if [[ ! -d photon_data/elasticsearch ]]; then
  echo "ERROR: expected photon_data/elasticsearch — upstream dump layout changed." >&2
  echo "Re-open docs/decisions/photon-index-source.md before proceeding." >&2
  exit 1
fi

echo "OK: $DEST/photon_data ($(du -sh photon_data | cut -f1))"
