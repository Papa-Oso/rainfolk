# Producer and consumer boundary

`gev-us-groundwater` reads bounded capture JSON on stdin and writes a validated
pack, or `--bundle-v1` inventory/observations/manifest bundle. Inventory and
observations remain separate. `--inventory-bundle-v1 <two-digit-state>` accepts
inventory captures. Unknown units/reference are not invented; signed observations
retain their semantics. Rust source and contract tests are authoritative.

`well-jobs run|resume|status <queue.json> <work-directory>` uses a finite queue and
validated checkpoints. Captures, response hashes, segments, candidate provenance,
counts and opaque next links are retained. Budget consumption means partial,
not complete; only exhausted pagination completes a jurisdiction. Failed or
malformed records are refused/quarantined explicitly, never made into fake points.
Legacy checkpoints lacking absolute expiry cannot continue network work silently.

`well_index <batches.json> <output-directory>` consumes a descriptor with
`snapshot` provenance and `candidates: [{path, sha256}]`, relative to that descriptor.
It validates every referenced candidate, deduplicates stable source IDs, records
conflicting versions in an audit, emits content-addressed detail packs and complete
multilevel counts, then replaces the index pointer only after validation.
Failed validation retains the last-good pointer. Supply the full desired candidate
set for an update; this command is not a database or automatic publisher.
Each output directory permits one writer via exclusive `.well-index.lock`.
Normal completion/errors release it. A crash leaves it behind and continuation
fails closed: verify no writer survives before any deliberate operator cleanup;
there is no age-based lock stealing. Immutable tiles, counts, audits and rollback
indexes use unique staged files, flush and atomic publication. Orphan staging
files are not index inputs. Coverage text follows actual readings/acquisition
collections; an observation-bearing example never claims no observations.

The separate GEV JavaScript/Cesium consumer validates hashes/contracts and loads
bounded visible-area detail or complete counts. Source coverage is partial;
unrendered or absent records never mean no wells. The app needs no Cargo runtime,
collector key, database or Rainfolk service. No GEV application is bundled here.
See GEV_INTEGRATION.md for the separate delivery/reproducibility gate.

## Partitioned count summaries (local format extension)

The index remains `gev-well-spatial-index`, schema version 1. Its `aggregates`
reference now declares schema version 2 and kind `gev-well-partitioned-counts`.
It references one immutable `overview` with the first four existing grid levels
(32, 8, 2 and 0.5 degrees). The finer levels (0.125 and 0.03125 degrees) contain
`chunks` anchored to existing 2-degree parents. Every reference has URL, SHA-256
and byte length; fine references also have parent bounds, population and occupied
cell count. Global total is catalog records, never a screen count or complete
physical-well census.

Chunk bodies reuse the count-grid/cell representation with one level,
`parentBounds` and a parent-local `total`. Counts and record-weighted centers
reconcile to that overview parent. Every level reconciles to the retained stable
IDs before publication. Unchanged subtrees reuse hashes even when another area's
population changes. Count files are capped at 8 MiB and use the existing immutable
staged write path and writer lock. Previous monoliths/index backups are retained.

The updated GEV consumer accepts both formats. It first reads the overview, then
only intersecting fine subtrees within bounded byte/cell/request budgets;
otherwise complete coarser counts remain available. Stale/failed work cannot
replace the last-good display. Renderer, IDs, readings, provenance and incomplete
source coverage are unchanged. This extension is local/uncommitted pending
separate review; published a815b55 still emits the earlier format.

Local full-corpus verification retained all 1,042,777 source IDs, complete record
fingerprints and acquisition provenance fingerprints. Every one of the six levels
reconciles to those records. Output: 663 summary files (one 366,923-byte overview
plus 331 chunks at each fine level), largest fine chunk 320,637 bytes. These are
local measurements of the pinned historical corpus, not current live coverage or
an automatic public data release. No collection, new dependency or service was
used. The compatible consumer enforces exact chunk byte lengths/hashes, adjacent
hierarchy/weighted-center conservation, complete subtrees and bounded caching.
