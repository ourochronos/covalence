# Backlog

This directory holds tech debt, features, observations, and questions that are tracked but not yet (or not ever) becoming an architectural change. The backlog is **the local authoritative issue tracker** for items that don't warrant a GitHub issue — small observations, deferred cleanups, soft-warning acknowledgements.

Per `CLAUDE.md` and the claude-ultra skill: when work is non-trivial enough to deserve cross-session memory, it lives either as a GitHub issue (most things) or here (smaller observations, framework-specific signals).

## When to file here vs GitHub

**File on GitHub:**
- New features, bug fixes, refactoring, infrastructure changes
- Spec-implementation misalignments
- Knowledge gaps that will drive ingestion
- Anything that benefits from labels, comments, cross-references

**File here:**
- Soft warnings acknowledged-and-deferred during a change
- Triggers from deferred manifest cells (mirrored here so they're visible across changes)
- Process observations specific to claude-ultra's operation in this repo
- Small refactoring observations that aren't worth a GitHub issue but shouldn't evaporate

When in doubt, GitHub issue. The backlog is for things that would otherwise live only in commit messages or session logs.

## File format

Each issue is a single Markdown file: `ISSUE-NNNN-short-slug.md`. Numbering is sequential. See `_TEMPLATE.md` for the structure.

## Lifecycle

- **Open** — the default state when filed.
- **Resolved** — when the work is done; mark in the file rather than deleting (audit trail).
- **Closed (won't fix)** — explicit decision not to address; record rationale.

When closing an architectural change, scan this backlog for issues the work likely addressed and propose resolution.
