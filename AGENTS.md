# Rainfolk agent map

Start with [engineering quick start](docs/ENGINEERING.md) and
[architecture decision](docs/decisions/0001-engineering-workflow.md).
Product scope: [README](README.md). Data boundary: [artifact contract](docs/ARTIFACT_CONTRACT.md).
Separate consumer: [GEV integration](docs/GEV_INTEGRATION.md).
Rights and safety: [component rights](docs/RIGHTS.md), [data sources](DATA_SOURCES.md),
[security](SECURITY.md), [migration gates](docs/MIGRATION.md).

## Working rules
- Read the issue's scope, acceptance checks, exclusions, budget and permissions before editing.
- Use a separate branch/checkout. One writer owns each work area; delegate only independent work.
- State the starting commit and owned paths. Never edit another active checkout.
- Give a useful progress update within 60 seconds while active. For a long command,
  report its purpose first; poll without blocking updates. Persist a checkpoint on
  milestones, blockers and handoff, not every minute in public issues.
- Stay within existing authorization. Stop the denied action; record target, reason,
  completed work, pending/denied/uncertain outcome and exact permission needed.
  Read remote state before retrying an uncertain write.
- No live collection, credentials, deployment, paid services, upstream writes or
  legacy deletion without specific authorization. No LLM calls per data record.
- Keep IDs, missingness, provenance and units intact. Source pagination completion
  is not complete well coverage; locations are not water-level observations.
- Use the exact commands in ENGINEERING.md. Never invent passing checks.
- Open a small PR with evidence. A fresh reviewer gets the same final commit,
  issue, diff, commands and logs; source changes after review require re-review.
- Do not merge or change security settings merely because CI passes.

## Manifest boundary
The producer release manifest is maintained by the producer owner.
For this workflow layer run `python -B scripts/check-workflow.py`.
It verifies the unchanged producer manifest plus an explicit workflow file set.
Do not regenerate producer provenance or manifest as a side effect of documentation work.
