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
	@echo "waiting for postgres..."
	@until docker compose exec -T postgis pg_isready -U $(POSTGRES_USER) -d $(POSTGRES_DB) >/dev/null 2>&1; do sleep 2; done
	PGPASSWORD=$(POSTGRES_PASSWORD) osm2pgsql -O flex -S config/osm2pgsql.lua \
	  -H 127.0.0.1 -P 5432 -U $(POSTGRES_USER) -d $(POSTGRES_DB) \
	  --slim --drop -C 6000 \
	  $(DATA_DIR)/osm/$(REGION_SLUG)-latest.osm.pbf
	docker compose exec -T postgis psql -U $(POSTGRES_USER) -d $(POSTGRES_DB) -c \
	  "CREATE INDEX IF NOT EXISTS osm_poi_geom_idx ON osm_poi USING GIST (geom); \
	   CREATE INDEX IF NOT EXISTS osm_poi_tags_idx ON osm_poi USING GIN (tags); \
	   CREATE INDEX IF NOT EXISTS osm_place_geom_idx ON osm_place USING GIST (geom); \
	   ANALYZE osm_poi; ANALYZE osm_place;"

up:
	docker compose up -d

down:
	docker compose down

all: fetch photon db dem graph tiles
