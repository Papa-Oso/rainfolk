# Local historical-record map bridge

Use a new `usgwd-field-quality-v2` normalization directory. Earlier generations
and original CSV bytes remain immutable. Invalid measurements have an explicit
field error and no usable numeric value; original strings remain available.
Structural and identity conflicts still quarantine all affected versions.
The already locked serde_json package enables `float_roundtrip` (no new package
or version) so re-reading finite measurements preserves their exact binary64
values and content hashes. The precision regression failed without this feature.

After the documented offline build:

```sh
well_index --usgwd normalized/usgwd-segmented-manifest.json map/states/New_Hampshire
well_index --merge-usgwd state-indexes.json map
```

Use the crate's executable path; `well_index` is not installed globally. A
single-file `usgwd-manifest.json` also works. The bridge verifies manifest and
partition hashes, record content/provenance and the authoritative reconciled ID
index before reusing the existing detail packs and six count grids. Raw row
partitions are never concatenated as accepted IDs. Identical source IDs coalesce;
conflicting versions do not leak into the map. Coordinates alone never merge IDs.

Each state index invocation is bounded by two million source rows, 10,016
200-row partition references, 8 MiB per artifact, 40,000 detail references,
16 GiB logical output and 300 seconds. Use an external watchdog for blocking
local I/O. A failed candidate leaves the last-good index pointer. There is no
network, scheduler, publication or source acquisition in these commands.
Historical export alone accepts an optional final deadline argument, e.g.
`well_index --usgwd manifest.json new-output 900`. The default stays 300 seconds;
explicit values must be 1..900. Input verification, candidate processing and final
writes share that same absolute invocation budget. Other modes remain at 300.
Use a matching external watchdog and an explicit nonrenewable job reservation;
this option does not renew an old attempt or authorize a retry. Record the chosen
limit and measured runtime. Row, memory, artifact and output limits are unchanged.
Historical rows share a bounded in-process cell buffer: flush thresholds are
10,000 rows or 64 MiB serialized row data globally, and 500 rows or 4 MiB per
cell. One bounded incoming row may cross a threshold before immediate flush.
This reduces durable tiny-file writes; the existing final compaction, identity
reconciliation, artifact limits and last-good pointer remain authoritative.

The merge descriptor contains `states: [{state, path, sha256}]`, with each path
exactly `states/<underscore_state_name>/index.json` under the new output root.
Include a separate `blockedStates` list. At most 50 state indexes and 20 million
source rows are admitted. They must share one immutable release, and their
release-qualified ID ranges must be disjoint. An overlap refuses the merge and
requires explicit reconciliation; it never resolves by location or state-ID text.

The root has global partitioned counts and hash/byte/count-pinned child indexes.
It does not duplicate all state detail files or expose a huge flat national
region list. The consumer uses complete global counts at broad scales. Near
views may read at most eight intersecting child indexes and 16 MiB of child-index
metadata, retaining the existing 56 MiB viewport, 2,000-detail-record and 16 MiB
detail budgets. If this cannot fit, complete counts remain visible. Federation
does not replace partially loaded aggregate cells with incomplete detail.

`catalogRecords`, `indexedRecords`, `unmappedRecords` and quarantine/duplicate
accounting are separate. `distinctPhysicalWells` remains null. The map counts
mapped source records, not unique physical wells or current active wells. The
USGS corpus is unchanged and is not silently merged with USGWD. Ambiguous state
IDs remain aliases with unresolved source-system/physical identity. Nebraska and
South Carolina remain acquisition blockers independent of other states.

Dense historical half-degree cells are split by reading one hash-verified initial
tile at a time (at most 500 rows / 8 MiB). Collection before recursive detail
splitting is limited to 2,000 records. At maximum depth, coincident records are
emitted in separate bounded packs rather than merged or loaded as one large cell.
Subdivision coalesces each quadrant's incoming rows across source tiles before
writing its next stage. Each of four buffers holds at most 500 rows and 4 MiB of
serialized row data; a single larger row is flushed alone under the unchanged
8 MiB artifact cap. This avoids hundreds of tiny, synced intermediate files for
fragmented inputs. Headers must match; source IDs, coordinates and provenance are
retained. Durable immutable writes and all existing resource guards still apply.
This retains the existing 300-second and 16 GiB logical-output guards. A guarded
failure leaves the prior published index unchanged; it never authorizes a retry
or a larger budget by itself.

## Coordinates and uncertainty

Source longitude/latitude strings retain NAD83 EPSG:4269. Display coordinates use
explicit EPSG:9603 geocentric translations, GRS80 to WGS84, zero intermediate
ellipsoidal height and discarded output height. Source elevations are not used
as ellipsoidal heights. No epoch, realization refinement or survey accuracy is
invented. Operation accuracy is separate from the unknown source location error.

| Geographic operation area | Operation | Translation X,Y,Z, metres | Stated operation accuracy |
| --- | --- | --- | --- |
| CONUS and mainland Alaska within operation extent | EPSG:1188 | 0,0,0 | 4 m |
| Hawaii main islands within operation extent | EPSG:1252 | 1,1,-1 | 4 m |
| Aleutian operation extent, including antimeridian | EPSG:1251 | -2,0,4 | 8 m |

Out-of-area or source-inconsistent coordinates remain catalog records and do
not become display points. Original coordinates, raw quality flags, operation,
assumptions and provenance accompany mapped records. Hollow amber markers and
detail warnings identify unverified source locations. A GPS/field-checked source
flag does not establish surveyed-wellhead accuracy. Negative quantities are never
repaired with `abs()` or inferred missing-value codes.

Primary references: [PROJ's EPSG transformation parameters](https://github.com/OSGeo/PROJ/blob/master/data/sql/helmert_transformation.sql),
[operation extents](https://github.com/OSGeo/PROJ/blob/master/data/sql/extent.sql),
and [geocentric Helmert operations](https://proj.org/en/stable/operations/transformations/helmert.html).

## Consumer and checks

The optional [source patch kit](../integrations/gev/README.md) extends the existing
GEV count/detail consumer. It is separate from the producer and the original GEV
installation. Source-specific admission preserves strict USGS validation; a
historical acquisition never exempts other records from their own validator.
The water-only shell reuses the existing renderer, controls and local Natural
Earth imagery. No new map engine, API subscription or backend is introduced.

Synthetic checks cover quantity warnings, cross-segment conflicts, original
coordinates, regional operations, overlapping count cells, ID-range collisions,
tampering and last-good preservation. Run the existing aggregate checks plus:

```sh
node --test integrations/gev/historicalPack.test.mjs
```

Actual state exports and browser evidence remain local/ignored. Public source
review is separate from data publication, hosting capacity, merge and deployment.
EOF does not establish complete state coverage; missing records never mean no
wells or plugged wells. Map information does not establish water safety or rights.
