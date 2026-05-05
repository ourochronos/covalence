# Proposed amendments — change 0001-bootstrap

**This directory is intentionally empty.**

Future architectural changes use this directory to stage edits to `docs/architecture/` (the holistic spec), which then merge into `docs/architecture/` at change closure (per INV-3: holistic spec evolves only through change closure).

The bootstrap is the one exception: it creates `docs/architecture/` from scratch — there is nothing to amend, so amendments are written directly. This deviation from INV-3 is recorded in `decisions.md` and is the bootstrap's only invariant exception.

For all subsequent changes, this pattern applies:
- During phase two (implementation), proposed edits to `docs/architecture/<file>.md` are staged here as full-file copies with the proposed changes applied.
- At phase three (reconciliation) closure, the staged files merge into `docs/architecture/`.
- Two concurrent changes amending the same section must rebase the second through change-spec authoring at close time.
