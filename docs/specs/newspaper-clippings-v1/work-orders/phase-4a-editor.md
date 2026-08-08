# Work order: Phase 4A clipping-note editor compatibility spike

**Status:** Blocked draft

**Assigned branch:** `spike/newspaper-clippings-phase-4a-editor`

**Stacked base:** `feat/newspaper-clippings-phase-1-persistence`

**Primary specification:** `../05-clippings-library-and-note-editor.md`

**Blocking decision:** `../00-decision-register.md#d-024-wysiwyg-package-selection`

**Execution contract:** `../08-coding-agent-execution-contract.md`

## Entry gate

Codex must not edit production code or approve D-024 until all are true:

- Phase 1 PR is approved and merged to `main`.
- The Rust formatting-baseline prerequisite is merged.
- This branch is rebased or recreated from the resulting current `main`.
- Phase 1’s complete exit gate is green.
- The master PRD marks Phase 4A Ready.
- No Proposed decision other than D-024 blocks the spike.

This stacked draft PR exists now to preserve the work queue and the exact
review boundary. It does not authorize bypassing the entry gate.

## Objective

Evaluate at least two current React 19-compatible rich Markdown editor
candidates under LinkVault’s actual Tauri/Vite/Tailwind environment, prove one
candidate behind a LinkVault-owned `ClippingNoteEditor` adapter, record honest
automated and native evidence, remove rejected candidate dependencies from the
final branch, and update D-024 only when all blocking evidence passes.

Phase 4A does **not** add the production Clippings route, list, detail pane,
autosave integration, backend writes, source card, reader Clip action, or final
navigation. Its deliverables are an isolated adapter proof, tests/harness,
evaluation report, dependency/license evidence, and the approved decision.

## Mandatory reading order

1. `docs/architecture/README.md`
2. `docs/architecture/adr-001-unified-workflow-modular-monolith.md`
3. `docs/architecture/adr-002-newspaper-clippings-managed-assets.md`
4. `docs/specs/newspaper-clippings-v1/README.md`
5. `docs/specs/newspaper-clippings-v1/00-decision-register.md`
6. `docs/specs/newspaper-clippings-v1/05-clippings-library-and-note-editor.md`
7. `docs/specs/newspaper-clippings-v1/07-verification-performance-and-release.md`
8. `docs/specs/newspaper-clippings-v1/08-coding-agent-execution-contract.md`
9. `apps/desktop/package.json`, Vite config, TypeScript config, Tailwind setup,
   existing primitives, Playwright harness, build/visual/UI verification scripts,
   and third-party notice conventions.
10. The merged Phase 1 implementation and final PR evidence.

## Required preflight response

Before editing, update the PR body with:

- Exact current `main` and branch SHAs.
- Proof that Phase 1 and the formatting prerequisite are merged.
- Candidate shortlist and why each is plausibly compatible with the approved
  Markdown subset.
- Exact official package versions, peer dependencies, license, last release,
  and maintenance evidence gathered at evaluation time.
- Exact expected files and deliberately untouched files.
- How rejected candidates will be isolated and removed cleanly.
- Baseline production bundle measurements before adding any candidate.
- Automated test plan and native Windows Chinese IME test plan.
- Accessibility and security test plan.
- Exit-gate commands and rollback boundary.

Do not select a candidate from memory, popularity, a chat recommendation, or a
single demo. Inspect current official package metadata/documentation and run the
same fixture and measurement procedure for every candidate.

## Candidate requirements

Evaluate at least two materially different viable approaches:

1. A Markdown-first React editor with a maintained Markdown source/serializer.
2. A Lexical- or ProseMirror-family alternative whose Markdown integration can
   be constrained to the approved subset.

The spec’s examples are a starting point, not an approval. At evaluation time,
Codex must inspect current candidates and may reject one before full integration
only for a documented hard blocker such as incompatible React peer dependency,
unacceptable license, unavoidable executable MDX/raw HTML, remote runtime
requirement, or unmaintained/broken package. A hard rejection still needs exact
reproduction evidence, and at least two candidates must reach a meaningful
fixture/build evaluation unless no second viable candidate exists. In that
case, stop and request a decision amendment rather than declaring a winner by
default.

Do not keep rejected editor packages in the final `package.json` or lockfile.
Do not add multiple production editor implementations.

## Binding adapter contract

The selected candidate must be encapsulated behind this LinkVault-owned API:

```ts
export type ClippingNoteEditorProps = {
  documentId: string;
  initialMarkdown: string;
  readOnly?: boolean;
  autoFocus?: boolean;
  onMarkdownChange: (markdown: string) => void;
  onBlur: () => void;
  onReady?: () => void;
};

export type ClippingNoteEditorHandle = {
  focus: () => void;
  getMarkdown: () => string;
};
```

The adapter root must expose:

```text
data-editor-root="true"
aria-label="Clipping note editor"
```

Production code outside the adapter may not import the editor package. The
adapter may not invoke Tauri or own autosave. `documentId` must reset document
content, selection, undo history, and composition state without leaking state
between clipping IDs.

## Approved Markdown subset

The adapter must serialize only:

- paragraphs;
- headings levels 1–4;
- bold;
- italic;
- strikethrough;
- unordered and ordered lists, including the fixture’s nested list;
- blockquotes;
- links;
- soft and hard line breaks.

It must not emit or execute:

- images or arbitrary attachments;
- raw HTML;
- MDX, JSX, expressions, imports, exports, or components;
- tables;
- code blocks or inline code;
- task lists;
- footnotes;
- iframe, audio, video, embedded media, OCR, or AI content.

Only explicit `http`, `https`, and `mailto` links may be interactive. Unsafe and
unknown schemes remain inert text. No editor preview may execute HTML/MDX.

## Binding fixture

Every candidate must load, edit, serialize, reload, and compare this exact
fixture:

```markdown
# Research note

This is **bold**, *italic*, and ~~removed~~ text.

- First point
- 第二點
  - Nested item

1. One
2. Two

> Quoted observation

[Source](https://example.com/path?q=test)

A paragraph entered with Chinese IME: 世界日報剪報測試。
```

Also use boundary/adversarial fixtures:

- empty document;
- 2 MiB Markdown boundary document;
- unsupported image, table, inline code, fenced code, task-list, footnote,
  raw-HTML, MDX/JSX/import/export/component input;
- safe and unsafe links;
- nested lists and mixed Chinese/English punctuation;
- rapid document switching;
- Strict Mode mount/unmount/remount;
- read-only and disabled/invalid states;
- composition in progress during parent rerender and document-switch request.

## Required evaluation matrix

### React and lifecycle

Prove:

- React 19 production build;
- development Strict Mode double mount/unmount/remount;
- no duplicate listeners, toolbar instances, transactions, or change events;
- no stale document content/history/selection after `documentId` change;
- controlled parent rerenders do not reset cursor or composition;
- no console errors or unhandled promise rejections.

### Chinese IME

Native Windows evidence must cover at minimum:

- Simplified or Traditional Chinese IME candidate window;
- typing and candidate selection without partial document commits;
- Enter, Space, arrow keys, punctuation, Backspace, and Escape during composition;
- undo/redo after committed composition;
- formatting around Chinese text;
- parent state update while composing;
- document-switch guard while composing;
- light and dark theme candidate-window/focus behavior where observable.

Browser automation may test synthetic composition events but may not be claimed
as native IME proof. If Codex cannot perform native IME UAT in its environment,
it must leave D-024 Proposed, keep the PR Draft, add exact human UAT steps, and
stop without fabricating evidence.

### Markdown fidelity

For every allowed construct:

- edit visually;
- serialize to plain Markdown;
- reload from serialized Markdown;
- compare semantic structure and expected normalized Markdown;
- prove no package-specific JSON or metadata is persisted.

Document any harmless normalization, such as bullet marker or emphasis marker
choice. Reject a candidate when normalization causes semantic loss, unsupported
syntax insertion, or unstable repeated round trips.

### Unsupported content and paste

Prove:

- raw HTML/MDX is never executed;
- unsupported rich HTML is stripped or converted to safe allowed text;
- clipboard image/file payload is rejected non-destructively with exact copy:

  ```text
  Images aren't supported inside clipping notes.
  ```

- toolbar/plugin commands for image, table, code, task list, AI, remote upload,
  collaboration, and embeds are absent or disabled;
- unsafe link schemes are inert and cannot reach the Tauri opener.

### Undo/redo and parent updates

Prove undo/redo across:

- plain typing;
- formatting;
- lists and nesting;
- Chinese IME commit;
- controlled parent state updates that represent future autosave acknowledgement;
- failed/unchanged parent updates;
- document switch, where old-document history must not cross into the new ID.

The adapter must report changes after a complete editor transaction and must not
emit each partial IME composition string as a saved document.

### Accessibility

Record:

- keyboard access to the required toolbar controls;
- visible focus and logical focus order;
- accessible names, pressed/disabled states, and editor label;
- heading selector semantics;
- link-dialog focus trap/return behavior when present;
- read-only behavior;
- screen-reader smoke with the platform tool available to the reviewer;
- high-contrast and reduced-motion compatibility.

Required toolbar order:

```text
Undo
Redo
Heading
Bold
Italic
Strikethrough
Bulleted list
Numbered list
Blockquote
Link
```

No image, file, table, code, task-list, AI, or source-MDX control may appear.

### Theme and visual integration

Use existing LinkVault tokens/primitives. Test light, dark, high contrast,
focus-visible, disabled, read-only, narrow width, and ordinary desktop width.
Do not redesign the product or add an editor-specific global theme system.

### Offline and privacy

Capture network traffic while loading and editing the harness. The candidate
must require no CDN, remote schema, telemetry, cloud collaboration, upload,
license server, or runtime network request. Document optional features that are
not enabled and how they remain excluded.

### Bundle and lazy loading

Measure from a clean production build:

- baseline primary startup chunk raw/gzip size;
- baseline total emitted JS raw/gzip;
- candidate build primary startup chunk raw/gzip;
- lazy editor chunk raw/gzip;
- total emitted JS delta;
- whether non-Clippings routes import/initialize editor code.

The selected editor must be lazy-loadable. More than 500 KiB gzip added to the
primary startup chunk, or any unavoidable eager initialization on unrelated
routes, requires explicit product/architecture approval and blocks D-024.

Use a repeatable size script or captured build artifact report; do not estimate
from package tarball size.

### License, maintenance, and supply chain

For each candidate record:

- exact direct dependency version/range;
- license from package metadata and repository license file;
- required `THIRD_PARTY_NOTICES.md` entry;
- transitive license scan/report;
- release date/cadence and open maintenance risks at evaluation time;
- install scripts, native binaries, remote assets, telemetry, and suspicious
  package behavior;
- vulnerability audit results and relevant false-positive review;
- lockfile diff.

Do not approve a package with an incompatible or unclear license, required
network service, unreviewed native binary, or unresolved critical/high
vulnerability affecting the enabled path.

## Phase 4A implementation artifacts

The final branch may contain only the selected candidate and evaluation support:

```text
apps/desktop/src/components/newspaper/ClippingNoteEditor.tsx
apps/desktop/src/components/newspaper/clipping-note-editor-*.ts
apps/desktop/src/components/newspaper/__tests__/... or existing test convention
apps/desktop/src/editor-evaluation/...     isolated non-production harness
apps/desktop/scripts/verify-clipping-editor-*.mjs when justified
apps/desktop/package.json
apps/desktop/package-lock.json or repository lockfile
THIRD_PARTY_NOTICES.md

docs/evaluations/newspaper-clipping-editor-2026-08-08.md
docs/specs/newspaper-clippings-v1/00-decision-register.md
```

Exact file placement may follow repository conventions. The evaluation harness
must not be linked from normal production navigation and must be removable
without changing the adapter contract. Rejected candidate code/dependencies
must not remain.

The evaluation report must include:

- exact environment and commands;
- exact package versions and configuration;
- candidate comparison table;
- fixture and round-trip results;
- unsupported-content/security results;
- automated and native IME evidence;
- accessibility evidence;
- network/offline evidence;
- raw/gzip bundle deltas;
- license, notice, audit, and maintenance evidence;
- selected candidate and explicit rejected reasons;
- adapter/plugin configuration;
- known limitations;
- Phase 4B integration instructions;
- rollback procedure.

## D-024 update rule

Update D-024 from Proposed to Approved only when:

- at least two candidates were meaningfully evaluated;
- one candidate passes every blocking matrix row;
- native Windows Chinese IME evidence is real and recorded;
- React 19 production/Strict Mode tests pass;
- approved Markdown round trips are stable;
- unsupported HTML/MDX/images/tables/code are safe;
- accessibility blocking issues are resolved or explicitly approved;
- offline/network evidence is clean;
- bundle behavior is acceptable and lazy;
- license/notices/audit are complete;
- rejected candidate dependencies are removed.

The decision entry must name the package, exact approved dependency range,
adapter configuration, evaluation report path, approval date, and known
limitations. If any blocking evidence is unavailable, leave D-024 Proposed and
keep Phase 4B blocked.

## Required verification

Run from a clean final worktree:

```powershell
npm.cmd --prefix apps\desktop run build
npm.cmd --prefix apps\desktop run verify:architecture
npm.cmd --prefix apps\desktop run verify:persistence
npm.cmd --prefix apps\desktop run verify:ui
npm.cmd --prefix apps\desktop run verify:visual
npm.cmd --prefix apps\desktop run verify:newspaper-performance
npm.cmd --prefix apps\desktop run verify:newspaper-performance-browser
npm.cmd --prefix apps\desktop run verify:newspaper-clippings
npm.cmd --prefix apps\desktop run verify:newspaper-clippings-browser

cargo fmt --manifest-path apps\desktop\src-tauri\Cargo.toml --check
cargo clippy --manifest-path apps\desktop\src-tauri\Cargo.toml --all-targets
cargo test --manifest-path apps\desktop\src-tauri\Cargo.toml
npm.cmd --prefix apps\desktop run verify:release
git diff --check
```

Add focused editor unit/browser tests and a repeatable bundle-report command as
needed. Record exact command, commit, OS, exit code, elapsed time, and relevant
output. Do not claim native IME or screen-reader success from synthetic browser
events.

## Codex start prompt

```text
Work only on the draft Phase 4A PR branch
`spike/newspaper-clippings-phase-4a-editor`.

Do not edit production code or approve D-024 until PR #2 and the rustfmt
prerequisite are merged, this branch is rebased on current main, and the master
PRD marks Phase 4A Ready. Read the documents in this work order's mandatory
order.

Before coding, update the PR body with entry-gate proof, current candidate
versions/licenses/peer dependencies, baseline bundle measurements, expected
files, automated fixture plan, native Windows Chinese IME plan, and exit
commands. Evaluate at least two viable candidates with the same fixture and
measurement process. Keep only the selected dependency and LinkVault-owned
adapter proof in the final branch. Do not add the production Clippings route,
autosave, backend writes, reader UI, source navigation, OCR, AI, tags,
annotations, or release changes.

Do not fabricate native IME, screen-reader, security, maintenance, license, or
bundle evidence. Leave D-024 Proposed and the PR Draft when blocking evidence is
missing. Run every Phase 4A gate, record intermediate failures and fixes, and
stop. Do not merge and do not begin Phase 4B.
```
