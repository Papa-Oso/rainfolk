# USGWD historical baseline

USGWD is a historical research compilation, not a current well census. Source:
[HydroShare resource 8b02895f02c14dd1a749bcc5584a5c55](https://www.hydroshare.org/resource/8b02895f02c14dd1a749bcc5584a5c55/).
Retain the original creators' attribution/citation and [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
Code licensing does not relicense the source data. Real CSVs, manifests, acquisition
receipts and generated records remain in ignored local storage. The committed
`fixtures/usgwd/synthetic.csv` contains only invented records.

After the existing offline build:

```sh
crates/us-groundwater/target/debug/well-jobs import-usgwd saved.csv source.json .temp-gev/usgwd-run 1
```

Use the same command to resume, permitting at most one new 200-row partition.
The optional allowance is 1..100 (default 10). There is no network code in this
command. Acquisition is separate and must obey its authorized budget.

The closed source receipt has schemaVersion=1, resourceId, releaseSha256,
inputSha256, inputBytes, state, sourceUrl, checkedAt, sourcePublishedAt,
sourceFileModifiedAt, sourcePublishedTimeZone, license, attribution and schemaSha256.
Source URL must be the resource's state CSV URL, license must be CC-BY-4.0,
checkedAt must have an explicit timezone, and publication time/timezone currently
remain null. Preserve the raw file modification timestamp separately. The release
fingerprint is SHA-256 of compact JSON encoding of sorted (relative resource path,
file size, manifest checksum) triples from both manifest pages. Retain both pages
and the schema workbook with their hashes. The receipt records operator-supplied
source evidence; it is not a signature or automatic license verification.

The initial local file is capped at 64 MiB and 250,000 rows, with 60 seconds per
invocation. Hashing and parsing stream from the local file, with 256 KiB CSV rows,
4 KiB fields, 200-row partitions, 8 MiB artifacts, 1 GiB output and a bounded
ID/hash map. Larger state files require an explicit reviewed limit change.
No dependencies were added. CSV quoting supports commas, escaped double quotes,
embedded newlines, UTF-8/BOM and LF/CRLF. Structurally ambiguous quoting stops the
stream, retaining earlier checkpoints; row-level UTF-8, width, quantity, ID and
year errors quarantine with value-free reasons.

The existing well-jobs immutable writer, atomic checkpoint writer, exclusive
writer lock and source-ID reconciliation are reused. Resume verifies the source
hash, replays the saved prefix locally, verifies its partition hashes and counts
the prefix once. It makes no source requests. Replaying a completed import leaves
its manifest/checkpoint unchanged. A partial import has no final manifest.
Do not delete checkpoint/lock artifacts to bypass a competing writer or mismatch.

Identity is `USGWD-<release-sha256>-<original-generated-Well-ID>`. Generated IDs
are only stable within that published release. State IDs remain raw, nullable and
nonunique; no state-ID upsert or location-only merge is performed. All 29 source
fields remain available, including FIPS/HUC12 strings and Location Verified,
Flag County, Flag State, Flag US and Flag Duplicate. Equal source IDs with equal
content coalesce; conflicting or invalid versions quarantine all occurrences.
ID-index shards are authoritative; row partitions retain duplicate evidence.

Longitude/latitude remain NAD83 EPSG:4269, as documented by the attribute table
and publisher projection evidence. Missing/invalid coordinates retain nonspatial
catalog records. Map-eligible counts mean valid NAD83 pairs without a reported
state/US inconsistency, for a CRS-aware consumer; source flags remain warnings.
They do not certify precise wellheads. `existingIndexEligible=false`: the existing
EPSG:4326 spatial index still needs a reviewed datum conversion and candidate
contract bridge. No index or corpus is activated here. Depth/screen/elevation
values retain feet and source references; capacity retains source gallons/minute (GPM).
Construction/reported dates retain year precision. Status and potable labels
remain source assertions, never current activity or verified water safety.
`snapshotFullyRead` describes only this file; `complete` always remains false.

## Refresh contract

Keep the baseline bytes, complete release manifest and identity map immutable.
The 100 core tabular/spatial state files were modified March 8, 2024; a landing
page changed in October 2026 does not update those records. Source acquisition
dates vary (Texas February 2022, Rhode Island September 2022).

Acquire official state updates into separate source namespaces. Use documented
stable state report IDs with explicit source-system mappings; nullable/nonunique
IDs and collisions require reconciliation evidence, not proximity merging.
Maintain checkedAt, actual source publication/modification times and dated source
observations separately. Conditional requests/validators or complete file hashes
detect change; a highest sequential ID cannot detect historical corrections.
Checkpoint each diff against a pinned prior snapshot, preserve old versions and
record added/changed/missing IDs. Missing records never imply plugged wells.
Sources without a reliable changefeed need periodic complete reconciliation.
No scheduler or state refresh implementation is included in this milestone.

Validation: Rust parser/normalization tests and
`python -B -m unittest discover -s scripts -p test_capture_usgwd.py` cover synthetic
resume, duplicate/conflicting IDs, state-ID collisions, CRS, unknown dates,
nonspatial retention, source/partition tampering and deterministic replay.
