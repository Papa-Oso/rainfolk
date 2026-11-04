# Rainfolk

Rainfolk is a personal project Josh Koelker works on in his spare time to make
public water information easier to find, understand and use. It is an educational
effort, not a registered nonprofit.

Wells are the first practical focus. Rainfolk prepares public well records for
map layers while keeping their sources, dates, gaps and quality warnings visible.
Broader water education and mapping are future work.

## Current status

Status as of November 4, 2026:

| Work | What is available |
| --- | --- |
| Public well-data feed | An immutable 48-state historical USGWD release: 13,980,064 catalog records, 12,171,830 mapped and 1,808,234 unmapped. |
| Local 50-state candidate | Validated locally: 14,260,581 catalog records, 12,452,285 mapped and 1,808,296 unmapped. It is not published; preview review and a new publication decision remain. |
| This repository | Rust validation/indexing, bounded Python tools, sample and synthetic fixtures, and a source-only God's Eye View integration kit. Full source datasets are not bundled. |
| Water atlas | Separate private development, not a released application in this repository. |
| God's Eye View contribution | [PR #1508](https://github.com/bilawalsidhu/gods-eye-view/pull/1508) is open for review and is not merged. Maintainer CI authorization/review and disclosed verification gaps remain; no partnership or acceptance is implied. |

The local candidate adds checksum-verified Nebraska data and a separately
checksum-verified South Carolina geospatial source. South Carolina's CSV still
fails its advertised checksum; its 171 additional unverified records remain
excluded. The reviewed preparation code is on
[`feature/usgwd-missing-states`](https://github.com/Papa-Oso/rainfolk/tree/feature/usgwd-missing-states),
not yet merged into `main`. State presence does not mean complete state coverage.

## Use the public feed

The current [48-state index](https://data.rainfolk.org/usgwd/8fa0f2843ef51c51f07da74644f9f5a2cf56c45563dc12f48dfaaca5e6aa6fe2/index.json)
is a versioned JSON data entry point, not a standalone map page or flat CSV.
No well-data API key is required. A compatible reader uses the index's bounds,
counts and hash/byte-pinned child references, then loads only the detail needed
for its viewport. It should preserve attribution and validate those references
rather than fetching the entire national dataset at startup.

For the full God's Eye View application, see [PR #1508](https://github.com/bilawalsidhu/gods-eye-view/pull/1508)
and its setup/review notes. The Water Wells layer is proposed under Infrastructure;
it is not part of a merged upstream release. Close views load historical source
records; dense views can report limited detail, requiring a closer zoom. An empty
or quiet view does not establish an absence of wells. The PR includes screenshots
and a recording, along with completed checks and remaining verification gates.

The [integration kit in this repository](integrations/gev/README.md) reproduces
an earlier pinned consumer and supports offline contract work. It is not an
installer for the latest full GEV application; use the PR's current instructions
for that contribution.

This is a pinned public release. Clients do not automatically discover or select
new releases. A new release needs an explicit client configuration change and
restart/rebuild of the configured consumer, followed by review. The full GEV
contribution selects its release with `VITE_RAINFOLK_WELLS_RELEASE`; see the PR for
its exact configuration. This does not automatically switch existing clients.
The local 50-state
candidate is not a replacement URL to use in a public client yet.

These are historical source records, not verified distinct physical wells or a
current census of active wells. Construction depth is not groundwater level.
Approximate or unverified locations are not surveyed wellheads, and the records
do not establish water safety, groundwater availability or drilling suitability.
Unmapped records remain part of catalog accounting rather than disappearing from
the totals. See the [USGWD contract](docs/USGWD.md) and
[map bridge](docs/USGWD_MAP.md).

The separate October 25, 2026 USGS inventory has 1,042,777 monitoring-location IDs
and remains unchanged. Its pagination status is source-retrieval progress, not
complete well coverage. That inventory collected location metadata, not
water-level time series. It is not silently merged with USGWD or deduplicated by
proximity.

## Run an offline example

For contributors, the local pipeline is **collect -> validate -> spatial index ->
map consumer**. Processing is deterministic; there are no AI calls per well record.
The browser consumer needs no Rust runtime or source API key.

You need Rust/Cargo supporting edition 2024 and Python 3. The recorded tested
Rust/Cargo version is 1.99.1; no older minimum version is claimed. Windows builds
also need the MSVC linker and SDK. Python transport uses the standard library.
Node is needed for GEV checks, not these collector examples.

From the repository root, fetch the locked dependencies once if they are not
already cached. This contacts crates.io, not a well-data source:

```sh
cargo fetch --locked --manifest-path crates/us-groundwater/Cargo.toml
```

Then build and run the local examples:

```sh
cargo build --offline --locked --manifest-path crates/us-groundwater/Cargo.toml --bins
python -B examples/offline.py
python -B examples/texas_sdr_offline.py
python -B scripts/check-workflow.py
```

The first example verifies six sample USGS IDs, repeatable hashes, tamper rejection
and last-good retention. The SDR example verifies four invented construction
reports, including one nonspatial record. Examples use ignored `.temp-gev/`
outputs and make no live data requests. Binaries are not installed into `PATH`;
see [engineering setup and full checks](docs/ENGINEERING.md) for tool paths,
verification and contributor workflow. Build and run from this checkout: the
current runner embeds checkout paths for transport and fixtures and is not yet
relocatable. Real collection needs a separately scoped
source configuration and finite resource limits; examples are not live jobs.

## Sources, licensing and refresh

The historical compilation is the **U.S. Groundwater Well Database (USGWD)** by
Chung-Yi Lin, Alexandra Miller, Musab Waqar and Landon T. Marston, published on
[HydroShare](https://doi.org/10.4211/hs.8b02895f02c14dd1a749bcc5584a5c55) under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Retain the publisher's
citation, source credits, licence and supplied notices, and describe modifications.
The contribution's [agency/source credits](https://github.com/Papa-Oso/gods-eye-view/blob/10a211f415d0f9fc1c3d120f40b14b6ebd353a1b/docs/US_GROUNDWATER_SOURCES.md)
and [supplied state notices](https://github.com/Papa-Oso/gods-eye-view/blob/10a211f415d0f9fc1c3d120f40b14b6ebd353a1b/docs/US_GROUNDWATER_SOURCE_NOTICES.md)
are pinned to PR revision `10a211f4`; the publisher's original tables remain
authoritative.
Rainfolk keeps original source identities and quality evidence, flags invalid
measurements without guessing corrections, and records display-coordinate
transformations separately from original coordinates.

Rainfolk's newly authored software and documentation use the [MIT licence](LICENSE).
That software licence does not relicense datasets or third-party material.
[Source terms](DATA_SOURCES.md), [component notices](docs/RIGHTS.md) and the
publisher's supplied agency notices remain relevant. A compilation licence is
not a blanket guarantee about every underlying right or about anonymity.

The [local snapshot-refresh tools](docs/SNAPSHOT_REFRESH.md) detect changed input
files, resume processing and compare source IDs. They do not provide automatic
agency acquisition, scheduled live updates or public release discovery. A changed
landing-page date is not evidence of fresh well records. The baseline and future
official-source updates need separate provenance and conservative reconciliation.

Texas [SDR construction-report adapters](docs/TEXAS_SDR.md), including the
[official bulk format](docs/TEXAS_SDR_BULK.md), are separate from the USGWD feed.
They do not automatically add records to the map totals.

## Help or support Rainfolk

Useful contributions include authoritative public source leads, reproducible
quality reports, documentation and testing. Include the publisher, source URL,
area covered and date checked. Keep credentials and private or sensitive material
out of issues and commits. Read [CONTRIBUTING](CONTRIBUTING.md) and
[SECURITY](SECURITY.md) before submitting work.

If you would like to support this personal educational project, voluntary support
is available through [Support Rainfolk on Ko-fi](https://ko-fi.com/rainfolk1).

The next practical steps are to review the local 50-state layer, decide its
publication separately, and resolve review/verification gates on the open GEV
contribution before expanding ongoing source refresh work.

- [God's Eye View](https://github.com/bilawalsidhu/gods-eye-view): independent upstream application.
- [GEV integration status](docs/GEV_INTEGRATION.md): boundaries and review gates.
- [Terlingua Water Atlas](https://github.com/Papa-Oso/terlingua-water-atlas): historical research, not Rainfolk's runtime dependency or current application.
