#!/usr/bin/env bash
# Region-specific expectations for the smoke suite.
#
# Everything here is a fact about the *area being served*, not about the software.
# Keeping it in one file is what lets `make verify` follow a region switch: change
# REGION_SLUG in .env, add a block here, and the whole suite retargets. Without
# this the assertions silently keep testing the previous region's landmarks.
#
# Tile coordinates are derived, not guessed:
#   x = (lon + 180) / 360 * 2^z
#   y = (1 - asinh(tan(lat)) / pi) / 2 * 2^z

: "${REGION_SLUG:=california}"

case "$REGION_SLUG" in
  california)
    REGION_NAME="California"
    # Geocoder probe: a landmark the index must resolve, and the box it must fall in.
    GEO_QUERY="Yosemite"
    GEO_MIN_LON=-124.5; GEO_MIN_LAT=32.5; GEO_MAX_LON=-114.1; GEO_MAX_LAT=42.1
    # A second query in a different register — checks the index has more than parks.
    GEO_QUERY_2="san francisco"
    # A land tile with real content. z10/163/395 covers San Francisco.
    TILE_Z=10; TILE_X=163; TILE_Y=395; TILE_PLACE="San Francisco"
    # Skadi tile over the Sierra Nevada — guaranteed non-ocean, high relief.
    DEM_BAND="N37"; DEM_TILE="N37W120.hgt.gz"
    # Minimum DEM tiles for the bbox. 11 lon x 10 lat, minus Pacific ocean cells.
    DEM_MIN_TILES=60
    # POI floors. Conservative, and verified against the real import.
    POI_TAG_A="natural=peak";  POI_MIN_A=200
    POI_TAG_B="natural=beach"; POI_MIN_B=50
    # Routing probe: San Francisco -> Los Angeles, ~600 km by road.
    ROUTE_FROM_LON=-122.4194; ROUTE_FROM_LAT=37.7749
    ROUTE_TO_LON=-118.2437;   ROUTE_TO_LAT=34.0522
    ROUTE_MIN_M=500000; ROUTE_MAX_M=900000
    ;;
  indonesia)
    REGION_NAME="Indonesia"
    GEO_QUERY="Bromo"
    GEO_MIN_LON=94.5; GEO_MIN_LAT=-11.1; GEO_MAX_LON=141.1; GEO_MAX_LAT=6.1
    GEO_QUERY_2="surabaya"
    TILE_Z=10; TILE_X=832; TILE_Y=532; TILE_PLACE="East Java"
    DEM_BAND="S08"; DEM_TILE="S08E112.hgt.gz"
    DEM_MIN_TILES=200
    POI_TAG_A="natural=volcano"; POI_MIN_A=50
    POI_TAG_B="natural=beach";   POI_MIN_B=100
    ROUTE_FROM_LON=112.7521; ROUTE_FROM_LAT=-7.2575
    ROUTE_TO_LON=112.6304;   ROUTE_TO_LAT=-7.9666
    ROUTE_MIN_M=50000; ROUTE_MAX_M=300000
    ;;
  *)
    echo "  FAIL: no smoke-test expectations defined for REGION_SLUG='${REGION_SLUG}'." >&2
    echo "        Add a block to tests/smoke/region.sh — the suite cannot verify" >&2
    echo "        a region it has no landmarks for." >&2
    exit 1
    ;;
esac

# Split "key=value" into the two halves the SQL and jq filters need.
POI_KEY_A="${POI_TAG_A%%=*}"; POI_VAL_A="${POI_TAG_A#*=}"
POI_KEY_B="${POI_TAG_B%%=*}"; POI_VAL_B="${POI_TAG_B#*=}"
