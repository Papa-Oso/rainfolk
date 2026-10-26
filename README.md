# Rainfolk

### A well-th of information

Rainfolk aims to make public water information easier to find, explore, and trace back to its sources. The planned free public tools include a water-focused interactive map, a source catalog, and research prompts that guide people to original, reliable sources.

Wells are the first collection. The project's geographic focus is nationwide, not limited to Terlingua.

## Current status

This repository publishes Rainfolk's reusable Rust collector/indexer, bounded Python transport and small offline fixtures. The separate God's Eye View consumer is not bundled here.

`rainfolk.org` is the intended home for the project. The public starter site has not launched.

## Current local pipeline

The local implementation uses Rust with deterministic transport scripts. Its current pipeline is:

**collect -> validate -> spatial index -> God's Eye View layer**

The pipeline is still being refined. Collection and processing do not require an AI call for every record. AI may be used during development or research.

Rainfolk is intended to be useful without depending on a founder-operated service. Exportable data, source attribution, and provenance are part of that goal. Data views should make gaps and source, retrieval, and measurement times explicit.

## Coverage and limits

The latest verified USGS inventory snapshot, dated **October 25, 2026**, contains:

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

The USGS API key used by the local collector is protected locally. Keys and other secrets must never be committed to this repository or placed in browser code.

Newly authored Rainfolk code and documentation are MIT licensed, copyright 2026 Joshua Koelker; see LICENSE. Third-party code and source data retain their separate terms and notices. This license does not cover datasets or unrelated third-party material. Third-party terms and notices are preserved.

## Roadmap

- Continue publishing reusable source components and document rights by component.
- Make source attribution, data provenance, coverage gaps, and relevant timestamps clear.
- Develop the nationwide well-focused source catalog and interactive map.
- Prepare the intended public site at `rainfolk.org`.

## Links

- [USGS monitoring-location API documentation](https://api.waterdata.usgs.gov/docs/ogcapi/) - current source documentation.
- [God's Eye View](https://github.com/bilawalsidhu/gods-eye-view) - independent upstream project; no official partnership is implied.
- [Terlingua Water Atlas](https://github.com/Papa-Oso/terlingua-water-atlas) - historical research and catalog snapshot, not Rainfolk's current codebase or a rebranded version of this project.

## Run the offline example

Tested toolchain: rustc 1.99.1 and Cargo 1.99.1 on Windows; no older MSRV or other platform is claimed tested. Prerequisites: a Rust toolchain supporting edition 2024, Cargo and Python 3. Windows also
requires the MSVC linker/SDK initialized in a Developer PowerShell. Python uses
only its standard library; no Node, database, account or API key is needed for
the offline example. The locked dependencies must already be cached for offline
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
python -B -m unittest discover -s scripts -p 'test_capture_usgs_inventory.py'
python -B examples/offline.py
python -B scripts/check-release.py
```

The example produces six indexed USGS IDs, verifies identical rebuilds and
rejects a tampered candidate without replacing the last-good index. Output is
kept under ignored `.temp-gev/`. It does not access the network or a credential.

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
The legacy Terlingua project is intended for retirement once migration gates
pass; it is not an operational dependency here. Do not delete it on the basis of
this candidate alone. The public site and its deployment are handled separately;
`rainfolk.org` remains unpublished in this source release's status.

## Texas SDR offline adapter candidate

The same Rust crate includes `--texas-sdr-v1` for at most 200 submitted driller
construction-report rows, with explicit synthetic or public capture types. After building, run
`python -B examples/texas_sdr_offline.py` for normalization, quarantine, hash and
replay evidence. The separate bounded Python transport defaults to a zero-request
dry run. The authorized public pilot stopped on a source API error with no live
records retained. There is no spatial-index activation or publication.
See the [source contract, public-field proposal and pilot evidence](docs/TEXAS_SDR.md); construction
reports do not verify active wells, ownership or water safety.
