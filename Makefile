SHELL := /bin/bash

# Tolerant include: `make` should give a useful error from the scripts' own
# `${VAR:?}` guards rather than hard-failing before .env has been created.
-include .env
export

.PHONY: verify fetch photon db dem graph tiles all up down

verify:
	@bash tests/smoke/run_all.sh

fetch:
	@bash scripts/fetch_osm.sh

photon:
	@bash scripts/fetch_photon.sh
	docker compose up -d photon

db:
	docker compose up -d postgis
	@echo "waiting for postgres to accept TCP..."
	@# NOT `docker compose exec pg_isready`: during first-run initdb the entrypoint
	@# runs a temporary server on a unix socket only, so the in-container check
	@# passes while the TCP listener osm2pgsql needs is still down. Probe the real path.
	@for i in $$(seq 1 90); do \
	  pg_isready -h 127.0.0.1 -p 5432 -U $(POSTGRES_USER) -d $(POSTGRES_DB) >/dev/null 2>&1 && break; \
	  sleep 2; \
	done
	@pg_isready -h 127.0.0.1 -p 5432 -U $(POSTGRES_USER) -d $(POSTGRES_DB) >/dev/null 2>&1 \
	  || { echo "postgres did not accept TCP in 180s" >&2; exit 1; }
	PGPASSWORD=$(POSTGRES_PASSWORD) osm2pgsql -O flex -S config/osm2pgsql.lua \
	  -H 127.0.0.1 -P 5432 -U $(POSTGRES_USER) -d $(POSTGRES_DB) \
	  --slim --drop -C 6000 \
	  $(DATA_DIR)/osm/$(REGION_SLUG)-latest.osm.pbf
	docker compose exec -T postgis psql -U $(POSTGRES_USER) -d $(POSTGRES_DB) -c \
	  "CREATE INDEX IF NOT EXISTS osm_poi_geom_idx ON osm_poi USING GIST (geom); \
	   CREATE INDEX IF NOT EXISTS osm_poi_tags_idx ON osm_poi USING GIN (tags); \
	   CREATE INDEX IF NOT EXISTS osm_place_geom_idx ON osm_place USING GIST (geom); \
	   ANALYZE osm_poi; ANALYZE osm_place;"

dem:
	@bash scripts/fetch_dem.sh

graph:
	@echo "stopping serving stack — the import wants the RAM"
	docker compose stop photon postgis graphhopper || true
	@# A re-import must start from an empty graph dir; GH will otherwise reuse whatever
	@# a previous, possibly half-finished run left behind.
	rm -rf $(DATA_DIR)/graph
	@# NOT `docker compose run`: that inherits the service's `mem_limit: 7g`, which is the
	@# *serving* budget. The import additionally holds the mmap'd Skadi DEM in page cache,
	@# and both live in the same cgroup — under a 7g cap it thrashes (observed: 0.4% CPU,
	@# 26 MB/s of disk reads, no log progress for 11 minutes). Worse, an -Xmx above the
	@# cgroup cap is a latent OOM-kill once LM preparation grows the heap.
	@# Run it directly, with the serving stack down, so heap (7g) and DEM page cache (~6g)
	@# each get a real budget out of the box's 15 GiB.
	docker run --rm --memory 13g \
	  -v $(DATA_DIR):/data -v $(PWD)/config:/config \
	  --entrypoint /bin/bash israelhikingmap/graphhopper:latest -c \
	  "java -Xmx7g -jar /graphhopper/graphhopper-web-*.jar import /config/graphhopper.yml"
	@# Bring the whole stack back, not just the services this target stopped: an import
	@# target that leaves some other service down makes `make all` order-dependent.
	docker compose up -d

# Planetiler's own --download fetches this, but from this droplet the route to
# osmdata.openstreetmap.de throttles to ~10 kB/s: 927 MB would take ~26 hours, and
# Planetiler reports it as a progress bar that never moves rather than as an error.
# The same file pulls at 4.3 MB/s from a workstation, so it gets side-loaded. Try the
# direct fetch with a throughput floor; if the floor is not met, stop at once and say
# exactly what to do instead. Present file => no-op, so `make tiles` stays re-runnable.
$(DATA_DIR)/sources/water-polygons-split-3857.zip:
	@mkdir -p $(DATA_DIR)/sources
	@echo "fetching water polygons (aborts if < 100 kB/s sustained for 60s)..."
	@curl -fL --retry 2 --speed-limit 102400 --speed-time 60 \
	    -o $@.part https://osmdata.openstreetmap.de/download/water-polygons-split-3857.zip \
	  && mv $@.part $@ \
	  || { rm -f $@.part; \
	       echo "" >&2; \
	       echo "ERROR: water polygons could not be fetched at a usable speed." >&2; \
	       echo "This host's route to osmdata.openstreetmap.de is throttled (~10 kB/s)," >&2; \
	       echo "which is a ~26h download. Side-load it from a better-connected machine:" >&2; \
	       echo "" >&2; \
	       echo "  curl -L -O https://osmdata.openstreetmap.de/download/water-polygons-split-3857.zip" >&2; \
	       echo "  scp water-polygons-split-3857.zip map-sgp1:$(DATA_DIR)/sources/" >&2; \
	       echo "" >&2; \
	       exit 1; }

tiles: $(DATA_DIR)/sources/water-polygons-split-3857.zip
	@echo "stopping serving stack — Planetiler wants the RAM"
	docker compose stop photon postgis graphhopper || true
	mkdir -p $(DATA_DIR)/tiles $(DATA_DIR)/tmp
	docker run --rm -e JAVA_TOOL_OPTIONS=-Xmx8g \
	  -v $(DATA_DIR):/data ghcr.io/onthegomap/planetiler:latest \
	  --osm-path=/data/osm/$(REGION_SLUG)-latest.osm.pbf \
	  --output=/data/tiles/$(REGION_SLUG).pmtiles \
	  --tmpdir=/data/tmp --download --force
	rm -rf $(DATA_DIR)/tmp
	docker compose up -d

up:
	docker compose up -d

down:
	docker compose down

all: fetch photon db dem graph tiles
