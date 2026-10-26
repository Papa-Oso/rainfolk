# Synthetic Texas SDR evidence

`synthetic-v1.json` contains four invented construction-report rows, not a live
TWDB response or well inventory. Two report IDs share coordinates deliberately;
the other rows exercise nonspatial retention for missing coordinates and
uninterpreted nonzero LocationError. All four records are retained; three have
map-eligible unverified coordinates and one remains nonspatial. No code meaning
is inferred from LocationError.
No owner, address or contact values are included. The source URL identifies the
reviewed schema only. The timestamp is synthetic, not an actual feature request.

See [source contract and public-pilot evidence](../../docs/TEXAS_SDR.md).
`SHA256SUMS` covers the included JSON fixture's exact bytes.
