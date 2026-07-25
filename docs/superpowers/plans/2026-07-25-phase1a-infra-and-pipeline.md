# Phase 1A — Infrastructure & Data Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a DigitalOcean droplet running the full Compose stack, with every Indonesia data artifact built and every service answering a smoke query.

**Architecture:** An offline `make` pipeline produces six artifacts onto a block-storage volume mounted at `/data`; an online Docker Compose stack serves them behind Caddy. Nothing in this plan writes application logic — it produces the substrate that Plan B's Rust API consumes.

**Tech Stack:** Docker Compose, Caddy, PostGIS 16, osm2pgsql (flex/Lua), Planetiler, go-pmtiles, GraphHopper, Photon, bash + curl + jq for smoke tests.

## Global Constraints

- Target region is **Indonesia** (`asia/indonesia`); the pipeline must stay region-parameterised so a different extract is a variable change, not a code change.
- Every service is **12-factor**: configuration by environment variable only, no host paths baked into images, `/data` is the only bind mount.
- All artifacts live under `/data` (the block-storage mount), never in the repo or the droplet root disk.
- Total artifact budget is **~22 GB** against a 100 GB volume.
- Droplet is **16 GB / 8 vCPU**; steady-state RAM budget is GraphHopper 4–6 GB, Photon ~4 GB (`-Xmx4G`), PostGIS 2–3 GB, tiles ~0.5 GB, OS ~2 GB.
- Imports (Planetiler, GraphHopper) run with the serving stack **stopped**.
- Java **21+** is required by Photon.
- Every `make` target is **idempotent** and independently re-runnable.
- Secrets come from `.env` (git-ignored); `.env.example` is committed and must list every variable.

---

## Two spec corrections this plan makes

Both were found while working through implementation detail. They change the spec and should be folded back into it.

**1. The graph needs Landmarks (LM), not just Contraction Hierarchies (CH).** The spec says `make graph` produces "CH + elevation". But CH bakes the cost function into the preprocessed graph — it cannot answer a query carrying an *arbitrary* runtime `custom_model`, which is precisely what the agent's `route` tool does. Runtime custom models require `ch.disable=true` plus LM-prepared profiles for acceptable speed. Task 8 prepares LM and keeps CH only for a fixed fast-car profile.

**2. The Photon country extract's version compatibility is unverified.** Task 4 is a gate that resolves it before any dependent work.

---

## File Structure

| Path | Responsibility |
|---|---|
| `Makefile` | All pipeline targets; the single entry point |
| `docker-compose.yml` | Service definitions for the online stack |
| `.env.example` | Every configuration variable, documented |
| `.gitignore` | Excludes `.env`, `/data`, build output |
| `config/graphhopper.yml` | GraphHopper profiles, LM/CH, elevation |
| `config/osm2pgsql.lua` | Flex-mode tag filtering to POIs and places |
| `config/Caddyfile` | TLS termination and reverse proxy |
| `scripts/fetch_osm.sh` | Geofabrik download + checksum verification |
| `scripts/fetch_photon.sh` | Photon index download + extraction |
| `scripts/fetch_dem.sh` | Skadi elevation tile download for the region bbox |
| `tests/smoke/lib.sh` | Shared assert helpers |
| `tests/smoke/*.sh` | One smoke test per service |
| `docs/runbook.md` | Droplet provisioning steps |

---

### Task 1: Repo scaffolding and smoke-test harness

**Files:**
- Create: `.gitignore`, `.env.example`, `Makefile`, `tests/smoke/lib.sh`, `tests/smoke/run_all.sh`

**Interfaces:**
- Consumes: nothing
- Produces: `make verify` runs every `tests/smoke/*.sh`; `assert_eq`, `assert_contains`, `assert_http_ok` helpers available to all later smoke tests

- [x] **Step 1: Write the failing test**

`tests/smoke/lib.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

FAILED=0

assert_eq() {
  local expected="$1" actual="$2" msg="${3:-}"
  if [[ "$expected" != "$actual" ]]; then
    echo "  FAIL: ${msg} — expected '${expected}', got '${actual}'" >&2
    FAILED=1
  else
    echo "  ok: ${msg}"
  fi
}

assert_contains() {
  local haystack="$1" needle="$2" msg="${3:-}"
  if [[ "$haystack" != *"$needle"* ]]; then
    echo "  FAIL: ${msg} — '${needle}' not found" >&2
    FAILED=1
  else
    echo "  ok: ${msg}"
  fi
}

assert_http_ok() {
  local url="$1" msg="${2:-}"
  local code
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$url" || echo 000)
  assert_eq "200" "$code" "${msg} (${url})"
}

assert_file_min_size() {
  local path="$1" min_bytes="$2" msg="${3:-}"
  local size
  size=$(stat -c%s "$path" 2>/dev/null || echo 0)
  if (( size < min_bytes )); then
    echo "  FAIL: ${msg} — ${path} is ${size}B, expected >= ${min_bytes}B" >&2
    FAILED=1
  else
    echo "  ok: ${msg}"
  fi
}

finish() { exit "$FAILED"; }
```

`tests/smoke/run_all.sh`:

```bash
#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")"
rc=0
for t in test_*.sh; do
  [[ -e "$t" ]] || continue
  echo "== ${t}"
  bash "$t" || rc=1
done
exit $rc
```

- [x] **Step 2: Run it to verify it fails**

Run: `make verify`
Expected: FAIL with `make: *** No rule to make target 'verify'`

- [x] **Step 3: Write the minimal Makefile and supporting files**

`.gitignore`:

```
.env
/data/
target/
node_modules/
*.tar.bz2
*.osm.pbf
```

`.env.example`:

```bash
# Geofabrik region path (e.g. asia/indonesia)
OSM_REGION=asia/indonesia
# Short region slug used in filenames
REGION_SLUG=indonesia
# Photon country-code extract (ISO 3166-1 alpha-2, lowercase)
PHOTON_COUNTRY=id
# Region bbox for DEM tile fetching: min_lon min_lat max_lon max_lat
REGION_BBOX="94.5 -11.1 141.1 6.1"
# Absolute path to the block-storage mount
DATA_DIR=/data
# PostGIS
POSTGRES_USER=map
POSTGRES_PASSWORD=changeme
POSTGRES_DB=map
# Public hostname for Caddy TLS
SITE_HOST=map.example.com
```

`Makefile`:

```make
SHELL := /bin/bash
include .env
export

.PHONY: verify fetch photon db dem graph tiles all up down

verify:
	@bash tests/smoke/run_all.sh

up:
	docker compose up -d

down:
	docker compose down

all: fetch photon db dem graph tiles
```

- [x] **Step 4: Run to verify it passes**

Run: `cp .env.example .env && chmod +x tests/smoke/*.sh && make verify`
Expected: PASS — exits 0 with no test files yet (loop finds nothing)

- [x] **Step 5: Commit**

```bash
git add .gitignore .env.example Makefile tests/smoke/
git commit -m "chore: repo scaffolding and smoke-test harness"
```

---

### Task 2: Droplet provisioning runbook

**Files:**
- Create: `docs/runbook.md`, `tests/smoke/test_host.sh`

**Interfaces:**
- Consumes: nothing
- Produces: a droplet with Docker installed, block storage mounted at `$DATA_DIR`, firewall configured. All later tasks assume `/data` is writable and has >70 GB free.

- [x] **Step 1: Write the failing test**

`tests/smoke/test_host.sh`:

```bash
#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${DATA_DIR:=/data}"

assert_eq "0" "$(docker info >/dev/null 2>&1; echo $?)" "docker daemon reachable"
assert_eq "0" "$(test -d "$DATA_DIR" && echo 0 || echo 1)" "$DATA_DIR exists"
assert_eq "0" "$(test -w "$DATA_DIR" && echo 0 || echo 1)" "$DATA_DIR writable"

avail_gb=$(df -BG --output=avail "$DATA_DIR" | tail -1 | tr -dc '0-9')
if (( avail_gb < 70 )); then
  echo "  FAIL: only ${avail_gb}G free on $DATA_DIR, need >= 70G" >&2
  FAILED=1
else
  echo "  ok: ${avail_gb}G free on $DATA_DIR"
fi

finish
```

- [x] **Step 2: Run to verify it fails**

Run: `bash tests/smoke/test_host.sh`
Expected: FAIL — `/data` does not exist on a fresh machine

- [x] **Step 3: Provision, following the runbook**

Write `docs/runbook.md` with these steps, then execute them:

```bash
# 1. Create droplet: Basic, 16 GB / 8 vCPU, Ubuntu 24.04 LTS, SSH key auth
# 2. Create a 100 GB block-storage volume in the SAME region, attach to droplet
# 3. On the droplet:

# Format and mount the volume (DEVICE from `lsblk`, e.g. /dev/sda)
DEVICE=/dev/disk/by-id/scsi-0DO_Volume_map-data
mkfs.ext4 -F "$DEVICE"
mkdir -p /data
echo "$DEVICE /data ext4 defaults,nofail,discard 0 2" >> /etc/fstab
mount -a

# Docker
curl -fsSL https://get.docker.com | sh
systemctl enable --now docker

# Firewall: SSH + HTTP/HTTPS only. Service ports stay internal to Compose.
ufw allow OpenSSH && ufw allow 80/tcp && ufw allow 443/tcp && ufw --force enable

# Swap, so import spikes don't OOM-kill the box
fallocate -l 4G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
echo '/swapfile none swap sw 0 0' >> /etc/fstab
```

- [x] **Step 4: Run to verify it passes**

Run: `bash tests/smoke/test_host.sh`
Expected: PASS — all four assertions ok, ~98G free

- [x] **Step 5: Commit**

```bash
git add docs/runbook.md tests/smoke/test_host.sh
git commit -m "docs: droplet provisioning runbook and host smoke test"
```

---

### Task 3: `make fetch` — OSM extract with checksum verification

**Files:**
- Create: `scripts/fetch_osm.sh`, `tests/smoke/test_osm_pbf.sh`
- Modify: `Makefile`

**Interfaces:**
- Consumes: `OSM_REGION`, `REGION_SLUG`, `DATA_DIR` from `.env`
- Produces: `$DATA_DIR/osm/$REGION_SLUG-latest.osm.pbf`, MD5-verified. Tasks 6, 8, 9 all read this file.

- [x] **Step 1: Write the failing test**

`tests/smoke/test_osm_pbf.sh`:

```bash
#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${DATA_DIR:=/data}" "${REGION_SLUG:=indonesia}"
PBF="$DATA_DIR/osm/$REGION_SLUG-latest.osm.pbf"

assert_file_min_size "$PBF" 500000000 "PBF present and >= 500MB"
# OSM PBF files begin with a 4-byte big-endian header length then "OSMHeader"
assert_contains "$(head -c 32 "$PBF" | tr -dc '[:print:]')" "OSMHeader" "PBF has valid OSMHeader magic"

finish
```

- [x] **Step 2: Run to verify it fails**

Run: `bash tests/smoke/test_osm_pbf.sh`
Expected: FAIL — `PBF present and >= 500MB` (file is 0 bytes / missing)

- [x] **Step 3: Write the fetch script**

`scripts/fetch_osm.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
: "${OSM_REGION:?}" "${REGION_SLUG:?}" "${DATA_DIR:?}"

DEST="$DATA_DIR/osm"
BASE="https://download.geofabrik.de/${OSM_REGION}-latest.osm.pbf"
mkdir -p "$DEST"

echo "Downloading ${BASE}"
curl -fL --retry 3 -o "$DEST/${REGION_SLUG}-latest.osm.pbf.part" "$BASE"
curl -fL --retry 3 -o "$DEST/${REGION_SLUG}-latest.osm.pbf.md5"  "${BASE}.md5"

# Geofabrik .md5 is "<hash>  <basename>"; verify against our .part file
EXPECTED=$(awk '{print $1}' "$DEST/${REGION_SLUG}-latest.osm.pbf.md5")
ACTUAL=$(md5sum "$DEST/${REGION_SLUG}-latest.osm.pbf.part" | awk '{print $1}')
if [[ "$EXPECTED" != "$ACTUAL" ]]; then
  echo "Checksum mismatch: expected $EXPECTED got $ACTUAL" >&2
  rm -f "$DEST/${REGION_SLUG}-latest.osm.pbf.part"
  exit 1
fi

mv "$DEST/${REGION_SLUG}-latest.osm.pbf.part" "$DEST/${REGION_SLUG}-latest.osm.pbf"
echo "OK: $DEST/${REGION_SLUG}-latest.osm.pbf"
```

Add to `Makefile`:

```make
fetch:
	@bash scripts/fetch_osm.sh
```

- [x] **Step 4: Run to verify it passes**

Run: `chmod +x scripts/fetch_osm.sh && make fetch && bash tests/smoke/test_osm_pbf.sh`
Expected: download completes, checksum matches, both assertions ok

- [x] **Step 5: Commit**

```bash
git add scripts/fetch_osm.sh tests/smoke/test_osm_pbf.sh Makefile
git commit -m "feat: make fetch — Geofabrik extract with checksum verification"
```

---

### Task 4: GATE — resolve Photon extract version compatibility

**Files:**
- Create: `docs/decisions/photon-index-source.md`

**Interfaces:**
- Consumes: nothing
- Produces: a written decision that Task 5 implements. **Task 5 must not start until this task closes.**

The spec chose Photon 1.x with embedded OpenSearch. The Indonesia country extract at
`https://download1.graphhopper.com/public/extracts/by-country-code/id/photon-db-id-250720.tar.bz2`
(452 MB, dated 2025-07-20) sits in a tree that is **not version-segmented**, so its
compatibility with Photon 1.x is unknown. Resolve it empirically before building on it.

- [x] **Step 1: Write the failing test**

There is no code to test here — the deliverable is a decision. The gate is: `docs/decisions/photon-index-source.md` exists and names exactly one of the three branches below.

Run: `test -f docs/decisions/photon-index-source.md`
Expected: FAIL (file absent)

- [x] **Step 2: Download the extract and the Photon 1.x JAR**

```bash
mkdir -p /data/photon && cd /data/photon
curl -fL --retry 3 -O https://download1.graphhopper.com/public/extracts/by-country-code/id/photon-db-id-250720.tar.bz2
curl -fL --retry 3 -O https://download1.graphhopper.com/public/extracts/by-country-code/id/photon-db-id-250720.tar.bz2.md5
md5sum -c photon-db-id-250720.tar.bz2.md5
apt-get install -y pbzip2
pbzip2 -cd photon-db-id-250720.tar.bz2 | tar x
# Grab the latest 1.x JAR from https://github.com/komoot/photon/releases
curl -fL -o photon.jar "<1.x release asset URL from the releases page>"
```

- [x] **Step 3: Attempt to start Photon 1.x against the extracted index**

```bash
java -Xmx4G -jar photon.jar -data-dir /data/photon 2>&1 | tee /tmp/photon-start.log
```

Then, in another shell:

```bash
curl -s 'http://localhost:2322/api?q=Bromo&limit=3' | jq .
```

- [x] **Step 4: Record the branch taken**

Write `docs/decisions/photon-index-source.md` recording **exactly one**:

- **(a) It loads and geocodes.** Proceed as specified. Record the Photon version and index date. Task 5 uses this extract directly.
- **(b) It fails with a version/index-format error.** Two sub-options — pick one and record why:
  - **(b1)** Pin Photon to the version matching the extract. Cheap; costs the maintained-line benefit the spec chose. Update the spec's decision 6.
  - **(b2)** Build an Indonesia index from the 1.x planet JSONL dump
    (`photon-db-planet-1.0-latest.jsonl.zst`, ~26 GB) filtered to `REGION_BBOX`, using
    Photon's `-import-file` path. Keeps Photon 1.x; costs a 26 GB download and an import
    step in `make photon`. **Requires re-checking the disk budget** — 26 GB transient on
    top of ~22 GB of artifacts still fits 100 GB, but verify before committing.
- **(c) It loads but geocoding quality is unusable** (e.g. missing local-language names). Treat as (b2).

Record the actual `curl` output for "Bromo" in the decision file as evidence.

- [x] **Step 5: Commit**

```bash
git add docs/decisions/photon-index-source.md
git commit -m "docs: resolve Photon index source (gate for make photon)"
```

---

### Task 5: `make photon` — geocoding service

**Files:**
- Create: `scripts/fetch_photon.sh`, `tests/smoke/test_photon.sh`
- Modify: `Makefile`, `docker-compose.yml`

**Interfaces:**
- Consumes: the decision from Task 4; `PHOTON_COUNTRY`, `DATA_DIR`
- Produces: Photon on `photon:2322` inside the Compose network. `GET /api?q=<name>&limit=<n>` returns GeoJSON `FeatureCollection`. Plan B's `geocode` tool calls this.

- [x] **Step 1: Write the failing test**

`tests/smoke/test_photon.sh`:

```bash
#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
BASE="${PHOTON_URL:-http://localhost:2322}"

assert_http_ok "$BASE/status" "photon status endpoint"

body=$(curl -s --max-time 15 "$BASE/api?q=Bromo&limit=3")
assert_contains "$body" "FeatureCollection" "geocode returns a FeatureCollection"

n=$(echo "$body" | jq '.features | length')
if (( n < 1 )); then
  echo "  FAIL: geocode 'Bromo' returned 0 features" >&2; FAILED=1
else
  echo "  ok: geocode 'Bromo' returned ${n} features"
fi

# Result must fall inside the Indonesia bbox
lon=$(echo "$body" | jq -r '.features[0].geometry.coordinates[0]')
lat=$(echo "$body" | jq -r '.features[0].geometry.coordinates[1]')
in_box=$(awk -v lo="$lon" -v la="$lat" 'BEGIN{print (lo>94.5 && lo<141.1 && la>-11.1 && la<6.1) ? "yes":"no"}')
assert_eq "yes" "$in_box" "top result inside Indonesia bbox (${lat},${lon})"

finish
```

- [x] **Step 2: Run to verify it fails**

Run: `bash tests/smoke/test_photon.sh`
Expected: FAIL — `photon status endpoint` returns 000 (nothing listening)

- [x] **Step 3: Implement the fetch script and service**

`scripts/fetch_photon.sh` (branch (a) form; adjust per the Task 4 decision):

```bash
#!/usr/bin/env bash
set -euo pipefail
: "${PHOTON_COUNTRY:?}" "${DATA_DIR:?}"

DEST="$DATA_DIR/photon"
DUMP="photon-db-${PHOTON_COUNTRY}-250720.tar.bz2"
URL="https://download1.graphhopper.com/public/extracts/by-country-code/${PHOTON_COUNTRY}/${DUMP}"
mkdir -p "$DEST"

if [[ -d "$DEST/photon_data" ]]; then
  echo "Index already present at $DEST/photon_data — skipping"; exit 0
fi

cd "$DEST"
curl -fL --retry 3 -O "$URL"
curl -fL --retry 3 -O "${URL}.md5"
md5sum -c "${DUMP}.md5"
pbzip2 -cd "$DUMP" | tar x
rm -f "$DUMP"
echo "OK: $DEST/photon_data"
```

Add to `docker-compose.yml`:

```yaml
services:
  photon:
    image: eclipse-temurin:21-jre
    restart: unless-stopped
    command: >
      java -Xmx4G -jar /photon/photon.jar
      -data-dir /photon -listen-port 2322 -listen-ip 0.0.0.0
    volumes:
      - ${DATA_DIR}/photon:/photon
    ports:
      - "127.0.0.1:2322:2322"
    mem_limit: 5g
```

Add to `Makefile`:

```make
photon:
	@bash scripts/fetch_photon.sh
```

- [x] **Step 4: Run to verify it passes**

Run: `chmod +x scripts/fetch_photon.sh && make photon && docker compose up -d photon && sleep 45 && bash tests/smoke/test_photon.sh`
Expected: all four assertions ok; Bromo resolves to roughly `-7.94, 112.95`

- [x] **Step 5: Commit**

```bash
git add scripts/fetch_photon.sh tests/smoke/test_photon.sh Makefile docker-compose.yml
git commit -m "feat: make photon — geocoding service with index fetch"
```

---

### Task 6: `make db` — PostGIS POI store

**Files:**
- Create: `config/osm2pgsql.lua`, `tests/smoke/test_postgis.sh`
- Modify: `Makefile`, `docker-compose.yml`

**Interfaces:**
- Consumes: `$DATA_DIR/osm/$REGION_SLUG-latest.osm.pbf` from Task 3
- Produces: tables `osm_poi(osm_id bigint, name text, tags jsonb, geom geometry(Point,4326))` and `osm_place(...)`, with a GiST index on `geom` and a GIN index on `tags`. Plan B's `search_poi` tool queries `osm_poi`.

- [x] **Step 1: Write the failing test**

`tests/smoke/test_postgis.sh`:

```bash
#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${POSTGRES_USER:=map}" "${POSTGRES_DB:=map}"
PSQL="docker compose exec -T postgis psql -U $POSTGRES_USER -d $POSTGRES_DB -tAc"

assert_eq "1" "$($PSQL "SELECT 1")" "postgis reachable"
assert_eq "t" "$($PSQL "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='postgis')")" "postgis extension installed"

# Indonesia has many volcanoes; assert a plausible floor
volcanoes=$($PSQL "SELECT count(*) FROM osm_poi WHERE tags->>'natural' = 'volcano'")
if (( volcanoes < 50 )); then
  echo "  FAIL: only ${volcanoes} volcanoes, expected >= 50" >&2; FAILED=1
else
  echo "  ok: ${volcanoes} volcanoes in osm_poi"
fi

beaches=$($PSQL "SELECT count(*) FROM osm_poi WHERE tags->>'natural' = 'beach'")
if (( beaches < 100 )); then
  echo "  FAIL: only ${beaches} beaches, expected >= 100" >&2; FAILED=1
else
  echo "  ok: ${beaches} beaches in osm_poi"
fi

# The bbox+tag query shape the agent will actually use must hit the GiST index
plan=$($PSQL "EXPLAIN SELECT * FROM osm_poi WHERE geom && ST_MakeEnvelope(112,-8,113,-7,4326) AND tags ? 'natural'")
assert_contains "$plan" "Index" "bbox query uses an index"

finish
```

- [x] **Step 2: Run to verify it fails**

Run: `bash tests/smoke/test_postgis.sh`
Expected: FAIL — `postgis reachable` (no container)

- [x] **Step 3: Implement the Lua config and service**

`config/osm2pgsql.lua`:

```lua
local tables = {}

tables.poi = osm2pgsql.define_table{
  name = 'osm_poi',
  ids = { type = 'any', id_column = 'osm_id' },
  columns = {
    { column = 'name', type = 'text' },
    { column = 'tags', type = 'jsonb' },
    { column = 'geom', type = 'point', projection = 4326, not_null = true },
  }
}

tables.place = osm2pgsql.define_table{
  name = 'osm_place',
  ids = { type = 'any', id_column = 'osm_id' },
  columns = {
    { column = 'name', type = 'text' },
    { column = 'tags', type = 'jsonb' },
    { column = 'geom', type = 'point', projection = 4326, not_null = true },
  }
}

-- Only these tag keys are retained; a full import would be 20GB+ and unused.
local poi_keys = { 'natural', 'tourism', 'amenity', 'historic', 'leisure' }

local function is_poi(tags)
  for _, k in ipairs(poi_keys) do
    if tags[k] then return true end
  end
  return false
end

local function insert(tbl, object, geom)
  tbl:insert{ name = object.tags.name, tags = object.tags, geom = geom }
end

function osm2pgsql.process_node(object)
  if object.tags.place then insert(tables.place, object, object:as_point()) end
  if is_poi(object.tags) then insert(tables.poi, object, object:as_point()) end
end

function osm2pgsql.process_way(object)
  if not object.is_closed then return end
  if is_poi(object.tags) then
    insert(tables.poi, object, object:as_polygon():centroid())
  end
end

function osm2pgsql.process_relation(object)
  if object.tags.type == 'multipolygon' and is_poi(object.tags) then
    insert(tables.poi, object, object:as_multipolygon():centroid())
  end
end
```

Add to `docker-compose.yml`:

```yaml
  postgis:
    image: postgis/postgis:16-3.4
    restart: unless-stopped
    environment:
      POSTGRES_USER: ${POSTGRES_USER}
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
      POSTGRES_DB: ${POSTGRES_DB}
    volumes:
      - ${DATA_DIR}/pgdata:/var/lib/postgresql/data
    ports:
      - "127.0.0.1:5432:5432"
    shm_size: 1g
    mem_limit: 3g
```

Add to `Makefile`:

```make
db:
	docker compose up -d postgis
	@sleep 10
	docker run --rm --network host \
	  -v $(DATA_DIR):/data -v $(PWD)/config:/config \
	  iboates/osm2pgsql:latest \
	  osm2pgsql -O flex -S /config/osm2pgsql.lua \
	    -H localhost -U $(POSTGRES_USER) -d $(POSTGRES_DB) \
	    --slim --drop -C 4000 \
	    /data/osm/$(REGION_SLUG)-latest.osm.pbf
	docker compose exec -T postgis psql -U $(POSTGRES_USER) -d $(POSTGRES_DB) -c \
	  "CREATE INDEX IF NOT EXISTS osm_poi_geom_idx ON osm_poi USING GIST (geom); \
	   CREATE INDEX IF NOT EXISTS osm_poi_tags_idx ON osm_poi USING GIN (tags); \
	   CREATE INDEX IF NOT EXISTS osm_place_geom_idx ON osm_place USING GIST (geom); \
	   ANALYZE osm_poi; ANALYZE osm_place;"
```

Note: `PGPASSWORD` must be exported for the osm2pgsql container; add `-e PGPASSWORD=$(POSTGRES_PASSWORD)` to the `docker run` line.

- [x] **Step 4: Run to verify it passes**

Run: `make db && bash tests/smoke/test_postgis.sh`
Expected: all five assertions ok; import takes 20–40 minutes

- [x] **Step 5: Commit**

```bash
git add config/osm2pgsql.lua tests/smoke/test_postgis.sh Makefile docker-compose.yml
git commit -m "feat: make db — PostGIS POI store via osm2pgsql flex"
```

---

### Task 7: `make dem` — Skadi elevation tiles

**Files:**
- Create: `scripts/fetch_dem.sh`, `tests/smoke/test_dem.sh`
- Modify: `Makefile`

**Interfaces:**
- Consumes: `REGION_BBOX`, `DATA_DIR`
- Produces: `$DATA_DIR/dem/<LATBAND>/<TILE>.hgt.gz` in Skadi layout. Task 8's GraphHopper import reads this as its elevation cache.

Pre-fetching rather than letting GraphHopper download on demand makes the graph import
reproducible and offline-repeatable. Ocean tiles do not exist and return 404 — the script
must treat that as normal, not as failure.

- [x] **Step 1: Write the failing test**

`tests/smoke/test_dem.sh`:

```bash
#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${DATA_DIR:=/data}"
DEM="$DATA_DIR/dem"

count=$(find "$DEM" -name '*.hgt.gz' 2>/dev/null | wc -l)
if (( count < 200 )); then
  echo "  FAIL: only ${count} DEM tiles, expected >= 200" >&2; FAILED=1
else
  echo "  ok: ${count} DEM tiles present"
fi

# Java (Bromo area) must be covered
assert_eq "0" "$(test -f "$DEM/S08/S08E112.hgt.gz" && echo 0 || echo 1)" "S08E112 (East Java) present"
assert_file_min_size "$DEM/S08/S08E112.hgt.gz" 100000 "S08E112 is a real tile"

finish
```

- [x] **Step 2: Run to verify it fails**

Run: `bash tests/smoke/test_dem.sh`
Expected: FAIL — 0 DEM tiles

- [x] **Step 3: Write the fetch script**

`scripts/fetch_dem.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
: "${REGION_BBOX:?}" "${DATA_DIR:?}"
read -r MIN_LON MIN_LAT MAX_LON MAX_LAT <<< "$REGION_BBOX"

DEST="$DATA_DIR/dem"
BASE="https://s3.amazonaws.com/elevation-tiles-prod/skadi"
mkdir -p "$DEST"

lat_start=$(printf '%.0f' "$(echo "$MIN_LAT" | awk '{print int($1) - ($1<0 && $1!=int($1) ? 1 : 0)}')")
lat_end=$(printf '%.0f'   "$(echo "$MAX_LAT" | awk '{print int($1)}')")
lon_start=$(printf '%.0f' "$(echo "$MIN_LON" | awk '{print int($1) - ($1<0 && $1!=int($1) ? 1 : 0)}')")
lon_end=$(printf '%.0f'   "$(echo "$MAX_LON" | awk '{print int($1)}')")

got=0; missing=0
for lat in $(seq "$lat_start" "$lat_end"); do
  if (( lat < 0 )); then band=$(printf 'S%02d' $(( -lat )); ) else band=$(printf 'N%02d' "$lat"); fi
  mkdir -p "$DEST/$band"
  for lon in $(seq "$lon_start" "$lon_end"); do
    if (( lon < 0 )); then lonp=$(printf 'W%03d' $(( -lon ))); else lonp=$(printf 'E%03d' "$lon"); fi
    tile="${band}${lonp}.hgt.gz"
    out="$DEST/$band/$tile"
    [[ -s "$out" ]] && { got=$((got+1)); continue; }
    # Ocean tiles legitimately 404 — record and move on.
    if curl -fsL --retry 2 -o "$out.part" "$BASE/$band/$tile"; then
      mv "$out.part" "$out"; got=$((got+1))
    else
      rm -f "$out.part"; missing=$((missing+1))
    fi
  done
done
echo "DEM: ${got} tiles fetched, ${missing} absent (ocean)"
```

Add to `Makefile`:

```make
dem:
	@bash scripts/fetch_dem.sh
```

- [x] **Step 4: Run to verify it passes**

Run: `chmod +x scripts/fetch_dem.sh && make dem && bash tests/smoke/test_dem.sh`
Expected: several hundred tiles fetched, ~8 GB, all three assertions ok

- [x] **Step 5: Commit**

```bash
git add scripts/fetch_dem.sh tests/smoke/test_dem.sh Makefile
git commit -m "feat: make dem — Skadi elevation tiles for the region bbox"
```

---

### Task 8: `make graph` — GraphHopper routing with runtime custom models

**Files:**
- Create: `config/graphhopper.yml`, `tests/smoke/test_graphhopper.sh`
- Modify: `Makefile`, `docker-compose.yml`

**Interfaces:**
- Consumes: the PBF from Task 3, DEM tiles from Task 7
- Produces: GraphHopper on `graphhopper:8989`. `POST /route` accepts `{points, profile, ch.disable, custom_model}` and returns `paths[].points` (GeoJSON LineString), `distance`, `time`, `ascend`, `descend`. Plan B's `route` and `elevation_profile` tools call this.

**This is the task the first spec correction applies to.** CH bakes the cost function into
the preprocessed graph and cannot serve an arbitrary runtime `custom_model`. LM (landmarks)
can. The config below prepares LM for the flexible profile and keeps CH only for a fixed
fast profile.

- [x] **Step 1: Write the failing test**

`tests/smoke/test_graphhopper.sh`:

```bash
#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
BASE="${GH_URL:-http://localhost:8989}"

assert_http_ok "$BASE/health" "graphhopper health"

# Surabaya -> Malang, plain route
plain=$(curl -s -X POST "$BASE/route" -H 'Content-Type: application/json' -d '{
  "points": [[112.7521,-7.2575],[112.6304,-7.9666]],
  "profile": "car", "points_encoded": false
}')
dist=$(echo "$plain" | jq -r '.paths[0].distance // 0')
if (( $(echo "$dist > 50000" | bc -l) )); then
  echo "  ok: plain route ${dist}m"
else
  echo "  FAIL: plain route distance ${dist}, expected > 50000" >&2; FAILED=1
fi

# THE critical assertion: a runtime custom_model must be accepted, not 400'd.
custom=$(curl -s -X POST "$BASE/route" -H 'Content-Type: application/json' -d '{
  "points": [[112.7521,-7.2575],[112.6304,-7.9666]],
  "profile": "car", "points_encoded": false, "ch.disable": true,
  "custom_model": { "priority": [{"if":"road_class == MOTORWAY","multiply_by":0.05}] }
}')
assert_contains "$custom" "paths" "runtime custom_model accepted (LM prepared)"

# Elevation must be present — the third ordinate of each coordinate.
ncoord=$(echo "$plain" | jq '.paths[0].points.coordinates[0] | length')
assert_eq "3" "$ncoord" "coordinates carry elevation (3 ordinates)"

finish
```

- [x] **Step 2: Run to verify it fails**

Run: `bash tests/smoke/test_graphhopper.sh`
Expected: FAIL — `graphhopper health` returns 000

- [x] **Step 3: Write the config and service**

`config/graphhopper.yml`:

```yaml
graphhopper:
  datareader.file: /data/osm/indonesia-latest.osm.pbf
  graph.location: /data/graph

  graph.elevation.provider: skadi
  graph.elevation.cache_dir: /data/dem/
  graph.elevation.dataaccess: MMAP

  # Encoded values the agent's custom models reference.
  graph.encoded_values: car_access, car_average_speed, road_class, road_environment,
                        surface, smoothness, max_speed, toll, track_type, curvature,
                        average_slope, max_slope

  profiles:
    - name: car
      weighting: custom
      custom_model: {}          # empty = plain fastest; overridden per request

  # Landmarks make runtime custom models fast. Required — CH cannot serve them.
  profiles_lm:
    - profile: car

  # CH only for the fixed fast profile; requests wanting a custom model send ch.disable=true.
  profiles_ch:
    - profile: car

  prepare.min_network_size: 200
  routing.non_ch.max_waypoint_distance: 1000000

server:
  application_connectors:
    - type: http
      port: 8989
      bind_host: 0.0.0.0
  admin_connectors:
    - type: http
      port: 8990
      bind_host: 127.0.0.1
```

Add to `docker-compose.yml`:

```yaml
  graphhopper:
    image: graphhopper/graphhopper:latest
    restart: unless-stopped
    command: ["--input","/data/osm/${REGION_SLUG}-latest.osm.pbf","--config","/config/graphhopper.yml"]
    environment:
      JAVA_OPTS: "-Xmx6g -Xms2g"
    volumes:
      - ${DATA_DIR}:/data
      - ./config:/config
    ports:
      - "127.0.0.1:8989:8989"
    mem_limit: 7g
```

Add to `Makefile`:

```make
graph:
	docker compose stop photon postgis || true
	docker compose run --rm graphhopper \
	  --input /data/osm/$(REGION_SLUG)-latest.osm.pbf \
	  --config /config/graphhopper.yml --import
	docker compose up -d graphhopper
```

- [x] **Step 4: Run to verify it passes**

Run: `make graph && sleep 60 && bash tests/smoke/test_graphhopper.sh`
Expected: all four assertions ok. Import takes 20–40 minutes. If `runtime custom_model accepted` fails with a 400 mentioning CH, `profiles_lm` did not prepare — check the import log for `prepare.lm`.

- [x] **Step 5: Commit**

```bash
git add config/graphhopper.yml tests/smoke/test_graphhopper.sh Makefile docker-compose.yml
git commit -m "feat: make graph — GraphHopper with LM for runtime custom models"
```

---

### Task 9: `make tiles` — PMTiles basemap

**Files:**
- Create: `tests/smoke/test_tiles.sh`
- Modify: `Makefile`, `docker-compose.yml`

**Interfaces:**
- Consumes: the PBF from Task 3
- Produces: `$DATA_DIR/tiles/$REGION_SLUG.pmtiles`, served on `tiles:8080`. Plan C's MapLibre frontend consumes `http://<host>/tiles/<slug>.pmtiles` via the `pmtiles://` protocol.

- [x] **Step 1: Write the failing test**

`tests/smoke/test_tiles.sh`:

```bash
#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${DATA_DIR:=/data}" "${REGION_SLUG:=indonesia}"
BASE="${TILES_URL:-http://localhost:8080}"

assert_file_min_size "$DATA_DIR/tiles/$REGION_SLUG.pmtiles" 1000000000 "pmtiles >= 1GB"

meta=$(curl -s --max-time 10 "$BASE/$REGION_SLUG/metadata")
assert_contains "$meta" "vector_layers" "tile metadata lists vector layers"

# A zoom-10 tile over Java must be non-trivial
code=$(curl -s -o /tmp/tile.mvt -w '%{http_code}' "$BASE/$REGION_SLUG/10/822/526.mvt")
assert_eq "200" "$code" "z10 Java tile served"
assert_file_min_size /tmp/tile.mvt 1000 "z10 Java tile has content"

finish
```

- [x] **Step 2: Run to verify it fails**

Run: `bash tests/smoke/test_tiles.sh`
Expected: FAIL — pmtiles file missing

- [x] **Step 3: Implement the build and service**

Add to `docker-compose.yml`:

```yaml
  tiles:
    image: protomaps/go-pmtiles:latest
    restart: unless-stopped
    command: ["serve","/data/tiles","--cors=*","--port=8080"]
    volumes:
      - ${DATA_DIR}/tiles:/data/tiles:ro
    ports:
      - "127.0.0.1:8080:8080"
    mem_limit: 1g
```

Add to `Makefile`:

```make
tiles:
	docker compose stop graphhopper photon postgis || true
	mkdir -p $(DATA_DIR)/tiles $(DATA_DIR)/tmp
	docker run --rm -e JAVA_TOOL_OPTIONS=-Xmx10g \
	  -v $(DATA_DIR):/data ghcr.io/onthegomap/planetiler:latest \
	  --osm-path=/data/osm/$(REGION_SLUG)-latest.osm.pbf \
	  --output=/data/tiles/$(REGION_SLUG).pmtiles \
	  --tmpdir=/data/tmp --force
	rm -rf $(DATA_DIR)/tmp
	docker compose up -d tiles
```

- [x] **Step 4: Run to verify it passes**

Run: `make tiles && bash tests/smoke/test_tiles.sh`
Expected: all four assertions ok. Build takes 15–30 minutes and needs ~5 GB transient in `/data/tmp`, which the target removes afterwards.

- [x] **Step 5: Commit**

```bash
git add tests/smoke/test_tiles.sh Makefile docker-compose.yml
git commit -m "feat: make tiles — Planetiler PMTiles basemap and server"
```

---

### Task 10: Caddy edge and full-stack verification

**Files:**
- Create: `config/Caddyfile`, `tests/smoke/test_edge.sh`
- Modify: `docker-compose.yml`, `Makefile`

**Interfaces:**
- Consumes: every service from Tasks 5–9
- Produces: TLS on `https://$SITE_HOST`, with `/tiles/*` proxied to the tile server and `/api/*` reserved for Plan B's Rust service. `make verify` runs the whole smoke suite.

- [x] **Step 1: Write the failing test**

`tests/smoke/test_edge.sh`:

```bash
#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
: "${SITE_HOST:?SITE_HOST must be set}"

assert_http_ok "https://$SITE_HOST/healthz" "edge healthz over TLS"

code=$(curl -s -o /dev/null -w '%{http_code}' "https://$SITE_HOST/tiles/${REGION_SLUG}/metadata")
assert_eq "200" "$code" "tiles proxied through the edge"

# Internal service ports must NOT be reachable from outside
for p in 2322 5432 8080 8989; do
  ext=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://$SITE_HOST:$p/" || echo 000)
  assert_eq "000" "$ext" "port $p not exposed publicly"
done

finish
```

- [x] **Step 2: Run to verify it fails**

Run: `bash tests/smoke/test_edge.sh`
Expected: FAIL — `edge healthz over TLS` returns 000

- [x] **Step 3: Implement the edge**

`config/Caddyfile`:

```
{$SITE_HOST} {
	encode gzip

	handle /healthz {
		respond "ok" 200
	}

	handle_path /tiles/* {
		reverse_proxy tiles:8080
	}

	# Reserved for the Rust API delivered by Plan B.
	handle /api/* {
		reverse_proxy api:8000
	}

	handle {
		root * /srv
		file_server
	}
}
```

Add to `docker-compose.yml`:

```yaml
  caddy:
    image: caddy:2-alpine
    restart: unless-stopped
    environment:
      SITE_HOST: ${SITE_HOST}
    volumes:
      - ./config/Caddyfile:/etc/caddy/Caddyfile:ro
      - ./frontend:/srv:ro
      - caddy_data:/data
    ports:
      - "80:80"
      - "443:443"

volumes:
  caddy_data:
```

Create `frontend/index.html` as a placeholder so the mount is valid:

```html
<!doctype html><meta charset=utf-8><title>map</title><p>Plan C delivers the frontend.</p>
```

Note: `handle /api/*` references a service Plan B creates. Until then Caddy returns 502 for
`/api/*`, which is expected and is not covered by this task's smoke test.

- [x] **Step 4: Run to verify it passes**

Run: `docker compose up -d caddy && sleep 20 && bash tests/smoke/test_edge.sh && make verify`
Expected: `test_edge.sh` passes all six assertions; `make verify` runs every suite green

- [x] **Step 5: Commit**

```bash
git add config/Caddyfile frontend/index.html tests/smoke/test_edge.sh docker-compose.yml
git commit -m "feat: Caddy edge with TLS and full-stack verification"
```

---

## Outcome — completed 2026-07-25

`make verify` exits 0 on `map-sgp1`; all 46 assertions across 8 suites pass. Six findings
surfaced during execution that the plan as written did not anticipate:

| # | Finding | Where it now lives |
|---|---|---|
| 1 | `docker compose run` inherits the service `mem_limit`, so the graph import ran inside the 7 GB *serving* budget and thrashed at 0.4% CPU. LM preparation was later measured at 11.28 GB — it would have been OOM-killed | `Makefile` (`graph` uses `docker run --memory 13g`), `docs/runbook.md` |
| 2 | The GraphHopper image's `ENTRYPOINT` wrapper derives its own graph directory and ignores `graph.location`, so the serving container re-imported into `/data/default-gh` and discarded the prepared graph | `docker-compose.yml` (explicit `entrypoint:`) |
| 3 | `elevation: true` must be sent per request; a 3D graph alone returns 2-ordinate coordinates while still reporting `ascend` | spec §8 table, `tests/smoke/test_graphhopper.sh` |
| 4 | `osmdata.openstreetmap.de` throttles DigitalOcean SGP1 to ~10 kB/s (~26 h for the 927 MB water polygons) while serving 4.3 MB/s elsewhere. Planetiler renders this as a stalled progress bar, never an error | `Makefile` (throughput floor + side-load instructions), `docs/runbook.md` |
| 5 | The `z10/822/526` tile this plan specified for the "East Java" content assertion is open water in the Java Sea and serves 163 bytes. Surabaya is `z10/832/532` | `tests/smoke/test_tiles.sh` |
| 6 | `assert_http_ok`'s `curl … \|\| echo 000` double-counted to `000000`, which would have made the port-exposure check fail on its *passing* value | `tests/smoke/lib.sh` (`http_code` helper) |

Deviations from the stated budget, both benign:

- **Artifacts total 25 GB, not ~22 GB** — `/data/dem` is 18 GB rather than the estimated 8 GB, and `/data/sources` (1.4 GB of Planetiler inputs) was unbudgeted. The droplet has 309 GB, so 275 GB remains free.
- **No block-storage volume** — see `docs/runbook.md`; the droplet ships 320 GB of local SSD.

## Definition of done

`make all && make verify` on a fresh droplet exits 0, with:

- ~22 GB of artifacts under `/data`
- Photon geocoding "Bromo" to a point inside Indonesia
- PostGIS answering an indexed bbox+tag query with a plausible volcano and beach count
- GraphHopper routing Surabaya→Malang **and accepting a runtime `custom_model`**
- PMTiles serving a non-empty z10 tile over Java
- TLS terminating at Caddy with no internal port publicly reachable

## Self-review notes

- **Spec coverage:** §6 pipeline → Tasks 3, 5, 6, 7, 8, 9. §5 architecture (Compose, Caddy) → Tasks 1, 10. §7 RAM budget → `mem_limit` in Tasks 5, 6, 8, 9. §4 decisions 2, 3, 11, 12 → Tasks 1, 2. Not covered here by design: §8–§11 (tools, agent loop, frontend) belong to Plans B and C; §14 Phase 2 is out of scope.
- **Two spec corrections surfaced:** the LM-vs-CH issue (Task 8) and the Photon version gate (Task 4). Both need folding back into the spec.
- **Naming consistency checked:** `REGION_SLUG`, `DATA_DIR`, `PHOTON_COUNTRY`, `REGION_BBOX`, `SITE_HOST` are used identically in `.env.example`, every script, the Makefile, and compose.
