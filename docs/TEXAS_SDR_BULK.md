# Local SDR bulk import

This adapter reads an already saved official SDRDownload.zip. It makes no network
requests, joins no auxiliary tables and activates no spatial index or corpus.
Python standard-library ZIP/CSV transport feeds bounded JSONL to the existing
`well-jobs import-sdr-bulk` command. Rust shares the SDR row validator, ID/version
reconciliation, immutable partition writer and atomic final-manifest writer.
No dependency or orchestration service is added.

Build existing binaries, then inspect the local plan:

```sh
python -B scripts/capture-texas-sdr-bulk.py --archive <saved.zip> --source-metadata <download.json>
```

The receipt must identify the exact official bulk URL, compressed byte count,
SHA-256 and acquisition time. The reader checks archive bytes/hash, safe unique
ZIP member paths, unencrypted ordinary entries and the exact 73-column WellData
header inspected in the official export. It does not extract data tables.

Process to a fresh ignored directory with the same command plus
`--output .temp-gev/<fresh-name> --run`. Existing directories are refused.
Limits: 512 MiB compressed archive and selected member, 2 GiB declared archive
expansion, one million rows, 1 MiB input lines, 200 rows per bounded batch,
16 MiB per artifact, 4 GiB total output and a 30-minute child deadline. The ID/hash
index is capped by that row allowance; it is not an unbounded in-memory dataset.
An interrupted import leaves unreferenced staged artifacts, with no final
manifest. Automatic bulk resume is deliberately deferred; the raw archive stays
the immutable replay source.

## Source mapping

- Read only WellData: pipe delimiter, no text qualifier, strict Windows-1252.
  Literal quotes remain literal; extra pipes and undecodable rows quarantine.
  Unselected owner/contact/address fields never enter the Rust stream or output.
- Preserve tracking numbers as strings and IDs as `TWDB-SDR-<native-id>`.
  No coordinate-only merging with USGS, SDR or GWDB.
- Preserve DateSubmitted, DrillingStartDate and DrillingEndDate as source calendar
  dates with day precision and no invented timezone. `completedAt` remains null;
  the source field is drilling-end date, not a verified completion timestamp.
- Preserve raw coordinate strings, HorizontalDatumType, textual KnownLocationError
  and LocationVerifiedByDriller. The bulk text flag is not an API integer code.
  Flags never silently discard records. Only source-declared WGS84 pairs passing
  the Texas range guard are map-eligible, still unverified. Unknown/blank datums,
  NAD27 and NAD83 remain retained nonspatial records until conversion is reviewed.
- Depth remains null with `not-derived-from-core-table`; no zero or API depth is
  invented. TypeOfWork, original-report reference and reported well count remain
  raw context, never a verified physical-well count or active-well status.

Each row retains archive/member/physical-row/hash provenance. Partitions contain
source-row evidence, including duplicate versions. The reconciled ID-index shards
are authoritative: identical selected-field versions count once; conflicting or
invalid same-ID versions quarantine all occurrences, with no arrival-order winner.
Counts obey input = accepted IDs + identical duplicates + quarantined rows, and
accepted IDs = map-eligible + nonspatial. Do not concatenate row partitions and
claim that count as unique accepted reports.

`bulk-manifest.json` binds source, selected-member hash, all partition/index hashes
and counts. `snapshotFullyRead=true` means WellData EOF was verified; `complete`
always stays false. No claim covers all private wells, all eighteen tables,
ownership, water safety or accuracy. Source data rights remain separate from code.

Synthetic regression suite:

```sh
python -B -m unittest discover -s scripts -p test_capture_texas_sdr_bulk.py
```

It covers decoding/quotes, dates, datum uncertainty, private-field projection,
cross-partition duplicate/conflicting IDs, malformed rows, byte/row limits,
truncated streams, duplicate JSON members, ZIP safety and deterministic hashes.
Real archives and processing artifacts belong only under ignored local storage.
