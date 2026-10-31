# Separate God's Eye View consumer

Upstream: https://github.com/bilawalsidhu/gods-eye-view . This is an independent
project, not an official partnership. The existing local integration stays in its
GEV checkout; this release candidate does not vendor the application or run it.

Current delivery status: the GEV preview uses the published 36-state historical
feed. The separately validated local 48-state candidate is not yet published.
Full visual acceptance and user approval remain gates before upstream submission.
See [current counts and limitations](../README.md#current-status). Older builder
and browser attempts below are historical evidence, not the current coverage list.

Recorded local upstream base: `9d3548c27c2cd4bf80ea39cbb3f0634ab1c5a06b`.
Recorded builder HEAD: `a2c3a9766e251eb0311368cc77a1215d34221ec3`, plus uncommitted
layer/collector changes. Neither is a claim about current upstream main. A
historical builder status is superseded for the scoped Rainfolk water layer by
[the source-only consumer kit](../integrations/gev/README.md). That kit pins the
base and affected-file hashes, prepares an isolated source tree from existing
local Git objects, and builds with existing dependencies. It does not modify an
installed GEV app. Consumer publication, visual acceptance and legacy retirement
remain separate gates. This document alone is not an app installation.

Consumer hooks are confined to the groundwater layer/source/pack modules,
catalog/source registration, immutable layer token, shared overlays and layer
panel, source credit, translations and their tests. Indexed data is static and
versioned; the optional six-site fixture remains available. No legacy research
portal or founder-hosted endpoint is required. Preserve upstream MIT notices.

The builder's latest affected 104 tests/build/translation/boundary checks passed.
Latest screen-aware grouping visual acceptance remains pending after a browser
coordinate-mapping safety stop. Do not claim those builder checks are an
independent release review. Before upstream/public consumer delivery, test the
patch on its selected clean base with required upstream gates and actual desktop
visual acceptance. Those tasks are separate from the producer offline test here.

The historical-record extension and its staged detail compaction have passed
fresh-context source review, offline producer tests, a clean pinned-base consumer
build, and the actual consumer-loading tests. Local real-data index/transport
checks are distinct from interactive visual acceptance. The current in-app
browser attempt could not reach the local preview; visual acceptance remains
pending. No upstream submission, installation change or deployment follows from
these source checks. See [the historical map contract](USGWD_MAP.md).
