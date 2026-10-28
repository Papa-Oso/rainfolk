# Existing GEV consumer: historical-source extension

This source-only kit reuses the reviewed thin groundwater consumer and water-only
Rainfolk shell. It does not alter an installed GEV app or submit upstream. Preserve
the existing MIT notice in `LICENSES/GEV-MIT.txt`; newly authored Rainfolk files
retain Rainfolk's MIT license. Cesium and other dependencies retain their notices.
Source data retain separate terms, including USGWD CC BY 4.0 attribution.

`consumer-pins.json` pins the original GEV base and per-file consumer delta.
The two base patches preserve the existing consumer and shared-host fix.
The base consumer patch omits four fixture hunks: reuse the same four files
already committed under Rainfolk `fixtures/us-groundwater/v1`, without new data
acquisition. `usgwd-consumer.patch` adds source-aware admission, finite child-index
loading and source-neutral labels. `historicalPack.mjs` is the closed historical
validator and description adapter. No dependency version is changed.

From Rainfolk, with the existing GEV base objects and installed dependencies:

```sh
python -B integrations/gev/prepare-consumer.py EXISTING_GEV_CHECKOUT .temp-gev/consumer-proof
node integrations/gev/build-map.mjs .temp-gev/consumer-proof .temp-gev/consumer-proof/build EXISTING_DEPENDENCY_CHECKOUT
node --test integrations/gev/historicalPack.test.mjs
```

The preparation command reads local Git objects at the pinned base and writes
only a new ignored directory in this checkout. It checks/applies all patches,
verifies affected source hashes, copies the existing tiny fixture and shell, and
fills only the relative-import closure needed by that shell. It never changes
the source checkout, downloads, installs, resets, deploys or creates credentials.
The build uses ordinary read-only paths to already installed dependencies; no
directory junction is required. Output must be new, with retained license notices.

Serve the built directory at `/map/` and a separately validated local merged
index at `/well-index-usgwd/`. This shell deliberately identifies historical
catalog records, mapped records, unmapped records and unknown physical identity.
It links to the publisher dataset/citation rather than inventing well-report URLs.
The existing USGS index remains a separate source generation; no proximity merge
or claim of physical-well uniqueness is made.

The scoped build and synthetic tests do not claim the entire unrelated GEV
application or upstream translation gates passed. Real data and browser evidence
are not part of this kit. Public distribution, hosting, deployment and upstream
submission remain separate actions.
