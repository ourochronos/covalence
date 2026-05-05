# Proposed amendments — change 0001-bootstrap

**This directory is intentionally empty.**

Future architectural changes use this directory to stage edits to `docs/architecture/` (the holistic spec). Per claude-ultra's framework rule (the holistic spec evolves only through change closure), staged amendments live here during phase two and merge into `docs/architecture/` at phase-three closure.

The bootstrap is the one exception: it creates `docs/architecture/` from scratch — there is nothing to amend, so amendments are written directly to `docs/architecture/` instead of being staged here. This deviation from the closure-only rule is recorded in `decisions.md` (Decision 11) and is the bootstrap's only such exception.

For all subsequent changes, this pattern applies:
- During phase two (implementation), proposed edits to `docs/architecture/<file>.md` are staged here as full-file copies with the proposed changes applied.
- At phase three (reconciliation) closure, the staged files merge into `docs/architecture/`.
- Two concurrent changes amending the same section must rebase the second through change-spec authoring at close time.

(Note: this rule is part of the claude-ultra framework, *not* one of Covalence's numbered invariants in `docs/architecture/invariants.md`. Covalence's INV-3 is "No attention dilution in extraction" — unrelated.)
