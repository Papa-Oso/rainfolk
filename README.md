# Rainfolk

Rainfolk prepares public well records for map layers, with source attribution,
quality warnings and a traceable processing history. The current priority is a
nationwide well-data feed for the independent God's Eye View application.

## Current status

The repository contains the Rust importer/indexer, bounded Python tools,
synthetic examples and a [GEV integration kit](integrations/gev/README.md).
It does not bundle the full GEV application or real source datasets. Website
development is separate from this data/layer work.

Verified historical USGWD status, October 31, 2026:

| Release | States | Catalog records | Mapped records | Unmapped records |
| --- | ---: | ---: | ---: | ---: |
| Validated local candidate; not published | 48 | 13,980,064 | 12,171,830 | 1,808,234 |
| Currently published version | 36 | 7,072,581 | 5,918,051 | 1,154,530 |

The [published 36-state feed](https://data.rainfolk.org/usgwd/f7e59fb0a049b3a1990cc0fd6862190b8cc6aeb370989fd41ac0300b9ee6d336/index.json)
is an immutable version. The local 48-state candidate closes its state-export
gaps, including Texas, but still needs publication approval. Nebraska's publisher
file is not ready, and South Carolina's download failed checksum verification;
neither is included in the 48 collected states.

These are historical source records, not a count of distinct physical wells or
a current census. The [USGWD contract](docs/USGWD.md) preserves release-scoped IDs,
original NAD83 coordinates, source dates and quality flags. The
[map bridge](docs/USGWD_MAP.md) records the display-coordinate operation, retains
invalid measurements as warnings and keeps records without usable locations in
catalog accounting. Approximate/unverified locations are not surveyed wellheads.

The GEV preview is under review. Full visual acceptance and approval are required
before submitting the integration upstream; no upstream PR is implied by local
data or source tests. Existing standalone GEV installations remain separate.

## Current local pipeline

The local implementation uses Rust with deterministic transport scripts. Its current pipeline is:

**collect -> validate -> spatial index -> God's Eye View layer**

Collection, validation and indexing are deterministic; there are no AI calls per
well record. AI may assist development or source research.

Rainfolk is intended to be useful without depending on a founder-operated service. Exportable data, source attribution, and provenance are part of that goal. Data views should make gaps and source, retrieval, and measurement times explicit.

The tested [local refresh path](docs/SNAPSHOT_REFRESH.md) detects changed CSV
snapshots by hash, resumes imports, compares source IDs and exports only changed
states while reusing other state indexes. Missing records are flagged for review,
never interpreted as plugged wells. This is complete-snapshot comparison, not
incremental fetching from agency changefeeds. Remote acquisition profiles and
schedule activation remain to be completed; the executable scheduler entry point
is inactive and does not install a service or publish a feed.

## Coverage and limits

The separate USGS inventory snapshot, dated **October 25, 2026**, remains unchanged:

- **1,042,777** distinct USGS monitoring-location IDs
- **44** jurisdictions with pagination complete for this source
- **7** jurisdictions with pagination partial for this source
- **9** quarantined records

This USGS inventory is location metadata. No water-level time-series observations were fetched in this collection. Pagination complete describes retrieval progress for this source; it does not mean all wells or relevant records have been found.

Rainfolk is not a complete registry of private wells. Its information does not establish groundwater availability, drilling suitability, or whether water is potable. Consult original sources and qualified local authorities for decisions that require current or site-specific information.

## Contributing source leads and quality fixes

Useful contributions include links to public water-information sources and corrections to source metadata, coverage, or retrieval results. Please include the source URL, the agency or publisher, the area covered, and the date you checked it.

Do not submit private well-owner details, credentials, or other sensitive information.

## Security and licensing

Some live USGS requests require credentials. Offline examples and the static GEV
feed do not. Keys and other secrets must never be committed or placed in browser code.

Newly authored Rainfolk code and documentation are MIT licensed, copyright 2026 Joshua Koelker; see LICENSE. Third-party code and source data retain their separate terms and notices. This license does not cover datasets or unrelated third-party material. Third-party terms and notices are preserved.

## Roadmap

- Publish the validated 48-state feed after approval and finish GEV visual review.
- Prepare the scoped upstream layer contribution after the user's review.
- Add verified official-state acquisition profiles and activate refresh scheduling only after approval.
- Keep source attribution, historical dates, uncertain locations and coverage gaps visible.

## Links

- [USGS monitoring-location API documentation](https://api.waterdata.usgs.gov/docs/ogcapi/) - current source documentation.
- [God's Eye View](https://github.com/bilawalsidhu/gods-eye-view) - independent upstream project; no official partnership is implied.
- [Terlingua Water Atlas](https://github.com/Papa-Oso/terlingua-water-atlas) - historical research and catalog snapshot, not Rainfolk's current codebase or a rebranded version of this project.

## Run the offline example

Tested toolchain: rustc 1.99.1 and Cargo 1.99.1 on Windows; no older MSRV or other platform is claimed tested. Prerequisites: a Rust toolchain supporting edition 2024, Cargo and Python 3. Windows also
requires the MSVC linker/SDK initialized in a Developer PowerShell. Python uses
only its standard library; no Node, database, account or API key is needed for
the collector examples. The full checks also use Node for the GEV contracts.
The locked dependencies must already be cached for offline
Cargo commands; first-time dependency retrieval is a separate normal crates.io
operation:

```sh
cargo fetch --locked --manifest-path crates/us-groundwater/Cargo.toml
```

That first-time command needs ordinary official crates.io access; it does not request source data. It was not run during this cached/offline verification.

Run from the checkout root:

```sh
cargo build --offline --locked --manifest-path crates/us-groundwater/Cargo.toml --bins
cargo test --offline --locked --manifest-path crates/us-groundwater/Cargo.toml
cargo fmt --manifest-path crates/us-groundwater/Cargo.toml -- --check
python -B -m unittest discover -s scripts -p 'test_capture*.py'
node --test integrations/gev/historicalPack.test.mjs integrations/gev/dataBase.test.mjs
python -B examples/offline.py
python -B examples/texas_sdr_offline.py
python -B scripts/refresh-wells.py examples/refresh-offline.json .temp-gev/refresh-example-v2 --plan
python -B scripts/check-workflow.py
```

The example produces six indexed USGS IDs, verifies identical rebuilds and
rejects a tampered candidate without replacing the last-good index. Output is
kept under ignored `.temp-gev/`. It does not access the network or a credential.
The synthetic SDR example retains four invented reports, including one nonspatial
record, and verifies deterministic hashes and replay. The aggregate workflow
checker verifies the producer release inventory and separate workflow files.
Real SDR records and diagnostics are excluded from the source release.

## Run and resume a fixture queue

After the debug build, use `crates/us-groundwater/target/debug/well-jobs` (append
`.exe` on Windows):

```sh
well-jobs run fixtures/us-groundwater/jobs-v2/queue-offline.json .temp-gev/example-queue
well-jobs resume fixtures/us-groundwater/jobs-v2/queue-offline.json .temp-gev/example-queue
well-jobs resume fixtures/us-groundwater/jobs-v2/queue-offline.json .temp-gev/example-queue
well-jobs status fixtures/us-groundwater/jobs-v2/queue-offline.json .temp-gev/example-queue
```

Replace `well-jobs` above with that executable path; it is not installed into
PATH automatically. This synthetic example accumulates 2, 4 and 5 records. Only
exhausted source pagination is completed. These are test records, not inventory.
Build and run from this checkout: the current binary embeds its build-time
repository path for the transport and fixtures, so it is not relocatable yet.

## Explicit live access and budgets

No live queue or national corpus is included. Do not turn a fixture queue into a
live national run by editing one flag. A reviewed adapter config must explicitly
set scope, source filters/projection, finite request/row/byte/disk/time budgets,
absolute expiry and polite pacing. Saved opaque cursors must remain unchanged.
The runner retains hashes, source IDs, quarantines, cumulative accounting and
last-good candidates, and stops on access/quota/validation failures. Budget
exhaustion means partial coverage. It never auto-renews a window or publishes.

On Windows, masked local entry is `powershell -NoProfile -File scripts/usgs-key.ps1
-Action Set`. DPAPI protects the key outside Git for that user/computer. The
helper's `Run` action expects the debug `well-jobs.exe`; `Status` checks presence
only and `Verify` makes one explicit one-row official request. No live action is
part of the offline quick start. On other platforms use a trusted secret manager
to supply process-local `USGS_API_KEY`; protected portable storage is not yet
implemented. Keys travel only in `X-Api-Key`, never URLs or logs.

## Architecture, migration and contribution

Rust owns validation, queue accounting, source-ID reconciliation and spatial
indexing. Python owns bounded standard-library HTTPS transport. Versioned artifacts
are the boundary with the independent JavaScript/Cesium GEV layer. The browser
needs no Rust runtime or Rainfolk service. See [artifact contracts](docs/ARTIFACT_CONTRACT.md),
[GEV integration status](docs/GEV_INTEGRATION.md), [migration gates](docs/MIGRATION.md),
[contribution preflight](CONTRIBUTING.md), [security](SECURITY.md), and
[component rights](docs/RIGHTS.md).

The local national collection is evidence of progress, not a bundled download
or complete physical-well census. Research prompts default to local research;
only explicit opt-in may hand off selected public evidence for review. No shared
ingestion endpoint, paid product or founder-operated service is required.
The Terlingua repository is historical research, not a runtime dependency or the
current Rainfolk implementation. Website work, hosting changes and any legacy
retirement require their own decisions; none follows from an offline test.

## Texas SDR source adapters

The Rust crate supports bounded API captures of up to 200 submitted driller
reports per page and the [official pipe-delimited bulk export](docs/TEXAS_SDR_BULK.md).
Both retain provenance, source IDs and quarantine evidence. The API date-omission
workaround, unknown location flags and field policy are documented in the
[source contract](docs/TEXAS_SDR.md). The transport defaults to a zero-request dry run.

SDR construction records are separate from USGWD; they are not silently added to
its map totals or counted as additional distinct wells. Datum uncertainty remains
explicit. Construction reports do not verify active wells, ownership or water safety.
