# Proposed GEV historical data feed

Source preparation only. No bucket, domain, billing, upload or public data release
is activated by this document or the consumer patch. Account prerequisites are a
separate task. The original GEV installation and existing USGS feed stay separate.

## Consumer connection

The existing `createSpatialWellSource` accepts a local directory as before, or an
explicit HTTPS directory whose exact origin is in `allowedDataOrigins`. Example
values below are placeholders, not configured infrastructure:

```js
createSpatialWellSource({
  base: 'https://data.example.test/usgwd/ROOT_SHA256/',
  allowedDataOrigins: ['https://data.example.test'],
  label: 'USGWD historical catalog',
});
```

Configure this at GEV source construction, not from a query string or untrusted
manifest. Relative artifact references cannot change origin or escape that
directory. Credentials, signed URLs, queries, fragments and redirects are refused.
The browser requests `index.json`, broad count chunks, then bounded nearby child
indexes/detail packs. Existing hash verification, last-good behavior, cancellation,
56 MiB viewport and 2,000-record/16 MiB detail limits remain. It does not download
the whole dataset for each viewer. Same-origin `/well-index-v1/` still defaults to
the existing USGS generation; `/well-index-usgwd/` works if explicitly served there.

## Candidate and exact upload boundary

Only the stable 36-state generation is proposed. Its root index SHA-256 is
`f7e59fb0a049b3a1990cc0fd6862190b8cc6aeb370989fd41ac0300b9ee6d336`
(150,592 bytes). It represents 7,072,581 catalog records: 5,918,051 mapped and
1,154,530 unmapped. Coverage is incomplete; physical well identity is unknown.
The separate successful New Jersey pilot is not in this root.

The manifest-referenced closure was inventoried without re-export or detail-body
rereads: 59,468 files, 17,463,294,415 bytes (16.264 GiB). This includes 37 indexes,
1,355 count files, 36 audits and 58,040 detail packs. It excludes raw CSVs,
normalized rows, checkpoints, staging files, failed exports, logs and New Jersey.
At upload time, enumerate references from this exact root and its hash-pinned
children, verify every local referenced byte count and SHA-256, and refuse any
missing/mismatched/extra candidate. Do not recursively upload an output directory.

Use one immutable object prefix `usgwd/<root-sha256>/`, preserving every relative
path. Copy `integrations/gev/DATA_ATTRIBUTION.txt` to that prefix as `ATTRIBUTION.txt`
as one additional object; it includes the current publisher citation, CC BY 4.0
link and adaptation notice. Preserve existing per-record provenance. No mutable
`latest` alias or worker service is needed for this first release.
The prepared attribution is 1,375 bytes (SHA-256
`9667d08ed8b7fc1902d8a388a19d71431300f676bb20e086ab2c22883a1e4cec`),
making the proposed total 59,469 objects and 17,463,295,790 bytes.

The exact object list and attribution byte count must be frozen in a local upload
receipt before approval. Initial transfer is one closure plus attribution only;
verify remote object metadata/checksums and bounded browser samples, not a second
whole-corpus network download. Retain receipts and retry only verified missing or
failed objects under an explicit finite request/transfer budget. Never overwrite
different bytes at an existing version key. Upload dependencies before the root;
switch the GEV configuration only after verification and approved visual QA.

Proposed finite activation job: at most 60,000 PUTs, 60,000 HEADs, 64 sample GETs
and 64 OPTIONS requests; 18,000,000,000 total transferred bytes and two hours total
nonrenewable wall time. At most one retry per transient failed object within those
same reservations; stop on access denial or rate limits. A stopped job retains its
ledger and budget consumption; it cannot reset limits by restarting. This budget
is a proposal, not permission to execute. Verify local SHA-256 before each PUT,
retain it as object metadata and check returned upload checksum where supported;
metadata alone is not an independent proof of remote body equality. Bounded GET
samples and the consumer's hash verification provide body checks.

## Headers, access and rollback

Use R2 Standard and an approved production custom HTTPS domain; `r2.dev` is for
development and has rate limits. Serve JSON as `application/json`; do not use an
HTML fallback. Preserve JSON bytes (no normalization/minification). Set
`Cache-Control: public, max-age=31536000, immutable` on versioned objects, including
the versioned root. The current consumer deliberately revalidates fetches with
`cache: no-cache`; do not claim that this patch eliminates origin read operations.
JSON edge caching requires a suitable custom-domain cache rule, checked separately.

Bucket CORS must allow the exact approved GEV origin(s), methods GET and HEAD,
and the browser's `Cache-Control` request header if preflighted. Expose ETag for
diagnostics; do not treat multipart ETag as the artifact SHA-256. No credentials,
cookies, write methods or wildcard identity are needed in the browser. CORS is
not access control: enabling public access publishes all accessible bucket keys.
Use a dedicated data bucket/prefix without private files. The GEV host's CSP
`connect-src` must include the selected data origin while retaining existing app
requirements. Test GET and OPTIONS with an actual approved Origin, redirects
disabled, before activation. CORS changes may need a custom-domain cache purge.

Rollback is a configuration change back to the prior approved base (or disable
the historical source and retain the existing USGS layer). Keep prior version
objects intact. Revocation means disabling public access through every enabled
bucket domain; cached/downloaded public copies cannot be recalled. Do not delete
local originals or prior generations as part of rollout or rollback.

## Cost exposure and approval

Current R2 Standard prices are $0.015/GB-month, $4.50/million Class A writes and
$0.36/million Class B reads, with 10 GB-month, 1 million A and 10 million B monthly
free allowances shared with other account usage. Egress is free; billable units
round up. This single closure is roughly $0.27/month storage before free usage,
or roughly $0.12 if all 10 free GB-month remain. Each retained extra version adds
storage. About 59,469 one-object writes fit the A allowance only if it remains;
retries, metadata checks and public traffic add requests. These are estimates,
not an enforced $5 cap. Alerts are not proof of automatic spending cutoff.
If the A free allowance is already exhausted, initial writes can round to a full
$4.50 billing unit before storage and reads. Verify actual remaining allowances.

Before activation Josh must approve: the actual account/bucket/domain and GEV
origin; publishing this exact 36-state closure plus attribution; its finite upload
and verification limits; and the recurring storage/read-cost policy and response
to an alert. Confirm account usage/payment prerequisites separately. Then obtain
the separate GEV configuration/deployment approval and full-app visual acceptance.
No paid activation or release follows from draft PR approval alone.

Official references checked 2026-10-29:
[R2 pricing](https://developers.cloudflare.com/r2/pricing/),
[public buckets and caching](https://developers.cloudflare.com/r2/buckets/public-buckets/),
[CORS](https://developers.cloudflare.com/r2/buckets/cors/), and
[publisher citation/license](https://www.hydroshare.org/resource/8b02895f02c14dd1a749bcc5584a5c55/).
