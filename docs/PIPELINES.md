# Data pipelines and schedules

Review date: 2026-11-13. This page lists every Rainfolk data source, what each
stage of its pipeline does today, and how often it should run.

## Stages

Every source follows the same five stages:

1. **Check:** has the source changed since our pinned copy?
2. **Fetch:** download exactly the pinned files and verify their fingerprints.
3. **Import:** normalize the records (Rust `well-jobs`).
4. **Map:** build per-state map files, then one national root (Rust `well_index`).
5. **Publish:** upload a new immutable folder to data.rainfolk.org. This stage
   always needs the owner's approval. It never runs on a schedule.

## Status by source

| Source | Check | Fetch | Import | Map | Publish |
| --- | --- | --- | --- | --- | --- |
| USGWD (HydroShare well database) | `acquire-usgwd.py check` | `acquire-usgwd.py fetch` | `well-jobs import-usgwd-segmented` | `well_index --usgwd`, `usgwd-national.py` | manual, approved |
| Texas SDR bulk zip | none | manual download | `capture-texas-sdr-bulk.py` | none | none |
| Texas SDR live API | none | `capture-texas-sdr.py` (pilot query fails with HTTP 400) | Rust queue | none | none |
| USGS monitoring locations | none | `capture-usgs-inventory.py` (API key optional) | Rust queue | `well_index` | none |

USGWD is the only source that runs all the way through. The other three stop
before the map stage. They should get a check stage and a schedule only after
their fetch step works.

## USGWD: the full path

```sh
# 1. Pin a release from retained HydroShare listing pages (done once per release)
python -B scripts/acquire-usgwd.py pin page1.json page2.json --out .temp-gev/usgwd/release-<sha8>.json --expect <releaseSha256>
# 2. Check the live source against the pin (two small requests)
python -B scripts/acquire-usgwd.py check .temp-gev/usgwd/release-<sha8>.json --status .temp-gev/usgwd/status/source-check.json
# 3. Fetch one state by exact pinned path, verified against the publisher checksum
python -B scripts/acquire-usgwd.py fetch .temp-gev/usgwd/release-<sha8>.json Nebraska --out .temp-gev/usgwd/inbox/nebraska
python -B scripts/acquire-usgwd.py fetch .temp-gev/usgwd/release-<sha8>.json "South Carolina" --out .temp-gev/usgwd/inbox/south-carolina --geospatial-fallback
# 4. Import and map each state
well-jobs import-usgwd-segmented <csv> <source.json> <state-import>     # repeat until the manifest exists
well_index --usgwd <state-import>/usgwd-segmented-manifest.json <state-map> 900
# 5. Combine with the published states into a national root (local only)
python -B scripts/usgwd-national.py https://data.rainfolk.org/usgwd/<root>/index.json .temp-gev/usgwd/national --add <state-map>...
```

### Why the release is pinned

The release fingerprint covers every file listed on HydroShare. It is part of
every record ID, and the merge step refuses to combine states from different
releases. On 2026-11-12, someone uploaded four 22-byte files with Nebraska-like
names to the resource. That changed the live fingerprint even though no well data
changed. Our earlier manual downloads also picked a wrong Nebraska entry.

The pipeline now pins the release from the retained listing and downloads only
pinned paths. The check sorts live differences into these groups:

- **Changed, removed or new data files:** exit code 3, `review-needed`. The
  source really changed. Rebuilding then means a new release with new IDs for
  every state.
- **Stray entries:** tiny or oddly named uploads such as the Nebraska junk.
  They are reported but don't need a rebuild.
- **Check failed:** network or format error, exit code 2. Nothing is retried
  automatically.

### Source-specific handling

- **Nebraska:** HydroShare can return a "preparing archive" JSON reply instead
  of the file. The fetch step polls that one task every 10 seconds, for at most
  15 minutes. It then takes the CSV out of the prepared zip and verifies it
  against the publisher checksum.
- **South Carolina:** the published CSV does not match its own published
  checksum (we got `1fd21825…`, the listing says `6f6d304b…`), so it is rejected.
  `--geospatial-fallback` downloads the geospatial zip instead. That zip does
  match its checksum, and `usgwd-dbf.py` converts its 23,278 attribute records.
  The CSV's extra 171 records are excluded because they can't be verified.
- **Downloads:** allowed only from www.hydroshare.org and HydroShare's own
  storage bucket for this resource.

## Schedules

| Job | Cadence | Cost | Status |
| --- | --- | --- | --- |
| USGWD source check | weekly, Monday 07:00 | 2 small requests | `scripts/schedule-usgwd-check.ps1 Register` |
| USGWD rebuild | only when the check says `review-needed` | about 4 GB download plus hours of processing | manual |
| Publish a new root | only after review | upload of new objects | manual, needs owner approval |
| Texas SDR bulk check | monthly (TWDB updates the zip) | 1 HEAD request | to build: compare `Last-Modified`, which the receipt already records |
| USGS monitoring locations | monthly | bounded pages | to build after the live config is settled |

USGWD is a 2024 research snapshot, not a live feed. HydroShare marks it
published and immutable, so weekly checks are plenty. The check exists to catch
real corrections and stray uploads before they surprise a rebuild.

The scheduled task writes `.temp-gev/usgwd/status/source-check.json` and adds a
line to `history.log`. Run `schedule-usgwd-check.ps1 Status` to see the latest
result.
