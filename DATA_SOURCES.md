# Source rights and fixture scope

Credit: **U.S. Geological Survey**. Included actual fixtures retain the official
source URLs, response fields and retrieval timestamps. USGS-authored information
is generally U.S. public domain; third-party material retains separate rights.
See https://www.usgs.gov/information-policies-and-instructions/copyrights-and-credits
and https://api.waterdata.usgs.gov/docs/ogcapi/ . Code licensing does not relicense
source data. No imagery, logos or bulk corpus is included.

`fixtures/us-groundwater/v1` contains six historical/sample metadata records and
six observations; it does not demonstrate live/current monitoring. Inventory-only
fixtures do not imply measurements. `usgs-inventory-pilot/capture.json` is a small
New York inventory response, despite the generic historical folder name.
`jobs-v1/la-capture.json` is a bounded Louisiana public inventory response.
`synthetic-invalid-wi.json` and `jobs-v2/synthetic-*.json` are deliberately altered
test inputs, not authoritative well records. Synthetic IDs/names may share real
fixture coordinates; never publish them as inventory. Per-folder SHA-256 lists
cover included JSON inputs, not remote authenticity.

The collected national corpus and historical Texas, USGWD and Louisiana HQA data
are not included. Missing/unknown coordinate quality remains unknown; provisional
measurements can change. No water availability, drilling or potability inference
is supported.

## Texas SDR offline candidate

`fixtures/texas-sdr/synthetic-v1.json` contains invented construction-report rows
only, with no downloaded SDR records. The bounded Rust adapter is synthetic-only;
it excludes owner/contact/address fields and does not activate the USGS index.
See [the SDR source contract](docs/TEXAS_SDR.md) for official metadata/policy
links, normalization rules and the unresolved confidential-report exclusion
blocker under Texas Occupations Code 1901.251(c). TWDB's general copying policy
has not been treated as blanket permission for SDR redistribution.
