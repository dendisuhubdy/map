# Decision: Photon index source and version

**Date:** 2026-07-25
**Status:** Decided — branch **(b1)**, pin Photon 0.7.4 on Elasticsearch
**Gate:** Task 4 of the Phase 1A plan. Closed.

## Question

The design spec (decision 6) chose **Photon 1.x with embedded OpenSearch**. The only
Indonesia index extract published by GraphHopper lives in a tree that is *not*
version-segmented, so its compatibility with the 1.x line was unverified. Task 4 existed
to resolve that empirically before anything depended on it.

## Evidence

Extract under test:

```
https://download1.graphhopper.com/public/extracts/by-country-code/id/photon-db-id-250720.tar.bz2
453 MB compressed → 902 MB extracted, MD5 verified OK
```

Its internal layout settles the question on its own:

```
/data/photon/photon_data/elasticsearch/{modules,data,plugins,config}
```

That is an **Elasticsearch** index. Photon 1.x is OpenSearch-only.

Both lines were then run against it on the droplet, in `eclipse-temurin:21-jre`,
`-Xmx4G`:

| JAR | Line | Result |
|---|---|---|
| `photon-1.2.1.jar` | 1.x / OpenSearch | **Failed** — container exited immediately, never served |
| `photon-0.7.4.jar` | 0.7.x / Elasticsearch | **Started and served** |

Query evidence from the working configuration:

```
GET /api?q=Bromo&limit=1
→ { "n": 1, "name": "Gunung Bromo", "coords": [112.9529769, -7.9420691] }
```

Correct: Mount Bromo, East Java.

## Decision

**(b1)** Pin Photon to **0.7.4** (`photon-0.7.4.jar`, the Elasticsearch build) and use the
published country extract as-is.

Note this restores the original request. The user asked to "keep Elasticsearch"; the spec
moved to OpenSearch on the reasoning that 1.x is the maintained line with ~40% smaller
dumps. **Neither premise holds on the country-extract path** — no 1.x-compatible Indonesia
extract exists — so the reasoning that produced decision 6 does not apply here.

Rejected:

- **(b2) Build an Indonesia index from the 1.x planet JSONL** (~26 GB, filtered to bbox,
  then imported). Would deliver Photon 1.2.1, OpenSearch, and a weekly-fresh index. Costs
  a 26 GB transient download and a permanent import stage in `make photon`, in exchange
  for freshness this use case barely benefits from — the geocoding targets are volcanoes,
  beaches, and established towns.

## Known limitations accepted

1. **Index date is 2025-07-20.** Country extracts are not refreshed on the planet dumps'
   weekly cadence. Natural features and established places are unaffected; newly-added
   businesses will be missing.
2. **0.7.4 (2025-09-18) is not the maintained line.** No upstream fixes will be picked up
   without revisiting this decision.

## Revisit if

- Geocoding quality proves inadequate in real use once the planner works end to end.
- A 1.x-compatible country extract for `id` appears under `by-country-code/`.
- Coverage expands beyond Indonesia, at which point the planet-JSONL path (b2) may be
  worth its cost anyway.

## Spec impact

Design spec decision 6 must be amended from "Photon 1.x + embedded OpenSearch" to
"Photon 0.7.4 + embedded Elasticsearch". Tracked with the other pending spec corrections.
