# Migration candidate and remaining gates

This isolated candidate is based on public `Papa-Oso/rainfolk` main commit
`9ae3e61a658fa77ccc708ee5b9fd18dd12b86fdd`. Its README/history are retained.
No commit or remote mutation has been made. The legacy project is intended for
retirement, not a parallel product. Nothing has been deleted or archived.

## Source lineage and deliberate selection

Builder source HEAD is `a2c3a9766e251eb0311368cc77a1215d34221ec3` plus uncommitted
tooling and consumer changes. The explicit imported-file hashes in
`source-files.json` identify the actual copied bytes, including uncommitted work;
the HEAD alone does not identify this candidate. `release-files.json` records the
final file allowlist, sizes and hashes (excluding itself). No recursive application
or corpus copy was used. The working GEV checkout and collection evidence remain
unchanged and are not runtime dependencies of this checkout.

Selected: eight Rust source modules/binaries plus manifest/lock, bounded transport,
offline transport tests, protected Windows key helper/path test and one-row
verification helper, small USGS/synthetic fixture inputs, offline example and docs.
The original crate name/paths remain for compatibility. USGS fixture bytes retain
their source provenance; SHA lists are regenerated for included files only.
All 43 available top-level license/notice files from the 21 locked registry
dependencies are retained verbatim, with declared licenses in `dependencies.json`.
No dependency implementation source or new package installation was added.

Staging-only adjustments: the Windows helper's `Benchmark` action is removed
because its historical implementation requires a specific national workspace.
That script and its test are left behind, not advertised as portable. Its `Set`,
`Run`, `Status` and `Verify` actions remain. Fixture READMEs are replaced with
concise source-rights links; an obsolete missing-linker statement is not copied.
The small `candidate-bundle.json` is included because an offline transport
last-good regression reads it. No producer logic was rewritten for migration.

Retained separately: GEV app/adapter, browser QA evidence, current map, immutable
national captures/checkpoints/index and all old local artifacts/history. The
consumer patch is a separate, still-pending reproducibility deliverable.

Left behind: old sites/catalog experiments, TWDB/USGWD/HQA data, all national raw
responses and generated packs, live/expired queue configs, baseline-specific
prepare/benchmark scripts, temporary directories, build outputs and unrelated
upstream features/assets. No personal documents, environment files or secrets
were copied. New builds/test outputs in this checkout are ignored local artifacts,
not release files.

## Legacy dependency assessment

The candidate source, imports, fixtures and offline commands do not open a
Terlingua checkout or request its URLs. A historical link remains in the inherited
README, explicitly marked historical; it is not a build/runtime dependency.
There is no CI/deployment configuration in this minimal candidate. No website
configuration is claimed migrated. The old repository has not been inspected,
changed, deleted or archived by this staging operation.

The offline build uses the installed toolchain and official cached Cargo packages;
it does not depend on the GEV checkout or its build artifacts. The example builds
its own six-site candidate/index. The fixture queue uses only included synthetic
files and reports zero network requests. No old-repo path or network dependency
was used to run them.

## Builder validation

- Offline locked clean build succeeded in this new source checkout.
- 14 library tests and 46 queue/segment tests passed. Three index tests initially
  returned Windows Access denied for atomic replacement in sandbox execution;
  all three passed via the supported scoped execution approval, without changing
  production atomic-write behavior. Rust formatting passed.
- 24 offline Python transport tests passed. The first attempt identified a
  missing fixture; adding the required small fixture resolved it.
- Six-site export/index: six accepted IDs; identical replay; tampered candidate
  refused; last-good index retained. Index SHA-256:
  `50e33f288d28b552a088ada19782f870b82b95567eca619101899b7fcb0e9df8`.
- Fixture queue: 2, then 4, then 5 accepted IDs, completed only after final page;
  zero observations and zero network requests. Status reads the saved result.
- Windows path-only test did not run: PowerShell reported `AuthorizationManager
check failed` / `UnauthorizedAccess`. No policy change or alternate execution
  route was attempted. This is a remaining platform verification gap.
- Automated release-text checks cover private machine paths, common provider
  tokens/private-key blocks/email addresses, credential-bearing fixture URLs and
  owner/contact/address fields. No findings. These bounded checks are not a
  guarantee of comprehensive secret/privacy review; final fresh-context review
  is required. Public author/contact text in dependency legal notices is retained
  and excluded from the private-email heuristic. Keys were never read for these
  checks. `python -B scripts/check-release.py` repeats the manifest/fixture-hash
  and bounded sensitive-pattern checks without network access.

## Astra correction verification

The owner approved MIT licensing for original Rainfolk code/docs, copyright 2026
Joshua Koelker; LICENSE and crate metadata now agree. Inherited notices/data rights
remain separate. The producer now derives coverage from each pack's actual
readings and acquisition collections, including after detail compaction. Raw
tiles, audits and rollback indexes use the same immutable staged publication as
counts. An exclusive output-directory writer lock protects index.pending; no
automatic stale-lock stealing is implemented.

All 65 Rust tests pass in the isolated candidate, including the two new coverage
and writer/interruption regressions. Locked offline build and Rust formatting
pass. All 24 transport tests and the six-site offline replay/tamper example pass.
The example's changed hash reflects corrected coverage text, not changed source
measurements or IDs. No live collection or credential read was performed.

## Not safe to delete legacy repository yet

Local producer self-containment is demonstrated, but retirement is not complete.
Remaining gates: final dependency-notice and release/security review;
review the exact secret/rights/bloat allowlist; resolve the key-path test block;
produce/test the separate pinned GEV consumer patch; preserve a recoverable
legacy history/evidence snapshot without making it an operational dependency;
reconcile website/deployment dependencies through that task's owner; run the
strong release review; and confirm the approved source commit on the intended
public remote. No public push is authorized by this candidate preparation.

## Texas SDR source release reconciliation (October 26, 2026)

The earlier sections record the original import and its validation. The current
isolated SDR branch starts from public main `98ef0f9e72e4d411ac33f7328f31fcb5f3b2831d`.
The SDR adapter review covered `7e8f79fe5b26cd1c3ecc5de4a8f6519a98203383`;
release inventory reconciliation uses that reviewed commit as its base. Imported
`source-files.json` provenance and all existing USGS fixtures remain unchanged.
The current repository includes the separate workflow layer; use
`python -B scripts/check-workflow.py` for the producer hashes, fixture checks and
explicit workflow inventory, with all commands in [ENGINEERING.md](ENGINEERING.md).
CI now runs both transport suites and both offline examples.

The producer inventory adds nine SDR source/test/example/contract/fixture files.
Only the invented SDR fixture is included; real public sample records and all
diagnostic captures remain under ignored local paths. The checker enforces the
three-file SDR fixture allowlist, synthetic marker and exact checksum. An error
logging test uses non-email private-value sentinels, retaining the release email
heuristic and its logging assertions. No new dependency versions or runtime
adapter behavior were added by this reconciliation.

The source-only patch is prepared for final publication review. Publication,
real-data distribution, the separate consumer, website deployment and legacy
retirement retain their own gates; no upstream write or email is performed here.
See [Texas SDR contract and bounded pilot evidence](TEXAS_SDR.md) for source limits.
