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
	docker compose up -d photon postgis graphhopper

tiles:
	@echo "stopping serving stack — Planetiler wants the RAM"
	docker compose stop photon postgis graphhopper || true
	mkdir -p $(DATA_DIR)/tiles $(DATA_DIR)/tmp
	docker run --rm -e JAVA_TOOL_OPTIONS=-Xmx8g \
	  -v $(DATA_DIR):/data ghcr.io/onthegomap/planetiler:latest \
	  --osm-path=/data/osm/$(REGION_SLUG)-latest.osm.pbf \
	  --output=/data/tiles/$(REGION_SLUG).pmtiles \
	  --tmpdir=/data/tmp --download --force
	rm -rf $(DATA_DIR)/tmp
	docker compose up -d tiles postgis photon

up:
	docker compose up -d

down:
	docker compose down

all: fetch photon db dem graph tiles
