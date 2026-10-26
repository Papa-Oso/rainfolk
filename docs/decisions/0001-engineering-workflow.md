# ADR 0001: Repository-native engineering workflow

Status: proposed in workflow PR. Date: 2026-10-26.

## Context
Rainfolk publishes a standalone producer. Rust validates bounded captures,
reconciles native IDs, accounts for queue budgets and builds spatial artifacts.
Python handles deterministic bounded transport. Versioned contracts connect to
the separate GEV consumer. The public website and consumer delivery have separate
release gates. Inventory metadata does not establish groundwater or potable-water
conditions; nationwide source retrieval is not a complete well registry.

## Decision
Use GitHub Issues for task contracts, one private Project for priorities/status,
small branches and PRs, Codex for execution, and fresh-context review of the same
source revision. AGENTS.md is a short map to durable docs, not a task diary.
Existing offline checks provide evidence. No custom scheduler, orchestration
service or agents-per-record runtime is introduced.

Keep the current producer manifest unchanged while its owner works on the next
release. A bounded workflow verifier recognizes exactly the workflow additions,
checks producer bytes with the existing release verifier and refuses unknown
files. It is a compatibility layer, not a replacement release manifest.
Before a combined release, the producer owner must reconcile its release inventory.

## Consequences
One writer per work area reduces conflicting edits; independent review reduces
shared-context blind spots. Explicit budgets, checkpoints and permission state
make long tasks resumable. CI cannot approve deployment, data rights or API access.
Project management can remain permission-blocked while repository work proceeds.
Performance claims require reproducible methods and visual changes require actual
visual evidence; passing unit tests alone does not satisfy those gates.
