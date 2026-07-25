# Natural-Language Trip Planner — Design

**Date:** 2026-07-25
**Status:** Approved, ready for implementation planning
**Supersedes:** the unapproved 2026-07-17 scenic-routing design (local Docker, Indonesia-first)

## 1. Context

A personal open-data map that plans trips from natural language. The origin motivation was that no map optimizes for scenic routes; that generalized during design into a broader idea: **you describe the trip you want in prose, and an LLM agent plans it** using real routing, geocoding, and POI search over OpenStreetMap.

"Scenic" stops being a hardcoded mode and becomes one of infinitely many expressible preferences.

Motivating example:

```
"3-day drive through Java, volcanoes and beaches, max 4h driving per day"
  → geocode("Java") → bbox
  → search_poi([natural=volcano, natural=beach], bbox) → 47 hits
  → cluster into day candidates
  → route(leg) → 4h12m ✗ over budget → re-plan → 3h40m ✓
  → Day 1: Bromo → Malang    (3h40m)
    Day 2: Malang → Pacitan  (3h55m)
    Day 3: Pacitan → Jogja   (2h30m)
```

This is a **personal tool, not a product**. Polish and audience are secondary; correctness and the quality of the routes are what matter.

## 2. Goals

- Plan multi-day, multi-stop trips from a prose description.
- Honor hard constraints (max driving time per day, avoid tolls, avoid highways).
- Honor soft preferences expressed in language (twisty, coastal, scenic, quiet).
- Draw results on a map incrementally, as the agent works.
- Run entirely on open data — no commercial map APIs.

## 3. Non-goals

- Public multi-tenant service, user accounts, or auth beyond basic access control.
- Turn-by-turn live navigation.
- Real-time traffic (no probe-data fleet exists for this project).
- Mobile apps.

## 4. Decisions

| # | Decision | Rationale |
|---|---|---|
| 1 | NL layer is an **agentic trip planner**, not a single-shot NL→weights compiler | Strictly a superset — the compiler capability lives inside it as the `custom_model` argument to `route` |
| 2 | Deploy to a **DigitalOcean droplet** (`s-8vcpu-16gb`, sgp1, $96/mo). **No block storage** | Local laptop had 10 GB free on a 99%-full disk. *Amended 2026-07-25 during implementation:* the droplet ships 320 GB of local SSD, so the planned 100 GB volume was dropped — everything under `/data` is reproducible via `make all`, so a rebuild costs re-import time, not unique state. Reversal procedure in `docs/runbook.md` |
| 3 | **Docker Compose**, but every service strictly 12-factor | Four stateful services pinned to one machine's disk. K8s adds a control plane and PVCs for zero benefit at one node; 12-factor discipline keeps the later translation cheap |
| 4 | **Rust + axum** for the API and agent service | Mostly I/O orchestration; single static binary deploys cleanly |
| 5 | **Raw HTTP** to the Anthropic API, hand-rolled agent loop | No official Rust SDK exists. Community crates lag the API and would not expose the two beta features this design uses |
| 6 | **Photon 0.7.4 + embedded Elasticsearch** for geocoding | Fuzzy, multilingual, typo-tolerant place search with real ranking. *Amended 2026-07-25:* originally specified Photon 1.x + OpenSearch, but the only published Indonesia extract is an **Elasticsearch** index that will not load on the 1.x/OpenSearch line — empirically verified. Full evidence and rejected alternatives in `docs/decisions/photon-index-source.md` |
| 7 | **PostGIS** (osm2pgsql flex) for POI category search | OSM tagging is a finite documented vocabulary an LLM maps onto near-perfectly. Exact, fast, fully explainable |
| 8 | **GraphHopper** for routing, with JSON CustomModels. Graph prepared with **Landmarks (LM) as well as CH** | CustomModel is a bounded DSL over encoded values — a clean compilation target for natural-language preferences. *Amended 2026-07-25:* CH bakes the cost function into the preprocessed graph and **cannot serve an arbitrary runtime `custom_model`**, which the `route` tool sends on every call. LM can. Requests carrying a custom model must send `ch.disable: true`; CH remains for the fixed fast path |
| 9 | **Skadi** for elevation, not Copernicus DEM | GraphHopper-native built-in provider. Copernicus would need a custom provider for no Phase-1 benefit |
| 10 | **Planetiler → PMTiles**, served over HTTP range requests | Single-file basemap, no tile server database |
| 11 | **Git push → rebuild on host** dev loop | Repo stays the source of truth; nothing lost if the droplet dies |
| 12 | Coverage: **Indonesia**, planet-ready pipeline | ~22 GB of artifacts fits the volume with room to spare |
| 13 | Scenic scoring is **Phase 2**, gated behind a spike | See §14 |

### Rejected during design

- **`pg_trgm` instead of Photon** — considered to drop a service, rejected: trigram similarity gives scoring but not the linguistic handling or ranking model that prose→place resolution needs.
- **Overpass API for POIs** — rate-limited external dependency in the agent's hot loop.
- **Vector/embedding POI search** — an embedding pass over millions of POIs plus non-deterministic relevance, for marginal gain over a controlled tag vocabulary. Addable later via `pgvector` without rework.
- **Kubernetes now** — see decision 3.
- **Photon 0.7.x on Elasticsearch** — satisfies "keep Elasticsearch" literally but pins to an unmaintained line with a ~40% larger index.

## 5. Architecture

Two strictly separated concerns: an **offline pipeline** producing artifacts on the block volume, and an **online stack** serving them.

```
┌─ OFFLINE (Makefile, runs on droplet) ─────────────────────┐
│  Geofabrik indonesia-latest.osm.pbf                       │
│    ├─ Planetiler          → /data/tiles/indonesia.pmtiles │
│    ├─ osm2pgsql (flex)    → PostGIS: osm_poi, osm_place   │
│    ├─ GraphHopper import  → /data/graph/  (LM + CH + elev)│
│    ├─ Photon dump fetch   → /data/photon/                 │
│    └─ scenic_score.py     → per-way scores      [Phase 2] │
└───────────────────────────────────────────────────────────┘
                             ↓ artifacts on /data
┌─ ONLINE (docker compose) ─────────────────────────────────┐
│  caddy         :443   TLS, reverse proxy, static frontend │
│    ├─ api      :8000  Rust/axum — agent loop + tools      │
│    ├─ graphhopper :8989  routing (read-only /data/graph)  │
│    ├─ photon   :2322  geocoding (embedded OpenSearch)     │
│    ├─ postgis  :5432  POI search                          │
│    └─ tiles    :8080  PMTiles over HTTP range requests    │
└───────────────────────────────────────────────────────────┘
```

**The agent never touches the graph or the database directly.** It calls four narrow tools, each a thin testable function over one backing service. That boundary is the core of the design: the LLM does intent and planning; deterministic services do geometry and search.

## 6. Data pipeline

One `Makefile`, one target per artifact, each idempotent and independently re-runnable.

Sizes below are **measured** as of 2026-07-25 except where marked estimated; the
original spec estimates ran high, several by an order of magnitude.

| Target | Produces | Size |
|---|---|---|
| `make fetch` | `/data/osm/indonesia-latest.osm.pbf` | 1.7 GB *(est. 1 GB)* |
| `make tiles` | `/data/tiles/indonesia.pmtiles` | *est. ~4 GB (+~5 GB transient)* |
| `make db` | PostGIS `osm_poi` (606k rows), `osm_place` (117k rows) | **190 MB** *(est. 4 GB)* |
| `make graph` | `/data/graph/` | *est. ~3 GB* |
| `make dem` | `/data/dem/` (Skadi tiles) | ~1.7 GB and counting *(est. 8 GB)* |
| `make photon` | `/data/photon/` | 903 MB |
| `make all` | all of the above | **~10 GB projected** *(est. 22 GB)* |

The `db` figure is the one worth internalising: filtering to POI and place tags in
the osm2pgsql flex config produced 190 MB rather than the estimated 4 GB, which is
the difference between a table that stays in cache and one that does not.

### osm2pgsql configuration

Flex mode with a Lua config filtering to POIs and places only — **not** a full OSM import. A full import would be 20 GB+ and provide nothing the agent uses. Retained tag classes:

`natural=*`, `tourism=*`, `amenity=*`, `historic=*`, `leisure=*`, `place=*`

Indexes: GiST on `geom`, GIN on the `tags` hstore/jsonb column.

### Photon index

Prebuilt dump, no Nominatim build required:

```
https://download1.graphhopper.com/public/extracts/by-country-code/id/
  photon-db-id-250720.tar.bz2    452 MB compressed
```

**Known caveat:** country extracts are not refreshed on the planet dumps' weekly cadence — the Indonesia extract is dated 2025-07-20. Acceptable for natural features and established places; stale for new businesses. If freshness becomes a problem, the fallback is building from the planet JSONL dump (26 GB) filtered to an Indonesia bbox.

### Import-time resources

Planetiler and the GraphHopper import want more RAM than steady-state serving. Run imports with the serving stack stopped, or temporarily resize the droplet for the import and scale back after — DO bills hourly, so an import costs cents.

## 7. Runtime resource budget (16 GB droplet)

| Service | RAM |
|---|---|
| GraphHopper (Indonesia, CH) | 4–6 GB |
| Photon + embedded OpenSearch (`-Xmx4G`) | ~4 GB |
| PostGIS | 2–3 GB |
| PMTiles server | ~0.5 GB |
| Rust API | ~0.2 GB |
| OS + headroom | ~2 GB |

Fits, with limited slack. The 64 GB figure in Photon's documentation is the **planet** recommendation and does not apply to a country extract.

## 8. Agent tools

Four tools, all declared with `strict: true` (plus `additionalProperties: false` and explicit `required`), so inputs are schema-guaranteed and the Rust side deserializes into concrete structs without defensive parsing.

| Tool | Backing service | Returns |
|---|---|---|
| `geocode(query, bbox?)` | Photon | ranked place candidates + coordinates |
| `search_poi(tags[], bbox, limit)` | PostGIS | POIs with name, coordinates, tags |
| `route(waypoints[], profile, custom_model?)` | GraphHopper | geometry, distance, duration, elevation |
| `elevation_profile(geometry)` | GraphHopper | ascent/descent, slope samples |

`route`'s `custom_model` is where preference expression lives — a GraphHopper CustomModel JSON the agent composes from the user's language, over encoded values `road_class`, `road_environment`, `surface`, `curvature`, `average_slope`, `toll`, `track_type`:

```json
{
  "priority": [
    {"if": "road_class == MOTORWAY", "multiply_by": 0.05},
    {"if": "toll != NO",             "multiply_by": 0.1},
    {"if": "curvature < 0.8",        "multiply_by": 0.4}
  ],
  "distance_influence": 30
}
```

## 9. Anthropic API request shape

Model `claude-opus-5`, raw HTTP to `POST /v1/messages`.

```json
{
  "model": "claude-opus-5",
  "max_tokens": 64000,
  "stream": true,
  "output_config": {
    "effort": "high",
    "task_budget": { "type": "tokens", "total": 120000 }
  },
  "fallbacks": "default",
  "system": [{ "type": "text", "text": "...", "cache_control": { "type": "ephemeral" } }],
  "tools": [ /* the four tools, strict: true */ ],
  "messages": [ ... ]
}
```

Headers: `anthropic-beta: task-budgets-2026-03-13, server-side-fallback-2026-07-01`

Rationale for each non-obvious field:

- **No `temperature` / `top_p` / `top_k`** — Opus 5 rejects them with a 400. Steering is prompt-only.
- **Thinking is on by default** on Opus 5 and counts against `max_tokens`, so 64000 covers thinking plus output. Add `"thinking": {"type": "adaptive", "display": "summarized"}` to surface reasoning in the UI; the default emits empty thinking blocks, which streams as a silent pause.
- **`task_budget`** caps the whole agentic loop rather than one response — the model sees a countdown and wraps up gracefully instead of being cut off. Minimum 20000.
- **`fallbacks: "default"`** — Opus 5's safety classifiers can decline with `stop_reason: "refusal"` on an HTTP 200; without a fallback the request simply stops.
- **`cache_control` on the system block** — the loop resends the full conversation each turn, so caching the system prompt and tool definitions is the difference between paying for them once versus once per tool call. Opus 5's minimum cacheable prefix is 512 tokens.
- **`effort: "high"`** as the starting point, to be swept against real queries. Opus 5 performs unusually well at `low` and `medium`; prior-model effort defaults rarely transfer.

## 10. Agent loop

One Rust module, narrow interface: conversation in, stream of events out.

```
POST /api/chat  ──► SSE stream to browser
  │
  ├─ loop (max 12 iterations, hard cap):
  │    POST api.anthropic.com/v1/messages  {stream: true}
  │      ├─ forward text deltas          → SSE: {type:"text", ...}
  │      └─ accumulate content blocks
  │
  │    match stop_reason:
  │      "refusal"    → SSE {type:"refused"};  break
  │      "end_turn"   → SSE {type:"done"};     break
  │      "pause_turn" → append assistant turn; continue
  │      "tool_use"   → ▼
  │
  │    execute all tool_use blocks CONCURRENTLY (tokio JoinSet)
  │      ├─ push results to the map immediately:
  │      │    route(...)      → SSE {type:"geometry", geojson}
  │      │    search_poi(...) → SSE {type:"markers",  geojson}
  │      ├─ push assistant content (incl. tool_use blocks) to messages
  │      └─ push ONE user message containing ALL tool_result blocks
  └─ ◄ loop
```

### Invariants

Each of these is a real bug if violated:

1. **Append the full `content` array**, not extracted text. Dropping `tool_use` blocks breaks the next request — every `tool_use` needs a matching `tool_result`.
2. **All `tool_result` blocks go in a single user message.** Splitting them across messages silently teaches the model to stop making parallel tool calls, which is exactly the behavior wanted for "volcanoes AND beaches."
3. **A failed tool returns `tool_result` with `is_error: true`** — never dropped, never a hard failure. The agent reads the error and adapts.
4. **Check `stop_reason` before touching `content`.** On a refusal `content` may be empty.
5. **Retry 429 and 5xx with exponential backoff + jitter.** The official SDKs do this automatically; raw HTTP means we own it.

The map updates **as** the agent works rather than after — the reason for structured SSE events rather than a plain text stream.

## 11. Frontend

MapLibre GL JS + the `pmtiles` protocol shim. No framework; static files served by Caddy.

```
┌──────────────────────────────┬──────────────┐
│                              │  chat        │
│                              │  ┌────────┐  │
│         MapLibre             │  │ 3-day  │  │
│    (PMTiles basemap,         │  │ drive… │  │
│     route + POI layers)      │  └────────┘  │
│                              │  ▸ geocoding │
│                              │  ▸ 47 POIs   │
│                              ├──────────────┤
│                              │  itinerary   │
│                              │  Day 1 3h40m │
│                              │  Day 2 3h55m │
└──────────────────────────────┴──────────────┘
```

Three MapLibre sources: basemap (PMTiles), route (GeoJSON, replaced per `route` result), POIs (GeoJSON). Chat panel shows tool activity live so a multi-minute plan does not look hung. Itinerary cards are click-to-highlight against the map.

## 12. Error handling

| Failure | Handling |
|---|---|
| GraphHopper: no route between points | `is_error` tool_result → agent picks a different waypoint |
| Photon: no match / ambiguous | Return top-N candidates → agent disambiguates or asks the user |
| `search_poi` returns zero rows | Empty result, **not** an error → agent widens bbox or relaxes tags |
| Anthropic 429 / 5xx | Backoff + jitter, capped retries |
| Anthropic refusal | Surface plainly to the user; do not retry the same prompt |
| Agent fails to converge | `task_budget` graceful wrap-up + hard 12-iteration cap |
| Long request (minutes) | Streaming keeps the connection alive; generous `reqwest` timeout |

## 13. Testing

Test-driven, in three layers:

1. **Tool functions** — integration tests against a small fixture region (Bali extract, ~50 MB) in Docker. Real Postgres, real GraphHopper, real Photon. No mocking.
2. **Agent loop** — tested against a **mock Anthropic responder** replaying canned `tool_use` / `end_turn` / `refusal` / `pause_turn` sequences. This is the critical layer: the loop is the riskiest code and is fully testable with zero API calls and zero cost. Every invariant in §10 gets a test.
3. **Behavioral goldens** — known query → assert route **properties**, not geometry. `"avoid tolls"` → assert zero toll segments. `"twisty backroads"` → assert mean curvature above baseline and zero motorway segments. Robust to OSM data drift in a way coordinate comparison is not.

## 14. Phasing

### Phase 1 — working planner

1. Droplet provisioned, block storage mounted at `/data`, Compose stack up.
2. Pipeline runs end to end for Indonesia; all six artifacts present.
3. Four tools implemented and integration-tested against the Bali fixture.
4. Agent loop implemented, all §10 invariants under test.
5. Frontend renders basemap, streams chat, draws routes and POIs live.

**Phase 1 milestone:** the Java example in §1 produces a real three-day itinerary with routes drawn on the map.

Phase 1 delivers genuinely preference-aware routing using GraphHopper's built-in encoded values alone. Scenic scoring is not required for it.

### Phase 2 — landscape scenic scoring

A Python batch step (osmium + rasterio over the Skadi DEM) computes a 0–100 scenic score per OSM way from elevation variance, coastline proximity, forest and water land cover, and viewpoint density — fed to GraphHopper as a custom encoded value so `custom_model` can weight on it.

Python rather than Rust for this one step: it is an offline batch job outside the serving path, and `osmium` / `rasterio` bindings are considerably more mature there than `osmpbf` / `gdal` are in Rust.

**Prerequisite spike (timeboxed, blocks Phase 2 only):** determine how to get a custom encoded value into GraphHopper for the installed version. Depending on version this may require a small Java `TagParser` plugin rather than pure configuration. Spike outcome is one of:

- (a) pure config — proceed as designed;
- (b) small Java plugin — proceed, accepting a Java build step in the pipeline;
- (c) neither is tractable — fall back to rewriting scores as synthetic OSM tags in the PBF before import.

## 15. Cost

| Item | Monthly |
|---|---|
| Droplet `s-8vcpu-16gb` (16 GB / 8 vCPU / 320 GB SSD, sgp1) | $96 |
| ~~Block storage 100 GB~~ — dropped, see decision 2 | ~~$10~~ |
| Anthropic API (Opus 5, $5/$25 per MTok) | usage-based; a trip plan is a few cents |
| **Fixed total** | **$96** |

## 16. Open items

Only one, and it blocks Phase 2 exclusively: the **GraphHopper custom-encoded-value spike** described in §14. Everything in Phase 1 is fully specified.
