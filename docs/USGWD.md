# USGWD historical baseline

USGWD is a historical research compilation, not a current well census. Source:
[HydroShare resource 8b02895f02c14dd1a749bcc5584a5c55](https://www.hydroshare.org/resource/8b02895f02c14dd1a749bcc5584a5c55/).
Retain the original creators' attribution/citation and [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
Code licensing does not relicense the source data. Real CSVs, manifests, acquisition
receipts and generated records remain in ignored local storage. The committed
`fixtures/usgwd/synthetic.csv` contains only invented records.

After the existing offline build:

```sh
crates/us-groundwater/target/debug/well-jobs import-usgwd saved.csv source.json .temp-gev/usgwd-run
```

The default (also available as optional `full`) processes the file in one bounded
invocation, retaining reconciliation state across 200-row batches. Its maximum
1,250 new partitions derives from the unchanged 250,000-row file cap. An explicit
numeric allowance of 1..100 permits deliberate interruption; e.g. append `1` for
one new partition. Use the same output directory to resume; the default full mode
verifies and replays its saved prefix once, then continues in the same process.
Summary fields report prefix replay rows and seconds. There is no network code
in this command. Acquisition is separate and must obey its authorized budget.

The saved 200,000-row/177.20 MB synthetic fixture completed in one full invocation
in 176.93 seconds, with 100.99 MB measured peak working set and zero prefix replay.
It emitted 1,000 row partitions and 1,000 ID-index shards; all 200,000 record hashes
and artifact references verified within 185.46 seconds total. The manifest was
367,269 bytes and checkpoint 183,272 bytes. This is a local debug-build measurement,
not proof of full-state coverage, a memory bound for every input, or index readiness.
The former forced-pause measurement preserved only 120,000 rows at its 300-second
total watchdog; its prior checkpoint and 600 partition hashes remain unchanged.

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

Local file limits are 192 MiB and 250,000 rows, with 300 seconds per invocation,
including source rehash, prefix replay and finalization. The 20,000-row synthetic
benchmark took 16.21 seconds for a full pass and 22.12 seconds over two resumed
passes, with at most 23.54 MB peak memory. The longer deadline provides bounded
headroom; it is not proof of full-state runtime or memory. The row cap is retained
until larger unique-ID measurements justify increasing it. Files exceeding either
bound stop even when the other allowance remains.

Hashing and parsing stream from the local file, with 256 KiB CSV rows, 4 KiB fields,
200-row partitions and 8 MiB artifacts. Logical output is capped at 2 GiB, reserving
1 MiB each for the checkpoint and final manifest. At most 1,250 row partitions and
1,250 index shards follow from the row cap; a regression checks their maximum
reference envelopes fit these metadata budgets. Checkpoint and manifest envelopes,
including generation/hash overhead, are checked before the shared atomic save.
Deadline checks guard each finalized ID, index write and metadata save. These are
cooperative local-I/O deadlines; a single atomic disk operation is not preempted.
Earlier checkpoints survive budget refusal. Limits do not reset the file's total
rows/output when resuming. Larger states still require a reviewed, coordinated
change to input/row/output/metadata/runtime bounds and measured unique-ID memory.
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
The documented integer flag domains are 0..2 for county/state/US and 0..3 for
duplicates. Surrounding whitespace is permitted; decimal spellings such as 1.0,
missing values and unsupported codes suppress mapping while retaining the record
and exact raw strings. Location Verified must be Yes, No or Unknown (with optional
surrounding whitespace); unsupported values likewise suppress mapping. A bad UTF-8
byte outside a recoverable Well ID still quarantines all versions of that ID.
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
Maintain immutable alias evidence tuples containing the source-system namespace,
exact raw state ID (including missingness), release-scoped USGWD ID, release ID,
and evidence references such as input hash, source row and documented crosswalk.
Permit multiple candidate aliases and explicit unresolved/collision states; a
matching state-ID string alone never establishes a unique well or permits overwrite.
Maintain checkedAt, actual source publication/modification times and dated source
observations separately. Conditional requests/validators or complete file hashes
detect change; a highest sequential ID cannot detect historical corrections.
Checkpoint each diff against a pinned prior snapshot, preserve old versions and
record added/changed/missing IDs. Missing records never imply plugged wells.
Sources without a reliable changefeed need periodic complete reconciliation.
No scheduler or state refresh implementation is included in this milestone.

Rhode Island's baseline is an incomplete digitized sample collected September 6,
2022. A successful read of that CSV does not establish all current Rhode Island
wells, and validation of source NAD83 pairs does not make the EPSG:4326 consumer
ready. Original coordinates and transformation/accuracy provenance must survive
any later reviewed conversion.

Validation: Rust parser/normalization tests and
`python -B -m unittest discover -s scripts -p test_capture_usgwd.py` cover synthetic
resume, duplicate/conflicting IDs, state-ID collisions, CRS, unknown dates,
nonspatial retention, source/partition tampering and deterministic replay.
