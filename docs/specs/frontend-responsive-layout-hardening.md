# PRD: Responsive frontend and desktop-shell hardening

**Author:** LinkVault engineering

**Date:** 2026-08-19

**Status:** Draft for implementation

**Reviewer:** TBD

## Context

LinkVault's desktop shell is resizable, but the current frontend does not have one explicit responsive-layout contract for the shell, sidebar, provider workspaces, and nested controls. The result is visible instability when the window or left navigation rail is resized.

This PRD is based on the current `main` implementation rather than assumptions:

- `apps/desktop/src/App.tsx` defines a sidebar range of 208-320 px, persists the selected width, and updates `--sidebar-width` imperatively during mouse drag before committing React state on mouseup.
- The same custom property is also supplied through the React `style` prop, creating two writers for the live drag value.
- `apps/desktop/src/index.css` gives `.lv-brand-logo` and its image `width: 100%`. Therefore widening the sidebar necessarily enlarges the LinkVault wordmark.
- Internal responsive rules are primarily viewport `@media` queries even though the width actually available to provider content is the viewport minus the sidebar and other fixed columns.
- The native Tauri window is resizable but has `minWidth: 1280` and `minHeight: 720`. Native acceptance therefore must concentrate on the real 1280 px floor rather than treating 980 px or 520 px CSS breakpoints as the primary desktop path.
- `scripts/verify-visual.mjs` already performs useful Playwright geometry sweeps for the Newspaper view, including a 1280 px viewport and horizontal-overflow checks. It does not currently exercise live sidebar dragging, logo invariance, LinkedIn/Coursera geometry with a wide sidebar, or concurrent React updates while a drag is in progress.
- `tsconfig.json` enables TypeScript `strict`, but `strict` does not prohibit explicit `any`. There is no dedicated repository gate that fails on an explicit `any` keyword.
- Rust ownership is already structured under `src-tauri/src/app`, `providers`, and `workflow`, with `verify-architecture.mjs` enforcing important backend boundaries. Frontend ownership is less explicit: visual concerns, IPC calls, input normalization, and some domain calculations are still mixed in large view components.

A GitHub code-search pass and targeted inspection of `App.tsx`, `components/primitives.tsx`, `components/coursera/CourseraView.tsx`, and `components/newspaper/newspaper-api.ts` did not find explicit `: any` or `as any` in those inspected sources. Search-index coverage is not sufficient to treat that as a permanent repository-wide guarantee. The implementation phase must establish the definitive baseline with an AST-based source scan before other changes are accepted.

## Product goal

Make LinkVault behave like a stable desktop application while the user resizes either the native window or the left navigation rail: branding remains visually fixed, controls remain inside their intended regions, page layouts reflow from the space they actually receive, and no domain behavior is moved into React merely to make the UI easier to implement.

This is a layout and architecture hardening effort, not a visual rebrand.

## Principles

1. **Available space, not viewport assumptions.** Provider layouts must respond to the width of their actual content container.
2. **One owner per live layout value.** A drag value must not be independently controlled by React render state and imperative DOM mutation.
3. **Reflow before clipping.** `overflow: hidden` must not be used to conceal controls that no longer fit.
4. **Presentation stays in the frontend. Domain authority stays in Rust.** Pointer input, element geometry, focus, responsive state, theme, and other presentation-only concerns belong in React/CSS. Validation, scheduling, persistence, filesystem work, queue behavior, auth, provider rules, and durable application decisions belong in Rust.
5. **No explicit TypeScript `any`.** Unknown external values start as `unknown` and are narrowed or decoded.
6. **Regression behavior is executable.** Important geometry rules must be asserted in Playwright or other repeatable gates, not only written in CSS comments or verified by string matching.

## Goals

- G-1: Resizing the sidebar must never scale the LinkVault wordmark.
- G-2: Resizing the sidebar across its full supported range must not push provider content, controls, dialogs, tables, or side panels outside their layout owner.
- G-3: Resizing the native window throughout the supported native range must produce deterministic reflow without horizontal document overflow.
- G-4: Provider pages must adapt to the width they actually receive after sidebar and secondary-column allocation.
- G-5: Live dragging must remain smooth and must not snap backward when unrelated React/Tauri state updates occur.
- G-6: Sidebar collapse/reopen behavior and persisted width must remain predictable across relaunch and window-size changes.
- G-7: Project-owned TypeScript/TSX must have a repeatable zero-explicit-`any` gate.
- G-8: This work must strengthen, not erode, the Rust/frontend ownership boundary.
- G-9: Existing visual language, shared primitives, data behavior, downloads, persistence, and provider workflows must remain functionally unchanged unless a separate requirement below explicitly says otherwise.

## Non-goals

- NG-1: Rebranding LinkVault or redesigning every page.
- NG-2: Making the Tauri desktop window mobile-first.
- NG-3: Moving DOM measurement, pointer handling, focus handling, responsive CSS, or visual state into Rust.
- NG-4: Rewriting all existing frontend IPC calls in one PR.
- NG-5: Migrating unrelated legacy business logic only because it currently lives in `App.tsx` or a provider view. If touched by this implementation, it must follow the ownership rules in this PRD; otherwise it should be captured as follow-up architecture debt.
- NG-6: Lowering the native 1280x720 minimum without measured evidence and a separate approval.
- NG-7: Fixing overflow by globally hiding it while leaving inaccessible controls off-screen.

## Functional requirements

### FR-1: Brand geometry is independent of sidebar width

- The wordmark MUST have a design-token-owned maximum rendered inline size.
- Its rendered width MUST NOT be derived from `100%` of the sidebar's current width.
- The implementation SHOULD use the rendered brand size at the current default 220 px sidebar as the baseline unless design review deliberately selects a different fixed size.
- The brand container MAY have flexible horizontal space, but the image itself MUST preserve a stable size and aspect ratio.
- The fixed token must work in light and dark themes and at supported display scaling.

### FR-2: Sidebar resizing has a single live source of truth

- Sidebar drag MUST use Pointer Events rather than a mouse-only listener.
- The resize handle MUST capture the active pointer so dragging remains coherent when the pointer moves quickly.
- There MUST be one authoritative live width during a drag.
- React rerenders caused by provider progress, timers, toasts, settings updates, or other state MUST NOT overwrite the in-progress width.
- The final committed preferred width MUST be clamped to the supported range.
- Drag cleanup MUST occur on pointer up, pointer cancel, component unmount, and loss of capture.
- Text selection and cursor state MUST always be restored after cancellation or unmount.

### FR-3: Sidebar resize accessibility

- The resize affordance MUST expose separator semantics and vertical orientation.
- It MUST expose current, minimum, and maximum values to assistive technology.
- Left/Right Arrow MUST resize by a small deterministic step.
- Shift+Left/Right Arrow SHOULD resize by a larger deterministic step.
- Home/End SHOULD move to minimum/maximum width.
- Keyboard resizing MUST use the same clamp and persistence path as pointer resizing.

### FR-4: Preferred width and effective width are distinct concepts

- Persist the user's preferred expanded sidebar width as presentation-only UI state.
- On startup, malformed, non-finite, negative, or extremely large stored values MUST be sanitized.
- If the current window cannot safely render the preferred width, the shell MAY temporarily use a smaller effective width or collapsed presentation without destroying the user's stored preference.
- When space becomes available again, the preferred width SHOULD be recoverable without requiring the user to resize it again.
- Sidebar width/collapse/theme remain frontend presentation preferences; they do not need Rust persistence merely because a Rust backend exists.

### FR-5: Main-shell sizing contract

- `.lv-shell`, `.lv-sidebar`, `.lv-main`, `.lv-content`, and each provider root MUST have explicit `min-width: 0` / `min-height: 0` behavior where required by Grid/Flex sizing.
- The shell MUST have exactly one horizontal-space allocation contract.
- The main workspace MUST never become wider than its grid track because of an intrinsic child minimum.
- Fixed secondary rails MUST either shrink, stack, collapse, or switch presentation based on available container width; they MUST NOT silently starve the primary workspace.
- A page MUST own its intentional scrolling surface. Accidental nested horizontal document scroll is forbidden.

### FR-6: Container-responsive provider layouts

- Layout decisions that compete with the sidebar MUST be based on the provider/workspace container's inline size, not only `window.innerWidth` / viewport media queries.
- Use CSS container queries or an equivalently local layout mechanism for LinkedIn, Coursera, Newspaper, history, clipping, and shell sub-layouts that need to change column count.
- Existing viewport media queries MAY remain for concerns that genuinely depend on the whole viewport, such as full-window overlays or global motion preferences.
- Existing fixed `minmax(...)` lower bounds MUST be reviewed. A minimum may remain only if the surrounding layout has a defined fallback before the minimum can overflow its owner.
- Responsive changes MUST preserve logical reading/tab order; CSS placement must not create a visually reordered keyboard sequence.

### FR-7: 1280 px native-floor behavior

At a 1280x720 native-sized viewport:

- Expanded sidebar at 208 px, default 220 px, and maximum 320 px MUST all produce valid layouts.
- LinkedIn Downloads, LinkedIn History, Coursera Downloads, Coursera History, Newspaper Download, Newspaper Library, Newspaper Clippings, and settings/dialog surfaces that are reachable in preview tests MUST not cause document-level horizontal overflow.
- Primary actions MUST remain visible and interactable.
- Text may truncate only where the design explicitly permits truncation; action buttons and required form controls MUST not be clipped away.
- The 980/520 CSS paths are not substitutes for this acceptance criterion.

### FR-8: Resize performance is correctness-preserving

- The existing `data-window-resizing` optimization may continue to disable expensive transitions/effects during rapid resize.
- Correct geometry MUST NOT depend on the 140 ms settled timer.
- Sidebar drag writes SHOULD be at most once per animation frame.
- A resize optimization MUST NOT leave stale layout attributes or body styles if the component unmounts or the drag is cancelled.
- Expensive page surfaces SHOULD continue to use containment where it is safe, but containment MUST NOT clip overlays that are expected to escape their owner.

### FR-9: No explicit `any` in project-owned TypeScript

- Add `npm run verify:no-any` (name may vary only with reviewer approval).
- The gate MUST use the installed TypeScript compiler API to parse project-owned `.ts` and `.tsx` source and fail on `SyntaxKind.AnyKeyword`; a regex-only implementation is not sufficient.
- The scan MUST cover `apps/desktop/src` and any project-owned TypeScript added for its verification harnesses where practical.
- It MUST catch `any`, `any[]`, `Array<any>`, `Record<string, any>`, `as any`, generic parameters using `any`, and nested occurrences because all contain an `AnyKeyword` AST node.
- It MUST report file and line/column for every violation.
- External library declaration files and `node_modules` MUST NOT be scanned.
- New boundary data from Tauri, browser APIs, JSON, or harnesses MUST use exact DTOs or `unknown` plus narrowing.
- `npm run build` with `tsc` remains required; the no-any gate supplements rather than replaces TypeScript strictness.

### FR-10: Frontend/Rust ownership boundary

Frontend MAY own:

- DOM and CSS geometry.
- Pointer/keyboard resize interaction.
- Focus and accessibility presentation.
- Theme and purely visual preferences.
- Transient open/closed/selected UI state.
- Formatting data that is already authoritative and typed for display.
- Typed IPC adapters that marshal inputs and outputs without redefining domain decisions.

Rust MUST own:

- Filesystem access and path safety.
- Database reads/writes, migrations, and durable recovery.
- Credentials and security decisions.
- Network/provider behavior.
- Download, retry, pause, cancellation, queue, scheduling, and resource-governor rules.
- Domain validation and normalization whose result changes what work is accepted or executed.
- Durable provider/application state transitions.
- Calculations that determine scheduling, persistence, download behavior, or provider semantics.

The responsive implementation MUST NOT move any Rust-owned concern into React to make testing easier.

### FR-11: Do not expand frontend domain debt

- New business rules MUST NOT be added to `App.tsx`, provider view JSX files, CSS helpers, or resize hooks.
- Existing frontend calculations such as scheduling normalization must not be copied or expanded as part of layout work.
- If this PR needs to modify a frontend domain calculation, the default action is to move the authoritative calculation to the relevant Rust owner and return a typed result over IPC.
- Browser-preview fixtures MAY emulate backend responses for UI testing, but the emulation MUST be clearly marked test/preview behavior and MUST NOT become the production authority.
- Direct `invoke(...)` calls MUST NOT proliferate. New IPC for this work, if any, belongs behind a typed adapter. Layout-only changes should require no new backend command.

### FR-12: Component extraction and ownership

- `App.tsx` MUST NOT grow additional resize-specific complexity.
- Sidebar resize behavior SHOULD be extracted into a focused typed hook/component or shell module.
- Pure layout math, if JavaScript is required, MUST be a pure typed function with unit-testable inputs/outputs.
- Provider components must consume shared shell behavior rather than each implementing their own window-size listeners.
- Shared primitives remain the default for controls; page CSS owns layout, consistent with `components/README.md`.

## Adversarial failure model

Implementation and review MUST actively try to break the layout with the following cases rather than testing only the happy path.

### A-1: React rerender races during sidebar drag

While the pointer is continuously moving the sidebar, trigger state updates such as progress events, timers, toast state, theme state, or provider refreshes. The sidebar MUST not jump to the last committed React width and then back to the pointer width.

### A-2: Window resize while sidebar drag is active

Begin dragging the sidebar, resize the browser/native window, continue dragging, then release. Width clamping, cursor cleanup, persistence, and geometry MUST remain coherent.

### A-3: Rapid min/max sweeps

Repeatedly drag 208 -> 320 -> 208 -> 320 without pauses. No element may accumulate stale inline styles, duplicate listeners, or incorrect persisted width.

### A-4: Lost pointer/cancel

Start a drag and cause pointer cancellation or component teardown. Body selection/cursor state and pointer capture MUST be released.

### A-5: Corrupted persisted state

Test missing, `NaN`, `Infinity`, negative, zero, fractional, and extremely large stored sidebar values. Startup MUST converge to a valid visual state.

### A-6: Breakpoint-edge sweeps

Sweep widths one pixel around every retained layout threshold, including existing 1200/1300-era boundaries if they remain. No single-pixel change may produce overlapping cards, inaccessible controls, or horizontal document overflow.

### A-7: Maximum sidebar at minimum native window

At 1280x720 with a 320 px sidebar, exercise every provider root. This is a release-blocking case.

### A-8: Long intrinsic content

Use long course titles, long filesystem paths, long status/error strings, and large numeric values. Text must wrap/truncate at intentional owners without widening the shell.

### A-9: Scrollbar appearance

Populate queues/libraries enough to add vertical scrollbars, then remove data. The appearance/disappearance of a scrollbar MUST not make adjacent controls overlap or shift outside their surface.

### A-10: Dialog/popover during resize

Open settings, tooltips, and sidebar popovers before and during window resize. Portaled overlays must stay viewport-clamped and non-portaled content must not be clipped by new containment rules.

### A-11: Theme and motion settings

Exercise dark/light theme and reduced-motion preferences. Disabling transitions must not alter final geometry.

### A-12: Display scaling/browser zoom

Exercise practical zoom/scaling values where the browser harness supports them. Brand size and control containment must remain stable in CSS pixels and usable to the user.

## Test and verification plan

### TV-1: Extend Playwright visual geometry verification

Extend `scripts/verify-visual.mjs` or split a focused `verify-responsive-layout.mjs` that:

- checks 1280x720, 1366x768, 1400x720, 1600x900, 1720x960, and 1920x1080;
- checks sidebar widths 208, 220, and 320;
- performs actual pointer-driven sidebar drags rather than only setting CSS values;
- records the brand image bounding box at each sidebar width and requires width/height variation of at most 1 CSS pixel after the fixed brand token is established;
- asserts `document.documentElement.scrollWidth <= clientWidth + 1`;
- asserts every critical provider root is within `.lv-main` bounds;
- asserts primary action controls have non-zero visible intersection with their owning surface;
- checks LinkedIn and Coursera as well as existing Newspaper geometry;
- checks collapse/reopen after resizing;
- runs a drag while forcing at least one unrelated React state update;
- repeats the width sweep in both directions to catch state hysteresis.

### TV-2: Unit-test layout math

If effective-sidebar sizing or breakpoint choice requires TypeScript logic, test pure functions for boundary values, malformed persisted input, and preferred/effective width restoration. Prefer CSS for layout when a JavaScript calculation is not necessary.

### TV-3: No-any gate

Run the AST no-any verifier before and after implementation. The first run establishes the real repository baseline. Any existing explicit `any` discovered by the authoritative AST scan is in scope to remove before the gate is enabled; do not grandfather it with an allowlist unless a reviewer documents a generated-code exception.

### TV-4: Existing gates

The completed implementation MUST pass at minimum:

```text
npm run build
npm run verify:no-any
npm run verify:architecture
npm run verify:ui
npm run verify:visual
```

If Rust code is changed, additionally run the repository's relevant Rust formatting/check/test gates. Existing persistence and release gates must not regress.

### TV-5: Native smoke

Browser Playwright can prove DOM geometry, but the implementation MUST also smoke-test the packaged/dev Tauri window at its native 1280x720 floor and verify that the OS window minimum and webview geometry agree.

## Acceptance criteria

### AC-1: Logo does not scale with the rail

Given an expanded sidebar

When its width changes from 208 to 220 to 320 px

Then the LinkVault wordmark's rendered width and height remain invariant within 1 CSS pixel

And its aspect ratio is preserved

And it stays fully contained in the brand region.

### AC-2: Drag survives unrelated rerenders

Given a sidebar drag is in progress

When an unrelated React/Tauri state update causes the shell to rerender

Then the sidebar does not snap to a stale committed width

And the final width matches the pointer-derived clamped value.

### AC-3: Real native floor is valid

Given a 1280x720 viewport

When the sidebar is set to its maximum expanded width

Then every supported provider view remains usable

And the document has no horizontal overflow

And primary actions remain visible inside their owning surfaces.

### AC-4: Provider layouts respond to their container

Given the same viewport width

When the sidebar changes available workspace width enough to cross a provider layout threshold

Then that provider reflows according to its container width

And does not wait for an unrelated viewport media-query threshold.

### AC-5: Persistence is resilient

Given any malformed stored sidebar value from the adversarial set

When the application launches

Then the effective width is valid and finite

And the app remains usable

And a temporary lack of space does not unnecessarily destroy a valid user preference.

### AC-6: Keyboard resize parity

Given focus is on the sidebar separator

When the user uses the supported resize keys

Then the same clamp, layout, accessibility value, and persistence rules used by pointer resize apply.

### AC-7: No explicit TypeScript any

Given project-owned TypeScript/TSX

When `npm run verify:no-any` runs

Then the TypeScript AST contains zero explicit `AnyKeyword` nodes in the governed source set

And the command fails with file/line diagnostics if one is introduced.

### AC-8: Rust remains domain authority

Given the responsive-layout implementation diff

When new or modified logic is classified by ownership

Then DOM/layout/presentation behavior is implemented in frontend code

And no scheduling, persistence, filesystem, security, provider, queue, or durable domain rule is newly implemented in TypeScript.

### AC-9: Existing behavior remains intact

Given all current provider workflows

When the responsive hardening is complete

Then download, history, queue, clipping, reader, settings, persistence, updater, and provider behavior remain functionally unchanged except for corrected layout/interaction behavior.

### AC-10: Regression tests fail for the original bug

Given the original `width: 100%` brand behavior or a reintroduced dual-writer drag race

When the new responsive verification runs

Then at least one deterministic test fails. A test suite that still passes the original failure mode does not satisfy this PRD.

## Implementation workstreams

### W-1: Shell and brand ownership

Likely touched areas:

- `apps/desktop/src/App.tsx`
- a new focused shell/sidebar component or hook under `apps/desktop/src/components/` or `src/lib/`
- `apps/desktop/src/index.css`

Deliver fixed brand geometry, pointer/keyboard resizing, and one live width owner.

### W-2: Container-responsive page layout

Audit provider roots and nested grids for intrinsic-width pressure. Convert only the layout decisions that depend on available workspace width to local container-responsive behavior. Avoid a blind rewrite of all media queries.

Priority order:

1. shared shell and `.lv-content`;
2. LinkedIn dispatch/history;
3. Coursera dispatch/history;
4. Newspaper dispatch/library/clippings;
5. settings and shared overlays;
6. tables, long-path rows, and other intrinsic-content edge cases.

### W-3: Type safety gate

Add the TypeScript-AST no-any verifier and wire it into a standard verification path. Establish and fix the baseline before relying on the gate.

### W-4: Architecture boundary reinforcement

Document the frontend/Rust ownership rule next to the existing architecture checks and add only enforcement that can pass without hiding existing debt. New responsive code should not introduce direct provider/domain logic. If touched code exposes an existing domain rule in TypeScript, move that rule to the appropriate Rust owner or explicitly split it into a follow-up rather than duplicating it.

### W-5: Adversarial geometry verification

Extend Playwright from its current Newspaper-focused sweep into a shell-wide/provider-wide resize contract, including real sidebar drag and rerender-race coverage.

## Review checklist

- [ ] The wordmark no longer scales with sidebar width.
- [ ] Live sidebar width has exactly one owner.
- [ ] Resize uses pointer capture and has cancellation cleanup.
- [ ] Keyboard resize works and exposes separator values.
- [ ] 1280x720 + 320 px sidebar passes every provider geometry test.
- [ ] Provider column changes are based on available container width where appropriate.
- [ ] No important control is merely hidden to make overflow assertions pass.
- [ ] Existing Newspaper resize tests still pass.
- [ ] LinkedIn and Coursera receive equivalent resize coverage.
- [ ] An unrelated rerender is forced during a sidebar drag test.
- [ ] Corrupt local storage cases are tested.
- [ ] `verify:no-any` is AST-based and reports source locations.
- [ ] TypeScript has zero explicit `any` in the governed source set.
- [ ] New frontend code contains no provider/domain authority that belongs in Rust.
- [ ] No unnecessary Tauri command was introduced for visual resizing.
- [ ] All applicable frontend, architecture, visual, Rust, and release gates pass.

## Rollback policy

- Layout changes should be separable from backend/domain changes so they can be reverted without touching persisted data.
- No database migration is expected for this PRD.
- If a container-query conversion regresses a provider surface, revert that provider's layout change while retaining independently verified shell, no-any, and test-harness improvements.
- If a new native window constraint is proposed after measurement, it must be isolated and justified; do not silently raise the 1280x720 minimum to hide an overflow bug.

## Open implementation decisions

These are intentionally not guessed in the PRD and must be resolved with measured evidence during implementation:

1. The exact fixed wordmark inline-size token. Use the current default-sidebar rendering as the first baseline measurement.
2. Exact container thresholds for each provider. Derive them from the minimum width at which real controls remain usable, not from the old viewport numbers by habit.
3. Whether the sidebar's effective width must shrink automatically near a future smaller native window size. At the current 1280 px native floor this may be unnecessary if provider containers reflow correctly.
4. Whether the no-any verifier is a standalone script or incorporated into a broader type-safety verifier. The AST requirement and zero-any outcome are non-negotiable.
5. Which existing frontend domain calculations should be migrated to Rust in this implementation versus recorded as follow-up debt. Any calculation touched by the responsive work must obey FR-10/FR-11.
