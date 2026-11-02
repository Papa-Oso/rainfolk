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

## Historical USGWD compilation

The [USGWD compilation](https://doi.org/10.4211/hs.8b02895f02c14dd1a749bcc5584a5c55)
by Chung-Yi Lin, Alexandra Miller, Musab Waqar and Landon T. Marston is published
under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). This is separate
from Rainfolk's MIT software licence and the USGS material above. Preserve the
publisher citation, licence, modification context and supplied source notices.
The resource's Metadata folder supplies Table S1 agency credits and Table S2
collection dates, notices and representation estimates; the original tables
remain authoritative. See also the [authors' paper](https://par.nsf.gov/servlets/purl/10523847).

A published compilation licence and its original acquisition/publication context
are not a blanket legal guarantee or anonymity certification. Supplied agency
notices, privacy and other rights still require proportionate consideration;
public accessibility alone is not a licence. Do not add excluded owner/contact
fields or assume that coordinates and source IDs cannot link to other records.

The current public 48-state feed and separate local 50-state candidate are
listed in [README](README.md#current-status). Neither is a complete current well
census. The baseline retains historical source dates and original NAD83 values;
display conversions and uncertainty are explicit. A metadata timestamp does not
prove a dataset refresh, and no automatic public release discovery is claimed.

## Texas SDR offline candidate

`fixtures/texas-sdr/synthetic-v1.json` contains invented construction-report rows
only, with no downloaded SDR records. The Rust adapter distinguishes synthetic
and explicit public captures and does not activate the USGS index. The authorized
public pilot's metadata/count reads succeeded. Field bisection reproduced ArcGIS
400 when selecting DateOfWellCompletion alone with report ID; all six other
approved fields work together. One real public report now passes through Rust,
with completion date unknown and the query-failure reason/hash in provenance.
Actual source responses/candidates are retained only under ignored local paths,
not committed or published. This is not a general service outage or access denial.
The minimum-field pilot does not fetch
owner/contact/address values; this is not a blanket policy against useful public
organization/professional/site metadata. See [the SDR source contract](docs/TEXAS_SDR.md)
for primary evidence supporting the public workflow, field policy, normalization,
remaining export-filter uncertainty and observed source-error blocker. No blanket
redistribution license or publication approval is claimed.
