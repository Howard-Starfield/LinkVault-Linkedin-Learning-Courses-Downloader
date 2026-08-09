# Newspaper clipping note editor evaluation — 2026-08-09

**Phase:** 4A — isolated compatibility spike only
**Status:** Draft. D-024 remains Proposed; this report does not authorize Phase 4B.
**Branch / base:** `spike/newspaper-clippings-phase-4a-editor` at the evaluation
worktree based on `60071e4ee868dacb016c62ab0e69ba174e09b6f6`; merged target
`main` is `2500d6da6022d85032689567ecc479de2df11bc1`.
**Implementation commit / clean-run base:**
`c0a465d9560b7a6cd9406546fe8937cd47443926`
(`test(editor): evaluate clipping note editor compatibility`).

## Scope and entry gate

The Phase 1 and rustfmt-baseline prerequisites are merged. The master PRD marks
Phase 4A Ready and D-024 is the only applicable Proposed decision. This spike
does not change the production app route, Newspaper library/reader UI, Tauri
commands, persistence, Rust, autosave, navigation, or release version.

The adapter contract under evaluation is the D-023/FR-EDITOR-001 contract:
`documentId`, `initialMarkdown`, read-only and autofocus inputs; Markdown and
blur callbacks; optional ready callback; a focus/get-Markdown ref; and an
adapter root marked `data-editor-root="true"` with the accessible name
`Clipping note editor`. It has no Tauri calls or persistence ownership.

The common fixture and the adversarial/MDX-edge/empty/headings/2 MiB/switch/
composition fixtures are isolated in `apps/desktop/src/editor-evaluation`. Browser evidence
uses the repository-locked `playwright@1.60.0` library from a bounded Node
`.mjs` runner, not `@playwright/test` or the Playwright CLI.

### Environment

| Item | Value |
|---|---|
| OS | Windows 11 Enterprise 24H2, 10.0.26100.8875, 64-bit |
| Node / npm | v24.14.0 / 11.9.0 |
| Vite | 8.0.16 |
| React | 19.x from the application lockfile |
| Browser runner | `playwright@1.60.0`, headless Chrome channel |
| Evaluation server | worktree-local Vite only, `127.0.0.1:1421` |

The listener was verified free before every evaluation server launch. The exact
worktree-local Vite Node process was stopped after the run, and port 1421 was
verified free afterward.

## Candidate A — MDXEditor 4.2.0 (rejected)

### Package and maintenance review

| Item | Evidence |
|---|---|
| Direct package | `@mdxeditor/editor@4.2.0` |
| React peers | `react` and `react-dom` `>= 18 || >= 19` |
| License | MIT package metadata and bundled `LICENSE` |
| Official release | `v4.2.0`, 2026-08-02T14:58:08Z |
| Upstream state at evaluation | `mdx-editor/editor`, non-archived, push 2026-08-02, 89 open issues |
| Install behavior | No package lifecycle install script or native binary was observed in package metadata. |
| Runtime network | The browser run recorded only the isolated `127.0.0.1:1421` Vite resources; no CDN, telemetry, cloud, upload, or license-server request occurred. |

MDXEditor's Context7 documentation describes plugin-based headings, lists,
quotes, links, and imperative Markdown ref methods. The tested configuration
enabled only the matching plugins and omitted its JSX, image, table, code,
source/diff, remote, and collaboration features.

### Build and browser result

| Command | Result |
|---|---|
| `npm.cmd --prefix apps\desktop run build:editor-evaluation` | Exit 0 in 3,684 ms. Isolated entry JS: 871,270 bytes raw / 273,760 bytes gzip; CSS: 128,710 / 23,580 bytes. Vite emitted its standard over-500 KiB warning. |
| Worktree-local Vite plus candidate-A Node browser runner | Exit 2 in 14,209 ms; 10 of 17 checks passed and 7 failed. The raw JSON report was promoted into the findings below before candidate removal. |

Passing checks covered React Strict Mode surface count (the expected two ready
callbacks), allowed-fixture serialization/reload, headings 1–4/hard-break
round trip, empty and 2 MiB documents, parent acknowledgement, document-switch
isolation, synthetic-composition isolation/one-change behavior, and offline
network observation. Synthetic composition is regression coverage only; it is
not native IME evidence.

The seven failures were investigated rather than treated as one undifferentiated
package failure:

| Matrix row | Observation | Classification |
|---|---|---|
| Allowed semantic rendering | The strikethrough DOM assertion did not find `s`/`del`. The fixture still serialized; this needed a package-specific semantic check before it could count as proof. | Inconclusive visual-semantic assertion |
| Typing/undo/redo | Undo passed but the runner used `Ctrl+Shift+Z`; this MDXEditor toolbar exposes Windows redo as `Ctrl+Y`. | Runner defect, not a rejection reason |
| Unsupported content | The serialized adversarial Markdown still contained `<script`. It did not execute, but it was not reduced to the approved Markdown subset. | **Substantive rejection reason** |
| Image/file paste | No exact required rejection message was shown. | Adapter configuration gap |
| Toolbar | Blockquote was absent from the configured toolbar. | Adapter configuration gap |
| Dark/read-only screenshot | Playwright was given a `URL` object where its API requires a path string. | Runner defect |
| Console error | A 404 console error was recorded; the runner did not identify its resource before the candidate was removed. | Inconclusive harness observation |

The package is MDX-first and, under the constrained plugin configuration,
preserved raw script/MDX-like source in serialized Markdown. Correcting that
would require a separate canonical sanitizer/rejection layer in addition to
the editor configuration. That is not an acceptable proof that the selected
editor itself serializes only D-025's allowed subset. The direct audit also
returned two high findings: `@mdxeditor/editor` inherits `js-yaml` advisory
GHSA/CVE-2026-59870 (`js-yaml` 4.0.0–4.3.0; no fix available at evaluation
time). Whether frontmatter parsing is reached by this deliberately disabled
configuration was not established, so this is recorded as supply-chain risk,
not asserted as an enabled-path exploit.

**Result:** rejected for this Phase 4A comparison. Its dependency, lockfile
entries, candidate implementation, and generated/ignored experiment output
were removed before the retained candidate was finalized. The report retains
the version, build, package, audit, and browser-failure evidence.

## Candidate B — Tiptap 3.29.2 (retained, pending native gate)

The serial evaluation installed exactly:

```text
@tiptap/react@3.29.2
@tiptap/starter-kit@3.29.2
@tiptap/markdown@3.29.2
```

The three serial `--ignore-scripts` install commands exited 0 in 2,777 ms,
2,868 ms, and 1,272 ms. Candidate A was removed before this sequence: there is
no MDXEditor dependency/lock entry, candidate source, or retained generated
candidate-A output. References to Candidate A in this report are the required
historical evaluation evidence only.

### Package, license, maintenance, and supply-chain review

| Item | Evidence at evaluation time |
|---|---|
| Direct packages | `@tiptap/react`, `@tiptap/starter-kit`, and `@tiptap/markdown`, all `3.29.2` |
| React compatibility | `@tiptap/react` peers allow React and React DOM 17, 18, or 19; the application resolves React 19. |
| License | All three manifests and bundled `LICENSE.md` files state MIT, copyright (c) 2025 Tiptap GmbH. The required notice and full MIT text are in `THIRD_PARTY_NOTICES.md`. |
| Release date | npm registry time for each direct package: 2026-07-28T11:55:15Z (within milliseconds). |
| Maintenance | `ueberdosis/tiptap` was non-archived, MIT, on `main`, pushed 2026-08-08T13:32:31Z, and reported 845 open issues. The issue count is a maintenance risk to monitor, not a known enabled-path defect. |
| Install behavior | Direct manifests expose only `build: tsup`; no direct install lifecycle script or native binary was observed. The 53-entry lockfile delta is MIT-only and has no `hasInstallScript` marker; 15 installed transitive manifests declare `prepare` only. The serial evaluation installs used `--ignore-scripts`. `marked` exposes a JavaScript CLI bin but no install hook or native binary. |
| Audit | `npm.cmd --prefix apps\desktop audit --omit=dev --json` exited 0 / 939 ms: 0 info, low, moderate, high, and critical findings. |

The 53 new lockfile packages are the Tiptap/ProseMirror/Markdown dependency
closure. The lockfile records no new `hasInstallScript` package, while the
transitive `prepare` declarations are recorded above rather than omitted. The
configuration disables code, code-block, gapcursor, dropcursor,
horizontal-rule, underline, and related unsupported editor features even where
StarterKit's wider dependency closure contains their implementation packages.
Browser assertions prove that the disabled features cannot render or serialize
as V1 content.

Context7 documentation for Tiptap v3 confirms Markdown initial content via
`contentType: "markdown"`, `editor.getMarkdown()`, extension disabling via
`StarterKit.configure`, and deferred `immediatelyRender: false` initialization.
The adapter keys `useEditor` by `documentId` rather than calling `setContent`
on ordinary parent rerenders, avoiding Tiptap v3's default `setContent` update
emission.

### Adapter boundary and browser evidence

`ClippingNoteEditor` is LinkVault-owned and is imported only by the isolated
evaluation app. It exposes the required `documentId`, `initialMarkdown`,
read-only/autofocus, callback, ref, `data-editor-root`, and accessible-name
contract. It has no Tauri, persistence, autosave, backend, telemetry, CDN,
upload, or product-route dependency.

`normalizeClippingNoteMarkdown` runs before Tiptap parses initial Markdown and
again when Markdown is emitted or read from the ref. Its bounded line/block
scanner tracks quotes and bracket depth across at most 256 lines to remove real
multiline ESM imports/exports, fenced code, footnote definitions, and both
pipe-wrapped and pipe-less GFM tables, including the one- and two-hyphen
delimiter rows accepted by the installed Markdown parser. Imports require a quoted module
specifier, and export declarations require a declaration token, so ordinary
prose such as `import findings from yesterday` and `export const findings from
yesterday` remains untouched. A single pass makes nested MDX braces inert, and
bounded raw-tag removal prevents raw HTML/JSX syntax from
persisting. Images, inline code, task-list markers, and unsafe-link Markdown
are also reduced to safe allowed Markdown or inert readable text. Rich clipboard
HTML is flattened to `text/plain`; file/image paste is prevented with the exact
required copy. The normalizer deliberately does not render or preview HTML.

`immediatelyRender: false` defers Tiptap construction to a committed React
effect. In React 19 Strict Mode this removed eager discarded-render editor
instances and produced exactly one committed `onReady` callback in the harness.

| Command | Result |
|---|---|
| `npm.cmd --prefix apps\desktop run verify:clipping-note-editor-markdown` | Exit 0 / 474 ms. Direct Node regression proves the exact nested-MDX, multiline import/export, short and standard pipe-less table, ordinary import/export prose, and safe-link contract. |
| `npm.cmd --prefix apps\desktop run build:editor-evaluation` | Exit 0 / 4,421 ms. Code-split entry JS: 197.51 kB raw / 62.69 kB gzip; lazy editor JS: 454.21 kB / 142.53 kB; CSS: 84.56 kB / 16.09 kB. No Vite >500 KiB advisory was emitted. |
| `npm.cmd --prefix apps\desktop run verify:clipping-note-editor` | Exit 0 / 39,893 ms: 15 of 15 browser rows passed through worktree-local Vite at `127.0.0.1:1421` and repository-locked `playwright@1.60.0` headless Chrome. |
| Normal production build | Exit 0 / 5,075 ms. Main JS was 424.59 kB raw / 122.52 kB gzip; no normal route imports the adapter. |

The final browser matrix proves one committed React 19 Strict Mode ready
callback; approved Markdown semantics and repeatable heading/hard-break round
trips; empty and exactly 2,097,152-byte documents; typing/formatting/list
undo/redo and committed Chinese text; parent acknowledgement/no-op/failed
rerender stability; document/history/composition isolation; raw HTML, MDX,
unsafe links, image, table, code, task list, and footnote exclusion from both
the rendered DOM and serialized Markdown; the exact nested-MDX, multiline ESM,
and pipe-less GFM regressions; preservation of ordinary import- and
export-leading prose and an explicit safe link; rich-HTML flattening; file/image paste rejection;
toolbar order and dialog focus behavior; light/dark, high-contrast,
reduced-motion, and read-only behavior; and no external request, local failed
response, console error, or page error.

The 2 MiB fixture uses 16 large ASCII paragraphs. It tests the specified byte
boundary without silently changing the requirement into a 37,000-node DOM
stress test. Its final serialisation length was 2,097,152 bytes and its matrix
row completed in 731 ms. The synthetic composition row is browser regression
coverage only; it is not native IME evidence.

| Comparison | Candidate A: MDXEditor 4.2.0 | Candidate B: Tiptap 3.29.2 trio |
|---|---|---|
| Isolated JS build | 871.27 kB raw / 273.76 kB gzip in one entry | 651.72 kB raw / 205.22 kB gzip total: 62.69 kB startup entry plus 142.53 kB lazy editor chunk |
| Browser result | 10/17 passed; raw `<script` persisted | 15/15 passed; raw HTML/MDX, disabled GFM/code/task features, and unsafe links cannot round trip |
| Lifecycle | Candidate-specific evidence only | One committed ready callback after deferred construction; keyed document recreation preserves history isolation |
| Dependency audit | 2 high `js-yaml` findings, no fix at evaluation | 0 findings with production dependencies omitted |
| Decision | Rejected and removed | Retained, pending native Windows IME gate |

Candidate B is the retained winner for this spike, subject to the native
Windows gate below. It does not approve D-024 or Phase 4B.

## Phase 4B integration instruction

When separately authorized, Phase 4B should import only this LinkVault-owned
adapter at its clipping-detail boundary, not direct Tiptap packages. It should
change `documentId` only for an actual clipping switch, provide the persisted
Markdown as `initialMarkdown` for that new identity, and flush/resolve any
active composition before the switch. It must not add `setContent` on ordinary
parent acknowledgement or rerender. This spike deliberately implements none of
that production integration.

## Native Windows IME gate

The machine has Windows 11 and the user profile preloads English (`00000409`)
and Chinese (`00000804`) input methods, with `zh-Hans-CN` listed in the user
language profile. No real installed Tauri editor-evaluation session, candidate
window interaction, or keyboard/IME result has yet been recorded. Therefore
none of N-IME-01 through N-IME-10 is passed, and synthetic browser composition
must not be presented as a substitute. D-024 remains Proposed and PR #5 must
remain Draft unless every native case is completed against the retained
candidate.

### Required native UAT remains outstanding

The permitted Phase 4A process surface was worktree Node/Vite plus headless
Chrome. It did not provide a real desktop editor window in which to observe an
IME candidate window, and Phase 4A is prohibited from adding a production
Clippings route or Tauri integration merely to create one. The automated
composition case dispatches synthetic browser events and must never be claimed
as native IME success.

A Windows reviewer must perform these steps against the retained adapter in an
approved real desktop evaluation surface before D-024 can move from Proposed:

1. Record Windows version, keyboard layout, Microsoft Pinyin (or approved
   Traditional IME), Tiptap package versions, theme, and display scale.
2. Open a note with mixed Chinese/English punctuation and verify candidate
   selection, Enter, Space, arrows, punctuation, Backspace, and Escape without
   partial saves.
3. Verify undo/redo after a committed composition, formatting around Chinese
   text, and no focus/candidate-window regression in light and dark themes.
4. Trigger a parent acknowledgement while composing, request a document switch,
   and verify the guard preserves or resolves the active composition according
   to the editor contract.
5. Perform the platform screen-reader smoke for the labelled body, toolbar,
   heading selector, link dialog, read-only state, and visible focus.

Native screen-reader smoke is also unverified. Neither native gap may be
covered by the headless browser matrix.

## Work-order verification status

The following was rerun from the clean implementation commit above. Generated
`apps/desktop/dist` and `apps/desktop/dist-editor-evaluation` outputs were
removed after the commands; neither is part of the implementation commit.

| Command | Exit / elapsed | Status |
|---|---|---|
| npm.cmd --prefix apps\desktop run build | 0 / 3,831 ms | Passed. |
| npm.cmd --prefix apps\desktop run verify:clipping-note-editor-markdown | 0 / 463 ms | Passed: direct exact nested MDX, multiline import/export, standard/short pipe-less table, import/export-leading prose, and safe-link regression. |
| npm.cmd --prefix apps\desktop run build:editor-evaluation | 0 / 3,474 ms | Passed: lazy editor chunk 454.21 KiB raw / 142.53 KiB gzip; no Vite >500 KiB advisory. |
| npm.cmd --prefix apps\desktop run verify:clipping-note-editor | 0 / 29,169 ms | Passed (15/15), including synthetic composition only; it is not native IME proof. |
| npm.cmd --prefix apps\desktop audit --omit=dev --json | 0 / 830 ms | Passed: 0 production dependency vulnerabilities. |
| npm.cmd --prefix apps\desktop run verify:architecture | 0 / 481 ms | Passed. |
| npm.cmd --prefix apps\desktop run verify:persistence | 0 / 13,899 ms | Passed (39 tests). |
| npm.cmd --prefix apps\desktop run verify:ui | 0 / 491 ms | Passed. |
| npm.cmd --prefix apps\desktop run verify:visual | 1 / 32,189 ms | Existing unrelated baseline failure: against a verified worktree-local preview, the script timed out waiting for `Register archive`. |
| npm.cmd --prefix apps\desktop run verify:newspaper-performance | 0 / 471 ms | Passed. |
| npm.cmd --prefix apps\desktop run verify:newspaper-performance-browser | 0 / 12,967 ms | Passed: 8/50/500 edition browser-mocked profiles through a verified worktree-local preview. |
| npm.cmd --prefix apps\desktop run verify:newspaper-clippings | 1 / 366 ms | Script is absent from the manifest (Phase 4B gate gap). |
| npm.cmd --prefix apps\desktop run verify:newspaper-clippings-browser | 1 / 370 ms | Script is absent from the manifest (Phase 4B gate gap). |
| cargo fmt --manifest-path apps\desktop\src-tauri\Cargo.toml --check | 0 / 457 ms | Passed. |
| cargo clippy --manifest-path apps\desktop\src-tauri\Cargo.toml --all-targets | 0 / 1,201 ms | Passed with 36 pre-existing warnings, no error. |
| cargo test --manifest-path apps\desktop\src-tauri\Cargo.toml | 0 / 11,567 ms | Passed: 464 passed and 4 intentionally ignored. |
| npm.cmd --prefix apps\desktop run verify:release | 0 / 35,049 ms | Passed: architecture, persistence, release baseline, UI/build, and release verification. |
| git diff --check | 0 / 59 ms | Passed after generated evaluation and normal-build artifacts were removed. |

For the browser-dependent visual and performance checks, the evaluator first
proved port 1420 free, started an exact worktree-local Vite Node process, and
ended only that owned process after the check. The isolated editor harness used
the same ownership rule on port 1421. No browser server remained after either
run.

Intermediate command setup failures were isolated and rerun: bare PowerShell
`npx` resolved to the policy-blocked `npx.ps1`, while `npx.cmd --version` passed
at 11.9.0; no gate depends on `npx`. An initial PowerShell timing wrapper
incorrectly captured command output and reported false failures, and a direct
Windows spawn of `npm.cmd` returned `EINVAL` before the visual verifier began.
The streaming rerun and the npm CLI rerun above are the authoritative results.

The first combined Rust/release group exceeded its 604,040 ms outer tool limit
while compiling the release profile from scratch. Its child compilation was
monitored to completion; the isolated release rerun above then supplied the
definitive passing exit. Candidate B's early browser iterations exposed and
fixed a JavaScript navigation ceiling too short after lazy loading, an
unrequired 37,000-node 2 MiB fixture, a focus assertion that inspected a
missing aria-label on an implicitly labelled input, and eager Tiptap creation
that produced discarded-render Strict Mode ready callbacks. The first
post-sanitizer audit found multiline/nested MDX and pipe-less table gaps; the
bounded scanner and direct regression were added. A direct installed-parser
check then showed one- and two-hyphen GFM table delimiters also render tables,
so those forms were added to the same regression. The first prose-regression
browser pass exited 2 / 27,031 ms only because its verifier still treated every
`import`-leading line as ESM; the verifier was narrowed to quoted-module syntax.
The final whitespace audit replaced the heading fixture's trailing-space hard
break with equivalent backslash Markdown and added rendered/serialized/reload
assertions. The final 15/15 result above is definitive. These harness failures
did not mask any passing native IME claim.

## Rollback

This phase has no data, migration, filesystem, IPC, or normal-product routing
change. Rollback is removal of the selected evaluation adapter/harness and its
direct dependencies/lockfile entries only.
