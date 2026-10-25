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
