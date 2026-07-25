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

up:
	docker compose up -d

down:
	docker compose down

all: fetch photon db dem graph tiles
