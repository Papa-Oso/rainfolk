# Engineering quick start

Clone the public repository, create a branch, and read AGENTS.md. Use a disposable
checkout for verification. The producer is Rust validation/indexing plus bounded
Python transport; the independent GEV JavaScript/Cesium consumer is separate.
No active collection or hosted application is implied by this source release.

## Prerequisites and commands

The published baseline records Rust/Cargo 1.99.1, edition 2024, and Python 3.
Windows also needs the MSVC linker/SDK. Use existing tools; if unavailable, report
the missing dependency rather than installing software without authorization.

One-time dependency preparation (official crates.io, no source collection):
```sh
cargo fetch --locked --manifest-path crates/us-groundwater/Cargo.toml
```

Then, from the repository root, with no live API credentials:
```sh
python -B scripts/check-workflow.py
cargo build --offline --locked --manifest-path crates/us-groundwater/Cargo.toml --bins
cargo test --offline --locked --manifest-path crates/us-groundwater/Cargo.toml
cargo fmt --manifest-path crates/us-groundwater/Cargo.toml -- --check
python -B -m unittest discover -s scripts -p 'test_capture*.py'
node --test integrations/gev/historicalPack.test.mjs
node --test integrations/gev/dataBase.test.mjs
python -B examples/offline.py
python -B examples/texas_sdr_offline.py
```

The USGS fixture example proves six IDs, identical replay, tamper rejection and
last-good retention. The synthetic SDR example proves four retained records,
three map-eligible with unverified accuracy, one nonspatial, and repeatable hashes.
That example also runs two invented SDR pages through the existing well-jobs
command, proving resume, cross-page deduplication and completed local replay.
Output belongs under ignored temporary/build directories. The release inventory
includes the reviewed SDR source additions and invented fixture only; real public
records and diagnostics remain outside the release. The wrapper verifies current
producer bytes in a temporary Git tree and scans explicit workflow files separately.
Direct `scripts/check-release.py` at the mixed repository root will reject workflow
additions; use the wrapper here. A passing check supports source publication review;
it does not authorize publication or certify live datasets.

## Pinned CI actions

The offline workflow uses Node24-native official action releases:

| Action | Release | Pinned commit |
| --- | --- | --- |
| `actions/checkout` | [v7.0.1](https://github.com/actions/checkout/releases/tag/v7.0.1) | `3d3c42e5aac5ba805825da76410c181273ba90b1` |
| `actions/setup-python` | [v7.1.0](https://github.com/actions/setup-python/releases/tag/v7.1.0) | `6d4fd22b20465ccc0f7354476588e8df7c9ca0e0` |

Their official release pages map to these full commit IDs and display GitHub's
Verified signature (key ID `B5690EEEBB952194`). This records GitHub's verification,
not a local GPG trust-store verification. The pinned
[checkout manifest](https://github.com/actions/checkout/blob/3d3c42e5aac5ba805825da76410c181273ba90b1/action.yml)
and [Python manifest](https://github.com/actions/setup-python/blob/6d4fd22b20465ccc0f7354476588e8df7c9ca0e0/action.yml)
both declare `node24`; the used inputs remain supported. Node24 actions require
runner version 2.327.1 or later; this workflow uses GitHub-hosted Windows runners.
Standard `pull_request`/`push` triggers, read-only repository permission and
`persist-credentials: false` remain. No unsafe-PR opt-in, custom Python mirror or
Node-runtime environment override is enabled. Final PR CI verifies compatibility.

## Work and evidence

Use the task issue template. Record outcome, scope/owned paths, acceptance,
exclusions, finite time/request/byte/cost budget and actual authorization.
Choose one writer per area. Keep independent work parallel only when explicitly
delegated; an agent is not a lock or permission grant.

A private GitHub Project should show Backlog, Ready, In Progress, Blocked and Done.
Issues are the public-safe source of task scope and evidence, not a second board.
Ready means scope and acceptance are actionable. In Progress means a writer is
active. Blocked records a concrete dependency/permission and next step. Done means
acceptance evidence and final source revision were reviewed, not merely coded.

Checkpoint format:
```text
Status | base/head commit | owned paths | completed evidence
Next step | remaining budget | blocker/permission state
```
Give a live update within 60 seconds while active; persist this checkpoint on
milestones, blockers and handoff. Do not publish chat history, machine paths,
credentials or personal details. Public screenshots/logs must be sanitized.

PRs link issues and exact check results, performance methodology and screenshots
where relevant. Distinguish local file timings from network latency or frame rate.
A fresh reviewer starts from the PR head with the issue, architecture map and
commands, without relying on the author's chat. Record the reviewed SHA and
remaining risks. Code changes after review invalidate that review.

## CI and permission boundaries

The offline-producer workflow uses one standard hosted Windows runner, read-only
repository permission, pinned checkout/setup actions, a 20-minute job limit and
cancellation of superseded runs. It fetches pinned Cargo dependencies as setup,
then runs offline checks without secrets, collection, publishing or deployment.
GitHub documents standard hosted Actions as free for public repositories.
A failed setup is a blocked check, never a passing test.

Project OAuth is separate from repository access. If scope is missing, stop and
request `project` authorization; never create a substitute board or refresh
credentials silently. Website publication, hosting/basemap terms and legacy
retirement remain separate approvals/gates.

Sources: [OpenAI harness engineering](https://openai.com/index/harness-engineering/),
[GitHub Actions billing](https://docs.github.com/en/actions/concepts/billing-and-usage),
[GitHub Projects](https://docs.github.com/en/issues/planning-and-tracking-with-projects).
