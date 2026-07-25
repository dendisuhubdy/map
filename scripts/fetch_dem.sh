#!/usr/bin/env bash
# Pre-fetch Skadi elevation tiles covering $REGION_BBOX.
#
# GraphHopper can download these on demand during import, but pre-fetching makes
# the graph build reproducible and re-runnable offline. Ocean squares have no
# tile and legitimately 404 — that is normal, not an error.
set -euo pipefail
: "${REGION_BBOX:?}" "${DATA_DIR:?}"

read -r MIN_LON MIN_LAT MAX_LON MAX_LAT <<< "$REGION_BBOX"

DEST="$DATA_DIR/dem"
BASE="https://s3.amazonaws.com/elevation-tiles-prod/skadi"
mkdir -p "$DEST"

# Skadi tiles are 1x1 degree, named by their SOUTH-WEST corner. Flooring is what
# maps a fractional bbox edge onto the tile that contains it; awk's int()
# truncates toward zero, which is wrong for negative latitudes (int(-11.1) = -11
# but the containing tile is -12), hence the explicit adjustment.
floor() { awk -v v="$1" 'BEGIN{ i=int(v); if (v<0 && v!=i) i--; print i }'; }

lat_start=$(floor "$MIN_LAT"); lat_end=$(floor "$MAX_LAT")
lon_start=$(floor "$MIN_LON"); lon_end=$(floor "$MAX_LON")

got=0; missing=0; cached=0
for (( lat=lat_start; lat<=lat_end; lat++ )); do
  if (( lat < 0 )); then
    band=$(printf 'S%02d' $(( -lat )))
  else
    band=$(printf 'N%02d' "$lat")
  fi
  mkdir -p "$DEST/$band"

  for (( lon=lon_start; lon<=lon_end; lon++ )); do
    if (( lon < 0 )); then
      lonp=$(printf 'W%03d' $(( -lon )))
    else
      lonp=$(printf 'E%03d' "$lon")
    fi

    tile="${band}${lonp}.hgt.gz"
    out="$DEST/$band/$tile"

    if [[ -s "$out" ]]; then cached=$(( cached + 1 )); continue; fi

    if curl -fsL --retry 2 --max-time 120 -o "${out}.part" "$BASE/$band/$tile" 2>/dev/null; then
      mv "${out}.part" "$out"; got=$(( got + 1 ))
    else
      rm -f "${out}.part"; missing=$(( missing + 1 ))
    fi
  done
done

echo "DEM: ${got} fetched, ${cached} already present, ${missing} absent (ocean)"
echo "DEM: total $(du -sh "$DEST" | cut -f1) across $(find "$DEST" -name '*.hgt.gz' | wc -l) tiles"
