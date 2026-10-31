# Snapshot refresh: implementation plan and boundary

The implementation builds on the reviewed historical-map exporter. Owned work is one
`well-jobs` diff command, one Python snapshot runner, synthetic tests/config,
this contract and the producer source inventory. Existing data stays immutable.

1. Reuse the verified USGWD manifest/index reader for source-record diffs.
   Compare complete snapshots, retain before/after evidence, distinguish missing
   and quarantined IDs, and never resolve identity through coordinates/state-ID text.
2. Watch a bounded local CSV/receipt inbox by complete file SHA-256. Reuse the
   segmented importer and existing map exporter only for changed source states.
   Unchanged states retain their exact previous export reference. This is
   full-snapshot detection/comparison, not a remote incremental changefeed.
3. Persist fingerprints, stage checkpoints, deadlines and immutable run history.
   Advance a local candidate reference only after all stages succeed; publication
   is always manual. A missing row is a review event, never a plugged-well claim.
4. Expose one finite CLI invocation and inactive scheduler configuration. Test
   detection, no-op/reuse, changed/missing records, cross-release identity,
   interrupted resume, tampering, failed exports and last-good preservation.

The first supported profile is the existing USGWD CSV contract. Its generated
IDs are release-scoped: a new release produces distinct IDs and unresolved
aliases, not unconditional upserts. Official-state adapters must supply their
own documented source namespace/stable ID contract before joining this path.

The inbox must be populated by a separately authorized acquisition using the
existing bounded source tools. This runner does not fetch remote URLs, treat a
landing-page timestamp as data freshness, or claim a source was checked remotely.
ETag/Last-Modified observations may be retained in receipts by an acquirer, but
are not a substitute for verifying the actual complete input hash. Remote
polling/acquisition, schedule activation, hosted pointer changes and publication
remain separate gates. No scheduler/service/database is introduced.

For this profile those HTTP observations belong in the acquirer's separate
receipt; the existing USGWD import receipt retains its strict field allowlist.
This runner's automatic change signal is the complete local file hash only.

## Executable local path

After the normal offline build, these commands use only the two existing
invented USGWD fixture rows:

```sh
python -B scripts/refresh-wells.py examples/refresh-offline.json .temp-gev/refresh-example-v2 --plan
python -B scripts/refresh-wells.py examples/refresh-offline.json .temp-gev/refresh-example-v2
python -B scripts/refresh-wells.py examples/refresh-offline.json .temp-gev/refresh-example-v2
```

The second invocation creates a local review candidate; the third reports
`unchanged` without importing, diffing or exporting. Production configurations
replace the CSV and receipt paths with an immutable, verified local snapshot.
Keep the inbox CSV/receipt pair fixed until that job completes. The receipt must
be the existing exact USGWD source contract, not a new inferred source schema.
`localCheckedAt` records this computer's file check; source publication,
modification, acquisition/checked dates and observation years remain separate.
If identical source bytes arrive with a changed receipt contract (apart from
the check timestamp), the runner holds for review rather than silently accepting
new attribution/schema/freshness assertions or reprocessing unchanged data.

The new Rust command is also usable independently:

```sh
well-jobs diff-usgwd previous-manifest.json current-manifest.json new-diff-directory 900
```

Use `-` for no prior snapshot. It verifies complete normalized inputs, row hashes,
row/ID accounting, quarantine evidence and provenance. Delta shards contain
added/changed/missing/previously-accepted-now-quarantined IDs with immutable
before/after evidence and exact raw state-ID aliases. `unchanged` compares the
normalized content hash, which excludes snapshot provenance; merely reordering
rows or checking a file later does not invent changed measurements. A new release
has a different source namespace even if its generated numbers or state-ID text
match. Anonymous/current quarantines remain separately visible in accounting.
No event authorizes deletion or infers physical-well status.

## Incrementality and export reuse

The scheduling unit is a source/state CSV, not an individual well. SHA-256 reads
the full inbox file. A changed file is normalized as a complete snapshot and
compared to the pinned previous successful snapshot. There is no high-ID or
completion-date watermark and no claim of remote changefeed completeness.

Only that changed state's existing map exporter runs; other states retain exact
hash-pinned index references. This is state-partition incremental processing.
Within a changed state the exporter scans the complete normalized snapshot and
regenerates its candidate; fine-grained changed-cell export selection is not
implemented. Identical content-addressed count subtrees remain identical, but
new source provenance can legitimately change detail-pack hashes. No publication
or global federation root is changed by this command.

`refresh-state.json` is a hash-checked local registry of successful review
candidates. Each immutable `complete.json` links its previous candidate and the
normalized manifest, delta report and index. A missing/quarantined ID or changed
release sets `requiresReview`; history and the previous export remain present.
Each transition is keyed by both the incoming fingerprint and the previous
completion hash. An A to B to A sequence therefore creates a new B-to-A comparison,
including missing-record warnings, rather than reusing the original A job.
Even a candidate without warnings has `activation=manual-only`. These references
are review artifacts, not the public feed's active pointer. Switching a reviewed
feed back to an older version remains possible because old versions are retained.

## Finite execution and recovery

One invocation finishes at most one changed state and checks at most 50 configured
sources. Default total lifetime is 7,200 seconds; explicit limits are 1,800..21,600
seconds, pinned before work. Resume never renews a job's deadline or 24-operation
allowance. Each segmented-import command retains its 300-second native limit;
diff and historical export reserve 900 seconds each. At most 16 normalization
segments/two million rows, 512 MiB input, 34 GiB normalization output, 2 GiB deltas,
16 GiB state export, 16 MiB metadata and 8 MiB delta/detail artifacts are admitted.
The runner observes child peak memory with a 2 GiB stop guard and diagnostic files
with 1 MiB caps. Native limits and an external parent watchdog remain distinct.
Reserve 80 GiB free before a new state job and stop below 24 GiB during execution.
History is not automatically pruned to make room.

`--steps 1` permits a clean planned pause after one command. Repeat to continue
the same job. OS writer locks prevent concurrent runners. Known child failures
retain the prior candidate and require explicit `--resume`; schedules never
automatically retry them. A parent crash leaves a `running` marker: verify that
the saved child has stopped before using `--ack-stopped`. Only a matching stale
index lock can then be removed. Source bytes, config, receipt, policy and executable
hashes are pinned; changing any during a pending job refuses continuation.
Executable hashes are rechecked immediately before every child launch, so a
between-stage rebuild cannot run under an earlier recorded hash. Protocol
`usgwd-local-refresh-v2` refuses v1 state; retain older evidence and use a new
reviewed state directory rather than editing its policy marker or resetting budgets.
An expired allowance needs an explicitly reviewed new execution window; this
command has no automatic deadline reset or amendment mechanism.

## Inactive scheduling configuration

`examples/refresh-offline.json` contains the exact entry point and
`schedule.enabled=false`. `--plan` validates it without writing state or source
files. It does not install or activate anything. An approved external Task
Scheduler/cron job can invoke the same command, with one configuration per state
recommended to isolate holds. For example, a daily cron command after explicit
activation approval would invoke:

```sh
python -B scripts/refresh-wells.py /operator/config/state.json /operator/local-refresh-state
```

No GitHub Actions schedule, daemon, credentials or hosting setup is added.
Missing inbox data, failed jobs and expired reservations must be surfaced by that
operator's scheduler. Remote freshness requires a separately tested and authorized
source-specific acquisition profile. The first such profile should use complete
official state snapshots where no reliable modified-record feed exists; it must
not rename USGWD IDs into official-state IDs or silently combine sources.

Acceptance: `python -B -m unittest discover -s scripts -p test_capture_refresh.py`
uses synthetic local snapshots through the real importer, diff and exporter.
It covers no-op detection, state reuse, content changes, missing IDs, release
boundaries, conflicts, deterministic deltas, pause/resume, known failures,
uncertain children, stale deadlines, tampering, ownership and inactive planning.
Each attempted native command retains a separate bounded outcome/diagnostic
receipt; successful recovery does not overwrite the failed attempt's evidence.
