# Texas SDR adapter and bounded public pilot

Base: public main 98ef0f9e72e4d411ac33f7328f31fcb5f3b2831d (PRs 6, 8, 10),
verified by clone/ref check on 2026-10-26. Branch: local/texas-sdr-offline.
AGENTS.md and engineering/architecture/artifact/rights/security/migration references
were read; the public tree has no nested AGENTS.md or .agents/skills directory.
Rust owns validation/hashes, Python bounded transport. No new scheduler, service,
registry package, credential or per-record AI call. Direct Serde reuses locked
1.0.229. The USGS corpus, historical GEV producer, website and index are unchanged.

## Source review and decision

Reviewed on 2026-10-26:

- [TWRSRS manual](https://www.twdb.texas.gov/groundwater/data/doc/TWRSRS_UserManual.pdf),
  November 2015: page 4 identifies public SDR reports/downloads; page 24 describes
  confidential reports as viewable only by users associated with the driller.
  This documents a controlled public workflow, not a verified export filter.
- [Current well-data page](https://www.twdb.texas.gov/groundwater/data/data-well-reports.asp)
  offers SDR downloads/viewer, nightly updates and a warning that State staff
  have not verified locations. No bulk file was downloaded.
- [TWDB policy](https://www.twdb.texas.gov/policies/site/index.asp) permits copying
  and distributing agency materials and requests acknowledgment. Attribution:
  Texas Water Development Board; TDLR Submitted Drillers Reports.
- [TDLR policy](https://www.tdlr.texas.gov/disclaimer.htm) gives accuracy/currency
  disclaimers and recognizes disclosure exceptions. No dataset CC0 grant is claimed.
- [Layer schema](https://services.twdb.texas.gov/arcgis/rest/services/Public/WellReports/FeatureServer/0?f=pjson)
  declares service EPSG:4326, UTC dates, nullable fields, advertised maximum 2,000
  rows. LocationError is a small integer/domain null, with no known numeric meanings.
  Attribute coordinate datum is not independently verified.

Evidence supports the specifically authorized minimum-field local public sample,
superseding the earlier blanket hold. Exact export confidentiality filtering and
later removal handling remain recorded uncertainty. They do not justify discarding
useful public records. No restricted workflow, account, full report, owner/contact
value, agency email or bulk file was accessed. Publication/expansion need review.

## Acquisition and observed result

Dry run: python -B scripts/capture-texas-sdr.py (zero network requests).
The authorized attempt used:

    python -B scripts/capture-texas-sdr.py --public-sample --output .temp-gev/texas-sdr-public-pilot-20261026

Metadata first, then one-second pacing and one fixed page: where=1=1, seven
explicit outFields, returnGeometry=false, tracking-number ordering, limit 200,
offset 0. No wildcard, pagination, retry, credential or index activation.
Caps: two requests, 200 rows, 512 KiB per response, 1 MiB total, 15-second socket
timeout and 60-second elapsed checks around reads. Schema change, redirects,
denial, 429, API errors and limits stop. No automatic budget renewal.

Observed: schema succeeded; query response was rejected with
"source API error; no retry". Two requests attempted, zero retries, zero valid
live records retained/accepted/quarantined/nonmapped. No public capture/body was
saved. The initial value-free handler discarded the service error code; specific
access denial versus query/server failure is not established. This does not prove
public data is restricted. The handler now retains only a numeric error code on
a future authorized attempt, never source messages/values. No repeat was made.

Evidence: .temp-gev/texas-sdr-public-attempt-20261026/attempt.json.
Status: pending source-error diagnosis. A successful run would save schema,
minimum-field response, sanitized capture, timestamps, exact request URL, hashes,
budgets, attribution and terms/uncertainty links under an ignored local directory.
Unrequested attributes refuse retention; an automatic numeric ObjectId may be
removed from the sanitized capture with explicit accounting.

## Capture and record contract

CLI: gev-us-groundwater --texas-sdr-v1, JSON stdin/stdout. Library:
texas_sdr::normalize. Cap 1 MiB, one response, 200 rows. Adapter accesses no
network, file, credential or environment.

- Synthetic kind rainfolk-texas-sdr-capture, synthetic=true.
- Real public kind rainfolk-texas-sdr-public-capture, synthetic=false, exact bounded
  queryUrl and sourceResponseSha256/sourceSchemaSha256. Operator assertions are
  not remote authenticity attestations.
- Both require schemaVersion 1, fixed official layer sourceUrl, zoned retrievedAt,
  response with spatialReference.wkid=4326, explicit exceededTransferLimit Boolean
  and features containing only approved attributes. Service CRS does not verify
  attribute datum; no guessed transformation occurs.
- Duplicate JSON members anywhere refuse capture before Value maps erase evidence,
  including identical duplicates/escaped equivalent names. Unknown fields, bad
  envelopes, wrong source/CRS and limits also refuse capture. Errors omit source
  keys/values. Ordinary malformed row IDs/types have quarantine reasons.

| Field | Behavior |
| --- | --- |
| WellReportTrackingNumber | 1..15 digit string; preserve zeros; ID TWDB-SDR-<native-id>. Report identity is not verified physical-well identity. |
| ProposedUse | Null/blank or bounded source wording; outer whitespace trimmed. No current-use or ownership inference. |
| LatitudeDD / LongitudeDD | Retain original values in sourceCoordinates. Complete finite pair inside broad Texas guardrail 25..37 / -107..-93 is map-eligible with unverified coordinates/unknown accuracy. Missing/out-of-range pair retains a nonspatial record and quality reason. No swap, fabricated point or geocoding. Malformed nonnumeric types quarantine. |
| DateOfWellCompletion | Null/blank or integer Unix milliseconds, years 1..9999, UTC source timestamp; not a water observation. |
| BoreholeDepthFt | Null/blank or finite nonnegative source feet; preserve zero and canonicalize signed zero before comparison/hash. No conversion/yield inference. |
| LocationError | Preserve null or every signed-16-bit integer without meanings. No zero/one Boolean assumption. Codes do not discard records or determine eligibility. Non-null codes add an uninterpreted-code warning; every coordinate remains unverified. |

Records retain official SDR viewer URL and tracking number for source lookup.
The seven-field pilot scope is not a product-wide ownership-metadata ban.
Construction reports do not verify active wells, present ownership or water safety.
Versioned rainfolk-texas-sdr-candidate output has explicit synthetic/public status
and local-review activation. It fails the USGS bundle/index contract. Map eligibility
describes review data shape/quality, not an activated/deployed map.

Records sort by qualified ID. Identical normalized same-ID rows coalesce;
conflicts quarantine every version, including valid/invalid mixed rows. Canonical
byte comparison protects retained raw-coordinate differences such as signed zero.
No arrival-order winner or coordinate-only dedup across SDR, USGS or future GWDB.
Quarantine retains row index, valid ID/null, reason and capture hash; original
bounded capture remains evidence. Accounting reconciles input rows to accepted
unique records + identical duplicates + quarantines, and accepted records to
map-eligible + nonmapped records.

Hashes reuse compact sorted-key JSON/SHA-256. Content hash excludes its own hash
and acquisition provenance; capture hash covers original bytes; manifest/payload
hashes cover their full artifacts. Retrieval/source-byte changes alter provenance
hashes even when normalized content stays unchanged.

## Public-field visibility proposal

| Class | Treatment and implementation impact |
| --- | --- |
| Useful well/construction/location quality | Include with source warnings. Pilot implements seven fields and nonspatial retention. County/work-type/plugging links can follow schema review. |
| Public organization owner, public-facility operator, business/driller/professional license identity | Favor inclusion with attribution when source evidence establishes entity/professional role. Add typed fields later; do not guess organization versus private person from a name. Not fetched here. |
| Well-site address | Useful if verified as well/facility location; distinguish owner home/mailing/contact address. Review relevance/precision; use official source access where copying private residential details is unsuitable. Not fetched here. |
| Sensitive private-person identifiers/home/mailing/contact details | Do not copy sensitive personal details or create person-search/doxxing exports. Use appropriate official source access instead. No owner profiling. |
| Restricted/confidential reports | Do not bypass controls; public source links do not authorize restricted access. |

No owner database, person-classification service or compliance framework is added.
Useful organization/professional/site context deserves field-specific evidence and
review; no extra personal-data request is needed to establish this pilot.

## Offline verification and gates

Use existing edition-2024 Cargo/cached packages and MSVC environment on Windows:

    cargo build --offline --locked --manifest-path crates/us-groundwater/Cargo.toml --bins
    cargo test --offline --locked --manifest-path crates/us-groundwater/Cargo.toml
    cargo fmt --manifest-path crates/us-groundwater/Cargo.toml -- --check
    python -B -m unittest discover -s scripts -p 'test_capture*.py'
    python -B examples/offline.py
    python -B examples/texas_sdr_offline.py

Unchanged synthetic fixture now demonstrates four retained records, three
map-eligible, one nonspatial, zero quarantines. Separate malformed/duplicate-ID
tests prove quarantine. Public mode/transport use synthetic or fake responses
in tests; real records are never relabeled synthetic or committed.

Verified locally: 83 Rust tests (14 core, 46 queue/segment, 6 index, 17 SDR),
33 Python tests (24 USGS, 9 SDR), locked offline build, formatting and both
examples passed. Windows index atomic-replacement checks used scoped execution
approval; no production file-writing/security behavior was changed. The USGS
example retains six IDs, identical replay, tamper rejection and last-good output;
index SHA-256 df1666c4381bd32d1bf5baf07ad508eb6eb3851a67ec13e1712fb9160061b7c7.
Synthetic SDR example independently verifies hashes/replay; payload SHA-256
ed6789923e13c6a8e3a7860567835c24d5d9e7661687e3248e9ccd8554d4b872.

The workflow checker pins the previous release inventory and rejects this
development candidate. Manifest/checker remain unchanged; release-inventory
reconciliation and fresh review of the new source commit remain gates. No push,
PR, publication, deployment, bulk collection or agency contact was performed.
