# Completing the historical USGWD baseline

Nebraska and South Carolina use the same published USGWD release identity as the
other states. These are historical source records, not a census of active wells.
No proximity merge or state-ID upsert is performed.

## Nebraska download lifecycle

The public HydroShare content route can return HTTP 200 with JSON containing
`zip_status`, `task_id` and `download_path`, rather than file bytes. This is an
asynchronous archive preparation response, not an unavailable dataset. The
[official client](https://github.com/hydroshare/hsclient/blob/master/hsclient/hydroshare.py)
polls `/hsapi/taskstatus/<task_id>/` every ten seconds, then downloads the returned
payload URL when status is `true`. Keep the task identity; do not repeatedly start
new archive tasks. Verify the exact inner CSV against the publisher manifest.

## South Carolina verified format alternative

The public CSV and the CSV obtained through the documented ZIP lifecycle both
have MD5 `1fd21825c4596fb9bd27572abbfe0ea6`, whereas the published CSV manifest lists
`6f6d304b6d43c135b2a9a51e0eef36c0`. Those CSV bytes remain rejected. This is a
publisher-byte/checksum inconsistency, not an importer validation failure.

The separately published geospatial ZIP matches manifest MD5
`aa4afbfefff2ff31ba808256051d139f`. It contains 23,278 DBF attribute records. The
unverified CSV has 23,449 records; do not combine the extra 171 records or claim
these representations are identical. State presence is distinct from complete
state coverage.

`scripts/usgwd-dbf.py` converts the verified ZIP attributes into the existing
29-column CSV contract. It checks the publisher ZIP checksum, bounded archive
structure, exact field aliases/types, Latin-1 encoding and exact NAD83 PRJ.
Fixed-width padding is removed; identifiers, numeric spellings, negative values
and missingness are retained. Deleted or malformed DBF records fail conversion;
they are never silently skipped. No geometry, owner profiles or new observations
are invented. DBF character widths may be narrower than the rejected CSV;
original DBF bytes and the entire unchanged archive remain the source evidence.

The v2 source receipt records the actual ZIP URL, archive and member hashes,
conversion policy and count. `inputSha256` identifies the derived CSV, not the
ZIP. The map pack carries this derivation alongside the derived input hash.
Original source coordinates remain NAD83 EPSG:4269; the existing explicit map
bridge handles conversion and uncertainty. Existing v1 CSV receipt and pack
bytes retain their original behavior.

```sh
python scripts/usgwd-dbf.py source.zip template.json derived.csv source.json --publisher-md5 PUBLISHED_MD5
well-jobs import-usgwd-segmented derived.csv source.json local-output
well_index --usgwd local-output/usgwd-segmented-manifest.json local-map 900
```

The template supplies the existing release, attribution, schema, state and source
dates. The converter replaces input identity and URL and adds derivation evidence.
This is offline processing; it does not acquire files, activate a schedule or
publish data. Keep real archives, derived records and receipts outside Git.

HydroShare resource publication was March 2024; the October 2026 landing-page
change is labelled a metadata update. No automatic source-data update cadence is
established. Official agency registries remain separate future refresh sources.
