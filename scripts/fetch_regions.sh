#!/usr/bin/env bash
# Fetch several Geofabrik continent extracts, resumably and MD5-verified.
#
# Separate from fetch_osm.sh, which fetches the single region the serving stack is
# currently built around. This one stages the raw inputs for a multi-region build;
# it downloads only — nothing here imports, and nothing here touches the running
# stack's artifacts under $DATA_DIR/osm.
set -uo pipefail
: "${DATA_DIR:?}"

REGIONS="${REGIONS:-north-america south-america central-america africa asia}"
DEST="$DATA_DIR/osm-regions"
mkdir -p "$DEST"

# Refuse to start if the download could not finish. Continent PBFs are large and a
# full root filesystem would take the serving stack down with it.
need_gb=60
avail_gb=$(df -BG --output=avail "$DATA_DIR" | tail -1 | tr -dc '0-9')
if (( avail_gb < need_gb )); then
  echo "ERROR: ${avail_gb}G free, need >= ${need_gb}G for the continent extracts" >&2
  exit 1
fi
echo "$(date -Is) starting: ${REGIONS}  (${avail_gb}G free)"

rc=0
for region in $REGIONS; do
  slug="${region##*/}"
  url="https://download.geofabrik.de/${region}-latest.osm.pbf"
  out="$DEST/${slug}.osm.pbf"

  if [[ -s "$out" ]] && [[ -s "$out.md5" ]] \
     && (cd "$DEST" && md5sum -c --status "${slug}.osm.pbf.md5" 2>/dev/null); then
    echo "$(date -Is) ${slug}: already present and verified — skipping"
    continue
  fi

  echo "$(date -Is) ${slug}: downloading"
  # -C - resumes a partial file, so an interrupted run costs nothing.
  if ! curl -fL --retry 5 --retry-delay 10 -C - -o "$out" "$url"; then
    echo "$(date -Is) ${slug}: DOWNLOAD FAILED" >&2
    rc=1
    continue
  fi

  curl -fsSL --retry 3 -o "$out.md5" "${url}.md5" || true
  # Geofabrik's .md5 names the plain basename; check from inside the directory.
  if [[ -s "$out.md5" ]]; then
    sed -i "s|  .*|  ${slug}.osm.pbf|" "$out.md5"
    if (cd "$DEST" && md5sum -c --status "${slug}.osm.pbf.md5"); then
      echo "$(date -Is) ${slug}: OK  $(du -h "$out" | cut -f1)"
    else
      echo "$(date -Is) ${slug}: CHECKSUM MISMATCH — removing" >&2
      rm -f "$out"
      rc=1
    fi
  else
    echo "$(date -Is) ${slug}: no checksum published, keeping $(du -h "$out" | cut -f1)" >&2
  fi
done

echo "$(date -Is) finished (rc=${rc})"
du -sh "$DEST"
df -h "$DATA_DIR" | tail -1
exit $rc
