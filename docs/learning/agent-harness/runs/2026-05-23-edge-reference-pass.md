# 2026-05-23 Edge And Reference Pass

## Purpose

Double-check migration edge cases and bind the migration harness to `LinkVault/design.md` and `LinkVault/reference.png`.

## Findings

- `design.md` is a shell blueprint, not just a theme note. It requires layout, primitive, trigger, accessibility, overlay, and drift-policy discipline.
- `reference.png` is 1536x1024 and should be the primary desktop screenshot target.
- The current C# app already has deterministic tests for token validation, trial prompt detection, enterprise hash extraction, exercise URL parsing, 1080 fallback, and zip extraction safety.
- The migration needs extra explicit decisions around batch failure behavior, token storage, Generic Video nav visibility, and app restart recovery.

## Files Added

- `agent-harness/README.md`
- `agent-harness/STATUS.md`
- `agent-harness/TODO.md`
- `agent-harness/REFERENCE_CONTRACT.md`
- `agent-harness/EDGE_CASE_MATRIX.md`
- `agent-harness/META_PROMPT.md`
- `agent-harness/runs/2026-05-23-edge-reference-pass.md`

## Verification

No application code was edited. This pass is documentation and migration-control only.

