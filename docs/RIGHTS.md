# Release rights gate

Joshua Koelker approved MIT licensing for Rainfolk's newly authored code and
documentation, with copyright 2026 Joshua Koelker. The root LICENSE records that
grant and the crate metadata agrees. This does not grant rights in source data
or remove third-party conditions. Final release/security and notice review remain
pending; approval of the original-material license is not approval to publish.

`LICENSES/GEV-MIT.txt` preserves the original upstream source-code notice and its
separate-data caveat. The GEV adapter remains a separate MIT-derived contribution.
Do not apply Bilawal Sidhu's copyright to newly authored Rainfolk code or remove
his notice from adapted source. The staged producer was developed in that
checkout; its exact imported source hashes are in `source-files.json`.

Cargo.lock pins official crates.io packages with checksums. Direct dependencies
are sha2 0.10.9, serde_json 1.0.145 and serde 1.0.229. The SDR member-validation
visitor reuses the already-locked Serde package; no registry package/version is
added by that direct declaration. Their transitive licenses and notices must
also be reviewed before publication; the lockfile is not a license grant. The
cached manifests' declared licenses are recorded in `dependencies.json`.
Forty-three existing top-level license/notice files from all 21 locked third-party
packages are preserved verbatim under `LICENSES/dependencies/`; no package lacked
a top-level notice. Declared choices include MIT/Apache-2.0, Unlicense/MIT,
Apache-2.0/BSL-1.0, and Unicode-3.0 conditions. This is notice preservation and
inventory, not final legal clearance. Public author/contact details in those legal
notices are deliberately retained, unlike private research/contact data. Python
transport uses only the standard library. Source datasets have separate rights
documented in DATA_SOURCES.md. No additional dependencies were installed.

## License scope

Scope: Rainfolk's newly authored code and documentation. Third-party code and
source data retain their separate terms; see LICENSES/, docs/RIGHTS.md and
DATA_SOURCES.md. This license does not relicense source datasets.
