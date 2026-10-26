# Texas Submitted Drillers Reports: offline adapter v1

Local candidate based on public producer main
`98ef0f9e72e4d411ac33f7328f31fcb5f3b2831d` (PRs #6, #8, #10), verified by a
fresh Git clone on 2026-10-26. Branch: `local/texas-sdr-offline`. The public tree
contains AGENTS.md and the engineering/rights/contracts documented there; it has
no `.agents/skills` directory or nested AGENTS.md. This task owns the SDR module,
its tests/fixtures/example, this document, the library/CLI wiring and the
README/DATA_SOURCES pointers only.

This is a bounded, synthetic-only source adapter in the existing Rust producer
crate. It reuses the existing SHA-256, compact encoding, artifact descriptor and
timestamp validation helpers. It adds no queue, transport, service, dependency,
credential requirement or AI call. Existing USGS collection, indexing and corpus
paths are unchanged. The 1,042,777-record national inventory is not opened or
rewritten. Construction reports describe submitted drilling/construction events,
not verified active wells, present ownership, yield or water safety.

## Source and rights review (2026-10-26)

Reviewed public metadata and policy pages only; zero feature queries, zero live
records, zero ZIP bytes. No account login or agency contact was attempted.

- [Official service metadata](https://services.twdb.texas.gov/arcgis/rest/services/Public/WellReports/FeatureServer?f=pjson)
  identifies TDLR's Submitted Driller's Report Database, reports from February
  2001 onward, hosted by TWDB. The service has empty copyright text, which is not
  an affirmative data license. Advertised editing capabilities are not permission
  to edit; this adapter has no HTTP client.
- [Layer schema](https://services.twdb.texas.gov/arcgis/rest/services/Public/WellReports/FeatureServer/0?f=pjson):
  EPSG:4326, UTC date field reference, native string report tracking number
  (length 15), nullable fields, pagination/order-by support, advertised
  maxRecordCount 2,000. ObjectId is a service row key, not our report identity.
  `LocationError` is a nullable small integer with no domain/definition supplied.
  Its meaning and accuracy interpretation remain unresolved.
- [TWDB policy](https://www.twdb.texas.gov/policies/site/index.asp) permits copying
  and distributing its materials and requests acknowledgment. This general
  statement does not settle whether every SDR report is TWDB material or how
  confidential-report exclusions are applied to this layer/bulk export.
- [TDLR policy](https://www.tdlr.texas.gov/disclaimer.htm) disclaims data
  accuracy/completeness/currency and recognizes exceptions to public disclosure;
  it does not resolve SDR-specific exclusion or redistribution handling.
- Confidentiality under Texas Occupations Code 1901.251(c) remains an exact
  rights/access blocker: no reviewed source establishes that this feature layer
  and bulk export exclude all reports covered by an owner's confidentiality
  request, or how later restrictions/removals should be reflected downstream.
  The [official statute endpoint](https://statutes.capitol.texas.gov/Docs/OC/htm/OC.1901.htm)
  returned a general statutes navigation page in this review, so no fresh
  statutory text verification is claimed. The unresolved question supplied in
  the task has not been treated as permission.
- [Official bulk route](https://www.twdb.texas.gov/groundwater/data/SDRDownload.zip)
  is recorded as a future alternative only; no HEAD/GET/download/extraction was
  performed. Bulk columns, file size, rights and confidential exclusions have
  not been verified.

Decision: live ingestion is **pending rights clarification**, not denied by the
agency or an observed API failure. A maximum of 200 is our future sample cap,
not evidence that such a sample is now cleared. No live queue is enabled. Do
not change `synthetic` to label real records as synthetic.

## Capture contract

CLI: `gev-us-groundwater --texas-sdr-v1`, JSON stdin/stdout; the library entry is
`texas_sdr::normalize(&[u8])`. Input cap: 1 MiB including envelope; at most 200
rows, one response. CLI reads at most cap + 1 bytes. No path/network/environment
access occurs in the adapter. Unknown keys refuse the whole capture, including
on rows that would otherwise be quarantined. Error messages never echo source
values or unknown field names.

Envelope, exactly the keys shown in
[`synthetic-v1.json`](../fixtures/texas-sdr/synthetic-v1.json):

- `schemaVersion: 1`, `kind: "rainfolk-texas-sdr-capture"`, `synthetic: true`.
- `sourceUrl`: the exact official layer URL above, without query/credentials.
- `retrievedAt`: zoned timestamp; operator-asserted synthetic acquisition time.
- `response`: `spatialReference: {wkid: 4326}`, explicit boolean
  `exceededTransferLimit`, `features: [{attributes: {...}}]`. No geometry fallback.

Only these seven attributes may appear; optional missing/null/blank values
normalize to null. Report ID and valid coordinates are necessary for admission.

| Source field | Normalization and validation |
| --- | --- |
| WellReportTrackingNumber | 1..15 ASCII digits as a string; preserve leading zeroes; ID `TWDB-SDR-<native-id>`. No numeric coercion or trimming. |
| ProposedUse | Null or bounded text (50 UTF-8 bytes, no control characters); trim outer whitespace; preserve source wording. No category or public/private inference. |
| LatitudeDD / LongitudeDD | Native finite JSON numbers; both required; broad Texas guardrail latitude 25..37, longitude -107..-93. This is not a Texas boundary polygon or location verification. No swapping, geocoding or fabricated zero. |
| DateOfWellCompletion | Null or integer ArcGIS Unix milliseconds in Gregorian years 1..9999. Preserve as `{unixMilliseconds, timeZone: "UTC", precision: "source-timestamp"}`; never treat completion as a water observation or invent a local/date-only interpretation. ISO/CSV strings require a separate reviewed mapping. |
| BoreholeDepthFt | Null or finite native number >= 0, preserving zero; output `{value, unit: "ft"}`. No metric conversion, inferred depth, yield or water level. |
| LocationError | Null or native signed 16-bit integer. Preserve null/zero; all nonzero values quarantine pending interpretation. Accuracy is always `unknown`, including zero. This is a conservative local guardrail, not a claim about a documented code domain. |

Owner/contact/address fields and all unreviewed source fields are excluded by
the allowlist. Quarantine stores only zero-based row index, valid source-qualified
ID (or null), stable reason code and capture hash; never raw attributes or values.
The synthetic capture stays available as the bounded evidence referenced by hash.
Malformed envelopes, unexpected fields, wrong CRS/source, API error envelopes,
bad JSON or limits refuse the entire input. Ordinary row defects quarantine the
row while valid independent IDs can proceed.

## Candidate, identity and hashes

Output kind is `rainfolk-texas-sdr-candidate`, containing a versioned manifest
and `rainfolk-texas-sdr-construction-records` payload. Activation is
`offline-review-only`; rights status is `confidential-report-exclusions-unresolved`.
It deliberately fails the existing USGS `validate_bundle` contract. There is no
SDR spatial-index/queue activation in this milestone; that requires reviewed
multi-source semantics and rights. It must not be relabeled as a USGS bundle.

Records are sorted by qualified ID. Identical normalized rows sharing one SDR
ID coalesce and increment `identicalDuplicateRows`. Conflicting valid versions
quarantine all rows for that ID; an invalid row with a valid ID also prevents
another row with that ID from being selected as a winner. Quarantine is sorted
by source row index. Accounting always reconciles input rows to accepted unique
records + identical duplicate rows + quarantined rows.

Coordinates never drive identity reconciliation. Equal coordinates with different
report IDs remain different records. `TWDB-SDR-0001`, `USGS-0001` and any future
GWDB namespace are separate source identities; linking them would require an
explicit evidence-backed crosswalk. Report counts are not physical-well counts.

Hashes use the existing `serde-json-compact-sorted-keys-v1` encoding:

- Manifest input hash covers original capture bytes; exact replay is byte-stable.
- Each record's `contentSha256` covers normalized fields before adding that hash
  and acquisition provenance. It stays stable across retrieval-time and source
  row-order changes; changed normalized source content changes it.
- Provenance retains official layer URL, asserted retrieval time, original
  capture hash and synthetic marker. Payload descriptor has bytes, SHA-256 and
  accepted record count; outer manifest hash covers the compact manifest.
- Overall artifact hashes change if capture bytes or retrieval provenance change.
  These are integrity/replay hashes, not remote authenticity or legal clearance.

## Offline commands and acceptance

From repository root, using an existing edition-2024 Rust/Cargo toolchain,
cached locked dependencies and Windows MSVC developer environment as needed:

```sh
cargo build --offline --locked --manifest-path crates/us-groundwater/Cargo.toml --bins
cargo test --offline --locked --manifest-path crates/us-groundwater/Cargo.toml
cargo fmt --manifest-path crates/us-groundwater/Cargo.toml -- --check
python -B -m unittest discover -s scripts -p test_capture_usgs_inventory.py
python -B examples/offline.py
python -B examples/texas_sdr_offline.py
```

The SDR example writes only under ignored `.temp-gev/`, runs identical replay,
checks two admitted synthetic IDs and two quarantines, and independently checks
the manifest/payload/content hashes. Tests also cover projection refusal,
invalid coordinates/IDs/dates/depths, nulls, unknown LocationError, limits,
same-location distinct IDs, duplicate conflicts and CLI behavior.

Release gate: `python -B scripts/check-workflow.py` passed on pristine base.
The inherited checker pins the old producer manifest and refuses new producer
files/changed hashes. It is expected to reject this development candidate; the
manifest and checker are deliberately not regenerated or weakened. The producer
owner must reconcile the combined release inventory before a release/CI claim.
Nothing in this local task authorizes push, PR, deployment or publication.

Verified locally on 2026-10-26 with existing Rust/Cargo 1.99.1, cached locked
packages, installed MSVC tools and Python 3.13:

- Locked offline binary build and formatting check: passed.
- Rust suite: 79 passed (14 core, 46 queue/segment, 6 index, 13 SDR); zero failed.
  The first sandbox run hit Windows Access denied in the six unchanged index
  tests during atomic replacement. Scoped execution approval resolved those
  tests without changing production file-writing behavior or security settings.
- Existing Python transport suite: 24 passed.
- Existing USGS offline example: six IDs; identical replay; tampering refused;
  last-good index retained. Index SHA-256
  `df1666c4381bd32d1bf5baf07ad508eb6eb3851a67ec13e1712fb9160061b7c7`.
- SDR offline example: two synthetic records, two quarantines, identical replay;
  independently verified input/record/payload/manifest hashes. Payload SHA-256
  `1274cc8fa0f54c14367c72747f993a8d3eb4a89e36b5e18e26c3255ded89d70d`.
- `git diff --check`: passed. Existing USGS fixtures, source provenance, release
  manifest, queue/index implementations, Cargo dependency manifest and lock are
  unchanged. No national corpus is included in the checkout or used for tests.
- Development-tree workflow checker: fails as expected with
  `Unexpected or missing tracked/workflow files`; producer release-inventory
  reconciliation is pending. No combined release/CI certification is claimed.

Status: offline adapter acceptance completed; live API sample **pending** the
rights/exclusion evidence described above. No remaining action has a denied or
uncertain remote-write outcome; no remote write was attempted. Publication and
agency contact remain outside this task's authorization. The historical GEV
producer and live website were not edited or used as dependencies.

## Next bounded step

Resolve authoritative SDR confidentiality exclusion/removal handling and the
LocationError definition using existing public documentation or an approved
agency inquiry (no contact is authorized here). Then review one sequential,
minimum-field read-only API sample with <= 200 records, <= 2 requests, <= 1 MiB
total response bytes, <= 60 seconds, no automatic retry/paging/renewal and no
credentials or paid services. If transport becomes justified, extend the
existing bounded Python transport/queue contracts, preserving exact request and
response provenance. Bulk download and multi-source index activation remain
separate later decisions.
