# ClientSense Frontend Shell Design

This document describes the current ClientSense desktop frontend shell as implemented in the
worktree. It is meant to be reusable as a design blueprint for future projects that need the same
Jan-style application shell, spacing, overlays, guarded actions, and interaction relationships.

## Source Map

- App root: `apps/desktop/src/App.tsx`
- Shell layout and sidebar: `apps/desktop/src/JanHostShell.tsx`
- Home, project routes, composer, seller workspace: `apps/desktop/src/SellerDashboardSurface.tsx`
- Search, project dialogs, Settings, Hub: `apps/desktop/src/ClientSenseDialogs.tsx`
- Global shell CSS: `apps/desktop/src/App.css`
- Theme persistence and runtime variables: `apps/desktop/src/theme.tsx`
- Route and action contracts: `apps/desktop/src/extension-shell.ts`
- Shared UI primitives: `packages/ui/src/*.tsx`

## Documentation Standard

This file should be treated as a code-aligned design-system document, not a static style guide.
Future projects should be able to implement the shell from this file and then verify against the
source files listed above.

Best-practice rules:

- Keep design decisions tied to source files, classes, tokens, or component props.
- Separate primitive values from semantic meaning. Raw colors and dimensions are not enough; the
  document must explain when and why each value is used.
- Document interaction behavior with the visual spec. A button spec is incomplete unless it states
  what the button does, what opens, what closes, and what state changes.
- Document accessibility behavior with the same seriousness as spacing and color.
- Prefer reusable component patterns over one-off pages. Each future project should copy the shell
  structure, then swap product-specific surfaces and route data.
- When implementation changes, update this file in the same change set.

Minimum sections required for future revisions:

- Source map
- Token hierarchy
- Layout blueprint
- Shared primitive specs
- Component usage and states
- Overlay and popup placement
- Trigger matrix
- Accessibility checklist
- Governance and drift policy
- Reuse checklist

## Design Intent

ClientSense uses a quiet desktop productivity shell. The layout is dense, low-decoration, and built
around repeat-use controls rather than marketing composition.

Primary design qualities:

- Full-height app shell with a persistent left sidebar and a single flexible content region.
- Neutral card surfaces, thin borders, and small radius controls.
- Round icon buttons for utility controls, pill buttons for primary actions, and compact rows for
navigation.
- Command-center composer as the primary first-screen affordance.
- Project workspace mode uses the same composer but adds route panels below it.
- Settings and Hub are utility surfaces that replace the main content padding with a flush
two-pane interface.
- Most destructive, network, provider, download, and file actions are intentionally guarded with
toasts until backing stores are connected.

## Runtime Structure

Top-level render order:

1. `ClientSenseThemeProvider`
2. `JanHostShell`
3. `SellerDashboardSurface` as the normal main content
4. `ClientSenseToaster`

The shell owns route state and utility-surface state:

- `activeRoute`: one of `command-center`, `buyer-inbox`, `order-board`, `buyer-profile`,
  `sync-center`.
- `activeSurface`: `null`, `settings`, or `hub`.
- A route change always clears `activeSurface`.
- A utility-surface change keeps route state in memory but replaces visible main content.
- Project lock state is independent and only affects seller project fixture visibility.

Mount point attributes are part of the design contract:

- Sidebar: `data-mount-point="jan.leftSidebar"`
- Home main: `data-mount-point="jan.homeMain"`
- Thread/project detail: `data-mount-point="jan.threadDetailPanel"`
- Settings route: `data-mount-point="jan.settingsTab"`
- ClientSense dashboard panel: `data-mount-point="clientsense.dashboardMain"`

## Global Tokens

Typography:

- Body font: `Inter`.
- Display and section heading font: `StudioFeixenSans`, then `Inter`.
- Body starts from `--font-size-base`; CSS declares `14px`, but theme runtime default applies `16px`
  unless stored settings override it.
- Supported interface font sizes: `14px`, `16px`, `18px`, `20px`.
- Letter spacing is generally `0`; only small uppercase menu labels use `0.04em`.

Core text scale:

- `--text-xs`: `0.75 * --font-size-base`
- `--text-sm`: `0.875 * --font-size-base`
- `--text-base`: `--font-size-base`
- `--text-lg`: `1.125 * --font-size-base`
- `--text-xl`: `1.25 * --font-size-base`
- `--text-2xl`: `1.5 * --font-size-base`
- `--text-3xl`: `1.875 * --font-size-base`
- `--text-4xl`: `2.25 * --font-size-base`

Color system:

- Main background: `--background`
- Main text: `--foreground`
- Card and popover surfaces: `--card`, `--popover`
- Quiet surfaces: `--secondary`, `--muted`
- Borders and inputs: `--border`, `--input`
- Accent ring: `--ring`
- Primary action color: `--primary`
- Sidebar uses `--sidebar`, `--sidebar-foreground`, `--sidebar-accent`,
  `--sidebar-accent-foreground`, and `--sidebar-border`.

Accent colors:

- Accent options are Gray, Red, Orange, Green, Emerald, Teal, Cyan, Blue, Purple, Pink, and Rose.
- Each accent changes `--primary` and `--sidebar`.
- Gray is the default accent and uses primary `#f17455`.

Theme modes:

- `auto`: follows OS dark mode and Tauri `theme-changed` events.
- `light`: forces light variables.
- `dark`: toggles `.dark` on `document.documentElement`.
- Theme is stored in `localStorage` key `theme`.
- Interface settings are stored in `localStorage` key `setting-appearance`.

Global behavior:

- Body is `overflow: hidden`; the main content region manages scrolling.
- Global lucide icons default to `14px` square.
- Text inputs force caret color to current foreground.
- Focus rings use the ring color at `50%` opacity, usually a 3px ring for shared primitives.

## Token Hierarchy

Use three token layers when moving this shell into future projects.

Raw/global tokens:

- Purpose: store base values such as color, font family, font size, radius, stroke, spacing, and
  animation timing.
- Current examples: `--background`, `--foreground`, `--primary`, `--border`, `--radius`,
  `--font-size-base`.
- Rule: use raw values only inside the theme layer or when defining the first semantic layer.

Semantic/alias tokens:

- Purpose: describe UI role rather than raw appearance.
- Current examples: `--card`, `--popover`, `--muted-foreground`, `--sidebar`,
  `--sidebar-accent`, `--input`, `--ring`.
- Rule: most app CSS should use semantic tokens, not hardcoded hex, oklch, or px values.
- Future project rule: create aliases for product-specific surfaces before adding new component
  CSS. Example: define `--shell-panel`, `--shell-panel-border`, or `--shell-danger-surface`
  before repeating raw values across routes.

Component tokens:

- Purpose: capture repeatable component-level decisions.
- Current component values are mostly class-based rather than named custom properties.
- Recommended next layer:
  - `--shell-sidebar-width: 15rem`
  - `--shell-header-height: 60px`
  - `--shell-content-padding: 2rem`
  - `--shell-composer-radius: 24px`
  - `--shell-composer-min-height: 132px`
  - `--shell-popover-max-width: calc(100vw - 16px)`
  - `--shell-row-gap: 8px`
  - `--shell-panel-radius: 8px`
  - `--shell-project-card-radius: 12px`
- Rule: if a value appears in three or more shell components, promote it to a component token.

Token source-of-truth:

- Runtime theme state lives in `apps/desktop/src/theme.tsx`.
- Current CSS variables live in `apps/desktop/src/App.css`.
- Shared primitive styles live in `packages/ui/src`.
- `packages/design-tokens/src/index.ts` is currently a placeholder; future work should move stable
  cross-package tokens there only after names and semantics are clear.

## Layout Blueprint

Application shell:

- `.jan-host-shell` is a flex container sized to `100svh` width and height.
- Shell background and foreground come from theme variables.
- There is no page padding on the outer shell.
- The sidebar provider also spans `100svh`.

Desktop sidebar:

- Default width: `15rem`.
- Mobile sheet width: `18rem`.
- Icon-collapse width: `5rem`.
- Resizable rail clamp: minimum `14rem`, maximum `20rem`.
- Desktop sidebar is `fixed` inside a peer wrapper.
- Floating sidebar variant adds `p-2 pr-0`, a rounded-xl inner panel, border, and shadow.

Window drag and controls:

- Drag region is fixed at top, height `3rem`, left offset `15rem`, right `0`, z-index `20`.
- Drag region cursor is `grab`, changing to `grabbing` when active.
- Window controls render only in Tauri on Windows.
- Window control group is absolute, right `1rem`, top `0`, height `3.75rem`, z-index `50`.
- Minimize, maximize, and close use shared ghost icon buttons sized `icon-sm`.

Main content:

- `.jan-content-inset` is a grid, fills `100svh`, scrolls vertically, and has `2rem` padding.
- Standard content gap is `16px`.
- `scrollbar-gutter: stable` prevents layout shift.
- When `data-utility-surface` is present, padding becomes `0`, gap becomes `0`, overflow is hidden,
  and utility content manages its own scroll.

Responsive changes:

- At `max-width: 760px`, the host shell becomes block layout and content padding becomes `1.5rem`.
- At `max-width: 1100px` or `max-height: 760px`, content padding also becomes `1.5rem`.
- At `max-width: 900px`, Settings changes from horizontal split to vertical stack.

## Shared Primitive Defaults

Button:

- Base layout: inline-flex, centered, gap `0.5rem`, `text-sm`, `font-medium`, no wrapping.
- Disabled state: pointer-events none, opacity `50%`.
- Focus state: border/ring highlight with `3px` ring.
- Default variant: primary background, pill radius, hover primary at `90%`.
- Destructive: destructive background, pill radius.
- Outline: border, pill radius, background surface, subtle shadow.
- Secondary: secondary background, pill radius.
- Ghost: pill radius, hover accent background.
- Default size: height `36px`, horizontal padding `16px`.
- Small size: height `32px`, horizontal padding `12px`.
- Extra small size: height `24px`, horizontal padding `8px`, text-xs.
- Icon sizes: `36px`, `24px`, `32px`, and `40px` square for icon, icon-xs, icon-sm, icon-lg.

Input:

- Height `36px`, rounded-md, border `--input`, white background in light mode.
- Horizontal padding `12px`, vertical padding `4px`.
- Text is base on mobile and sm on desktop.

Textarea:

- Minimum height `64px`.
- Rounded-md, border `--input`, padding `8px 12px`.
- Uses field-sizing content and wraps anywhere.

Switch:

- Track is `34px` wide by `18px` high.
- Thumb is `16px` square.
- Checked track uses primary color.
- Disabled opacity is `50%`.

Dropdown menu:

- Portal content z-index `50`.
- Minimum width `8rem`.
- Max width `calc(100vw - 1rem)`.
- Padding `4px`, rounded-md, border, shadow.
- Default side offset `4px`; collision padding `8px`.
- Menu item padding `6px 8px`, gap `8px`, rounded-sm, text-sm.

Popover:

- Portal content z-index `50`.
- Default width `18rem`.
- Padding `16px`, rounded-md, border, shadow.
- Default side offset `4px`.
- Uses side-aware slide and zoom animations.

Dialog:

- Overlay is fixed inset `0`, z-index `50`, black at `50%`, with backdrop blur.
- Content is fixed center at `50%/50%`, z-index `50`.
- Content max width is `calc(100% - 2rem)` by default, `sm:max-w-lg`, `lg:max-w-2xl`,
  `xl:max-w-3xl`.
- Max height is `85vh`; content scrolls vertically.
- Default padding `24px`, gap `16px`, rounded-lg, border, shadow.
- Close button is absolute top `16px`, right `16px`, small square icon.

Sheet:

- Uses dialog overlay semantics with z-index `50`, black `50%`, backdrop blur.
- Right/left sheets are `75%` width with `sm:max-w-sm`.
- Top/bottom sheets have auto height and border on the attached edge.

Drawer:

- Mobile drawer overlay is fixed inset `0`, z-index `50`, black `50%`.
- Bottom drawer max height `80vh`, rounded top-lg, border-top, and a visible grip.
- Top drawer max height `80vh`, rounded bottom-lg, border-bottom.

DropDrawer:

- Desktop renders as dropdown menu.
- Mobile below `768px` renders as drawer.
- Desktop content defaults to align `end`, side offset `4px`, min width `220px`.
- Mobile submenu navigation uses a stack with forward/back animation and a header back button.

Tooltip:

- Delay duration is `0`.
- Content is foreground background with inverse text.
- Padding `6px 12px`, text-xs, rounded-md.
- Default arrow is visible.

## Component Usage and States

Use this section when deciding whether to reuse an existing primitive, compose a local shell
component, or add a new component.

Button:

| Variant | Use When | Avoid When | States To Preserve |
| --- | --- | --- | --- |
| `default` | Primary action inside a compact row, footer, or toolbar. | Passive navigation or destructive actions. | Hover primary `90%`, disabled opacity `50%`, 3px focus ring. |
| `outline` | Secondary action, guarded setup, or neutral row action. | Main navigation rows; use sidebar buttons instead. | Border visible, hover accent background, disabled opacity. |
| `secondary` | Low-risk icon tool, especially composer toolbar buttons. | A route-changing primary action. | Pill radius, compact icon sizing, muted disabled state. |
| `ghost` | Icon-only chrome, overflow menus, cancel actions. | A primary save or submit action. | No background until hover/focus. |
| `destructive` | Confirmed destructive actions inside menus/dialogs. | Guarded destructive previews that do not execute. | Destructive color, focus ring, clear label. |

Sidebar row:

| Element | Use When | Required Behavior |
| --- | --- | --- |
| Main action row | Top-level shell action. | 32px row, icon left, optional shortcut right, animated icon on hover. |
| Project parent row | Expandable workspace group. | Toggle expanded state only; route selection belongs to subnav or menu. |
| Project subnav row | Route within workspace. | Prevent default anchor behavior; route only if allowed by extension capability. |
| Sidebar overflow action | Row-scoped secondary action. | Show on hover/focus, open menu to the right on desktop. |

Composer controls:

| Control | Use When | Required Behavior |
| --- | --- | --- |
| Textarea | Primary prompt input. | Enter submits, Shift+Enter inserts newline, IME composition must not submit. |
| Attachment plus | File/image attachment entry. | Open DropDrawer; guard unavailable storage with toast. |
| Seller tools | Project-only tool list. | Visible only in seller workspace; guarded tools stay disabled or warning-only. |
| Send | Submit draft. | Disabled for empty trim; preserve draft when provider is missing. |

Overlay primitives:

| Primitive | Use When | Placement Rule |
| --- | --- | --- |
| Dialog | Global command or confirmation requiring focus trap. | Centered; blurred black overlay; max height `85vh`. |
| Dropdown menu | Small anchored local actions. | Align to triggering control; collision padding `8px`. |
| Popover | Anchored selector or richer lightweight panel. | Side/align must be explicit for reusable shell controls. |
| DropDrawer | Menu that must become a mobile drawer. | Desktop dropdown, mobile drawer below `768px`. |
| Hover card | Supplemental read-only details. | Delay open; never contain required actions. |
| Toast | Brief status or guarded-action feedback. | Do not use as the only record of a destructive success. |

State coverage every component should document:

- Default
- Hover
- Focus-visible
- Active or selected
- Disabled
- Loading or pending, when async
- Empty
- Overflow/truncation
- Mobile behavior
- Guarded/unavailable behavior, when side effects are intentionally blocked

## Sidebar Design

Header:

- `.jan-sidebar-header-shell` gap `8px`, padding `10px 8px 6px`.
- Topbar height `30px`, padding `0 6px`, gap `8px`.
- Brand text is `14px`, weight `600`.
- Topbar action group gap is `2px`.

Download manager trigger:

- Size `28px` square, pill radius.
- Color is muted foreground.
- Hover and focus use sidebar foreground at `8%` mixed into transparent.
- Click also shows a guarded-download toast.
- Popover opens below the trigger, aligned start, side offset `6px`.
- Popover min width `240px`, max width `calc(100vw - 16px)`, no padding.
- Empty state padding `12px`, grid gap `6px`, label text `12px`, title `13px` weight `500`,
  paragraph `12px` with `18px` line height.

Collapse trigger:

- Size `28px` square, pill radius, transparent background.
- Hover uses the same sidebar foreground `8%` wash.
- Click calls the shared sidebar toggle.
- Keyboard shortcut from the shared sidebar provider is Ctrl/Cmd+B.

Main action rows:

- Actions are New Chat, New Project, Search, Hub, and Settings.
- Row height comes from shared sidebar menu button default: `32px`.
- Row padding is `8px`, gap `8px`, rounded-md.
- Section gap is reduced to `3px`.
- Icon size is `16px`, opacity `0.7`.
- Shortcut hints are `11px`, weight `500`, muted sidebar foreground, pushed to the right.
- Animated icons start animation on mouse enter and stop on mouse leave.

Main action triggers:

- New Chat: clears active utility surface and routes to `command-center`.
- New Project: opens the project dialog.
- Search: opens the search dialog.
- Hub: sets utility surface to `hub`.
- Settings: sets utility surface to `settings`.
- Keyboard shortcuts implemented in `extension-shell.ts`: Ctrl/Cmd+N for New Chat, Ctrl/Cmd+P for
  New Project, Ctrl/Cmd+F for Search.

Projects group:

- Group label uses shared sidebar label height `32px`, px `8px`, text-xs, medium weight, muted.
- eBay Seller Workspace row toggles the local expanded state only.
- Chevron sits at `margin-left: auto` and rotates `90deg` over `160ms` when open.
- Project subnav margin is `2px 0 2px 22px`, padding-left `10px`.
- Subnav row height is `30px`, font size `12px`.
- Active subnav row uses sidebar accent background and foreground with weight `500`.

Project row overflow menu:

- Trigger is a sidebar menu action, shown on hover/focus, positioned on the right of the row.
- Dropdown opens to the right, aligned start, width `12rem`.
- View Project: expands the project group and routes to `buyer-inbox`.
- Edit Project: opens the edit project dialog.
- Delete Project: opens the delete project dialog.

Lock button:

- Visible only when a project route is active and no utility surface is open.
- Full sidebar footer width, height `34px`, gap `8px`, radius `6px`.
- Unlocked state uses card background and input border.
- Locked state uses foreground background and card-colored text.
- Click toggles project lock state.

Sidebar rail:

- Rail is hidden on small screens and visible from `sm`.
- Width is `16px`, positioned just outside the sidebar edge.
- It supports drag resize and toggling when collapsed.
- Hover paints a narrow half-pixel visual rail.

## Header and Model Picker

Route header:

- Header is `60px` high, flex row, padding `0 16px`.
- Inner row flexes full width with `8px` gap.

Model picker trigger:

- Pill trigger, min height `32px`.
- Border `1px solid --border`, background `--card`.
- Text size `12px`, gap `6px`, padding `6px 16px`.
- Selected model label uses foreground, weight `500`, ellipsis overflow.
- Chevron icon is `14px` square.
- Hover changes background to `--background` and border to `--input`.
- Focus-visible uses `2px` outline with `2px` offset.

Model picker popover:

- Opens below the trigger, aligned start.
- Uses Radix popover placement, side bottom, `avoidCollisions` true when search is empty.
- Background is `--background` at `95%` with `24px` blur.
- Min width is `min(280px, calc(100vw - 16px))`.
- Max width is `min(90vw, calc(100vw - 16px))`.
- Empty-search height is fixed at `320px`.
- Search row has `8px` padding and a bottom border.
- Search input height is `30px`, font size `14px`, no inner border.
- Clear search button is absolute right `8px`, top `0`, bottom `0`, width `24px`, icon `16px`.
- Provider group has margin `6px`, padding `4px 0`, radius `2px`.
- Provider heading is `11px`, weight `600`, padding `5px 8px 3px`.
- Provider group header is `13px`, weight `500`, padding `4px 8px`.
- Provider settings button is `24px` square, radius `2px`, foreground wash background.
- Model menu item min height `38px`, padding `6px 8px`, margin `0 4px 4px`, radius `2px`.
- Selected model item uses primary at `14%` background and an inset primary border at `38%`.

Model picker triggers:

- Clicking a model option sets selected model id and closes the popover.
- Provider settings buttons show a guarded provider-settings toast.
- Search filters by model label and detail.
- Opening focuses the search input after `100ms`.
- Closing clears the search value.

## Home and Project Stage

Home stage:

- `.jan-home-stage` centers content vertically with min height `min(54svh, 520px)`.
- The inner width is `100%`, `80%` at `min-width: 768px`, and `66.6667%` at `min-width: 1280px`.
- In home mode the inner wrapper is shifted upward with `margin-top: -80px`.
- Title block has bottom margin `16px` and centered text.
- Home title is `24px`, StudioFeixenSans, weight `500`, line-height `1.25`, margin `8px 0 0`.

Project stage:

- Project mode sets `data-workspace-open="true"`.
- Stage aligns to top, removes min-height, uses padding `16px 16px 8px`.
- Inner margin-top resets to `0`.
- Project title row is flex, gap `8px`, space-between, margin-bottom `16px`.
- Project title is StudioFeixenSans `24px`, weight `600`, line-height `32px`.
- Project title overflow action is a ghost icon-xs button.

Project route body:

- Stacks panels with `24px` gap.
- Width matches the composer column:
  - `calc(100% - 32px)` by default
  - `calc(80% - 25.6px)` from `768px`
  - `calc(66.6667% - 21.333px)` from `1280px`
- Bottom margin `24px`.

## Composer

Shell:

- Composer shell is positioned relative and full width.
- Frame has radius `24px`, overflow hidden, padding `2px`.
- Composer surface has card background, `1px` input border, radius `24px`, min height `132px`.
- Bottom padding is `40px` to make room for the toolbar.
- Focused composer adds a `1px` ring-like box shadow using ring color at `50%`.

Textarea:

- Transparent background, no border, no shadow.
- Font size `14px`, line-height `1.5`.
- Minimum height `52px`.
- Padding `16px 16px 0`.
- Resize disabled.
- Placeholder uses muted foreground at full opacity.

Toolbar:

- Absolutely positioned at bottom `0`, left `0`, right `0`.
- Padding `8px`, gap `8px`, z-index `2`.
- Left and right tool groups are flex rows with gap `8px`.
- Toolbar icon buttons use `icon-sm` size, pill radius, and `18px` icons.
- Send button uses primary foreground; disabled send button uses muted foreground.

Composer triggers:

- Enter submits when Shift is not held and IME composition is not active.
- Shift+Enter creates a newline.
- Send button is disabled when `composerDraft.trim()` is empty.
- Current provider state is `providerConfigured = false`, so any non-empty submit shows a
  "No provider configured" warning and preserves draft text.
- If provider configuration later becomes true, submit clears the draft.
- Attachment plus opens a DropDrawer aligned start on desktop.
- Add Images calls attachment intent and currently shows "Attachments guarded".
- Add documents or files is disabled while `modelSupportsTools = false`.
- In home mode only, Use Assistant appears as a submenu and selecting an assistant updates local
  selected assistant state.
- In project mode, Seller Tools appears as a right-side toolbar button.

Attachment menu:

- Desktop min width `220px`, max width `calc(100vw - 16px)`.
- Menu item text truncates with ellipsis.
- Menu icons are muted `16px`.
- On mobile, this becomes a drawer via DropDrawer.

Seller tools menu:

- Desktop DropDrawer content opens from the Seller Tools button, aligned end and side top.
- Menu min width `260px`, max width `calc(100vw - 16px)`, overflow hidden.
- Label is `13px`, padding `4px 8px`.
- Server row radius `5px`, margin `2px auto`, padding `8px`.
- Tool count pill is min `18px`, font `11px`, border, radius `4px`, padding `0 5px`.
- Submenu max height `280px`, max width `min(320px, calc(100vw - 16px))`.
- Tool list max height `224px`, padding `4px`, vertical scroll.
- Tool item padding top/bottom `6px`, margin-top `4px`.
- Tool title is `13px`, weight `500`; description is `11px`, line-height `1.35`.
- Current tool toggles are disabled and tool row activation shows "Seller tools guarded".

## Search Dialog

Placement:

- Uses centered dialog content with overlay.
- Custom dialog class removes default close button and uses no internal padding.
- Width is `sm:max-w-xl`.
- Radius is `12px`.

Input row:

- Min height `48px`.
- Border-bottom `1px solid --border`.
- Padding `0 12px`.
- Search icon is `16px`, muted.
- Input height `46px`, font `14px`, padding `0 12px`, no border.

Results:

- Results area is grid, gap `4px`, max height `320px`, vertical scroll.
- Padding `8px 4px`.
- Result button min height `36px`, radius `6px`, gap `8px`, padding `8px 12px`.
- Result font is `14px`.
- Hover and selected state use secondary background.
- Active route uses font weight `600`.
- Result icon is `16px`, muted.
- Result trailing group label is `12px`, muted, pushed to the right and truncated.

Footer:

- Border-top `1px solid --border`.
- Min height `40px`, padding `8px 12px`, gap `12px`.
- Footer text is `11px`.
- Kbd chips use secondary at `55%`, radius `4px`, font `10px`, padding `2px 6px`.

Search triggers:

- Opening resets query, selected index, recent searches, then focuses the input.
- Empty query shows a New Chat result first, followed by up to 5 recent searches.
- Clear recent removes `clientsense.recent-searches` from localStorage.
- Typing filters allowed routes and utility surfaces by label, id, mount point, or keyword.
- ArrowDown and ArrowUp move selected index.
- Enter selects the highlighted result.
- Selecting a route sets route, clears utility surface, stores it in recents, and closes.
- Selecting Hub or Settings sets utility surface, stores it in recents, and closes.
- New Chat clears utility surface, routes to `command-center`, and closes.

## Project Dialogs

Create and edit project dialog:

- Uses centered shared dialog.
- Body grid gap `8px`.
- Label font `13px`, weight `600`.
- Help text font `12px`, line-height `1.5`, muted.
- Name input uses shared input.
- Assistant trigger is a full-width `36px` row, radius `6px`, padding `0 10px`.
- Assistant dropdown min width equals trigger width and max width `calc(100vw - 16px)`.
- Footer actions are Cancel ghost small and Save/Create default small.

Project save behavior:

- Empty name disables save.
- Create with protected project name shows a protected warning.
- Edit protected project to a different name shows a protected warning.
- Unchanged edit disables save.
- Save intent closes the dialog.
- Assistant store action shows "Assistant store not connected".

Delete project dialog:

- Uses centered shared dialog.
- Content width `sm:max-w-md`.
- Delete button is destructive small.
- Protected project delete shows a warning toast and keeps the project.
- Cancel closes dialog.

Add provider dialog:

- Uses centered shared dialog with width `sm:max-w-md`.
- Closing clears input.
- Create requires non-empty name.
- Create calls guarded provider creation, then closes.

## Utility Surface: Settings

Utility route header:

- Utility surfaces set the main inset to padding `0`.
- Top utility header is `60px`, border-bottom, padding `0 16px`.
- Header title uses StudioFeixenSans `16px`, weight `500`.

Settings shell:

- Horizontal split layout fills available height.
- Left menu is `232px` wide, border-right, padding `16px 6px`, gap `2px`, vertical scroll.
- Content pane is grid, gap `12px`, padding `0 16px 20px`, vertical scroll.
- Settings content header is `60px`, StudioFeixenSans `16px`, weight `500`.

Settings menu rows:

- Row height `30px`, radius `4px`, padding `0 8px`, gap `8px`.
- Text is `13px`, weight `500`, truncated.
- Icons are `18px`, muted; active icons become foreground.
- Hover and active rows use secondary background.
- Section labels are `11px`, weight `600`, uppercase, `0.04em`, margin `16px 8px 4px`.
- Experimental badge is pill, `11px`, primary color, margin `-23px 0 4px 96px`, padding `1px 8px`.
- Provider sublabels are `11px`, uppercase, margin `6px 8px 2px`.
- Provider menu rows are `28px` high and muted unless active.

Settings cards:

- Cards have `1px` border, radius `8px`, overflow hidden.
- Card title row padding `14px 16px`, gap `12px`, space-between.
- Card title h2 uses StudioFeixenSans `16px`, weight `500`.
- Card action text is `12px`, muted, gap `8px`, no wrap.
- Form list starts with secondary top border, gap `12px`, padding-top `14px`.
- Form row is grid gap `7px`.
- Form row label is `13px`, weight `500`.
- Form row help text is `12px`, line-height `1.45`, muted.
- Two-column form grids use `repeat(2, minmax(0, 1fr))`.
- Inline actions use grid columns `minmax(0, 1fr) auto`, gap `8px`.
- Action strips use secondary top border, flex-wrap, gap `8px`, padding-top `12px`.

Appearance settings:

- Rows have min height `66px`, padding `14px 16px`, gap `16px`, border-bottom.
- Copy title is StudioFeixenSans `14px`, weight `500`.
- Copy description is `12px`, line-height `1.35`, margin top `4px`.
- Dropdown triggers are min width `170px`, justify space-between.
- Dropdown menus are min width `170px`, aligned end.
- Accent grid flex-wraps with `8px` gap and max width `360px`.
- Accent buttons are `22px` square circles with `1px` border.
- Active accent has double ring: `0 0 0 2px --card`, `0 0 0 4px --primary`.

Settings triggers:

- Menu row click changes local `settingsSection`.
- Add provider buttons open the Add Provider dialog.
- Provider navigation rows set section to `model-providers` and selected provider detail.
- Theme dropdown sets active theme.
- Accent swatches set accent color.
- Font size dropdown sets font size.
- Notification position dropdown sets toast position.
- Reset settings calls `resetInterface` and shows a success toast.
- Most service, provider, connector, hardware, analytics, update, local API, proxy, Claude Code,
  folder, and external-link controls show guarded warning toasts.

## Utility Surface: Hub

Hub shell:

- Hub uses the utility surface wrapper but no settings side menu.
- Toolbar has search plus filters; content is a model list or detail route.
- Model discovery is offline in the current slice.

Hub model cards:

- Card background `--card`, border, radius `8px`, padding `14px 16px`, grid gap `10px`.
- Header is flex, gap `12px`, space-between.
- Model title button is transparent, text-left, StudioFeixenSans `16px`, weight `500`, ellipsis.
- Fit pill is `11px`, radius `4px`, padding `5px 7px`, gap `5px`, icon `12px`.
- Model paragraph is `13px`, line-height `1.5`, muted.
- Metadata row is flex-wrap, `12px`, gap `10px`; icons are `14px`.
- Tags are `11px`, radius `4px`, padding `2px 6px`.
- Variant toggle is transparent, `12px`, weight `500`, gap `6px`.
- Variant list has top border and padding-top `6px`.
- Variant row min height `42px`, grid columns `1fr auto auto`, gap `12px`.

Hub hover card:

- Fit pill opens a hover card after `150ms`.
- Hover card is placed to the left.
- Info card width is `320px`, grid gap `14px`.
- Header has bottom border and padding-bottom `12px`.
- Header title `14px`, weight `600`; subtitle `12px`, muted.
- Info grid gap `14px`; labels `12px`, values `13px`, body text `12px`, line-height `1.45`.

Hub detail route:

- Detail route grid gap `24px`.
- Back button is ghost small, icon `16px`, gap `8px`.
- Detail h1 is StudioFeixenSans `24px`, weight `600`, line-height `1.2`.
- Description is `14px`, line-height `1.55`, max width `780px`.
- Stats row is `13px`, flex-wrap, gap `14px`; icons `15px`.
- Tags are `12px`, weight `500`, padding `6px 8px`.
- Detail sections are grid gap `12px`.
- Tables use fixed layout, min width `620px`, cells `12px 8px`, text `13px`.
- Mobile table mode hides the header and stacks cells.

Hub triggers:

- Search input filters scaffold models by name, developer, description, and tags.
- Sort dropdown changes local sort value.
- Downloaded switch filters downloaded models.
- Model title click opens detail route.
- Back to Hub clears selected model id.
- Download buttons show guarded model-action toast.
- Variant toggle expands or collapses that model row.
- Show all models clears search and downloaded filter.

## Project Workspace Panels

Shared project section:

- Background `--card`, border, radius `12px`, overflow hidden.
- Header min height `56px`, padding `14px 16px`, gap `12px`, border-bottom.
- Header h2 uses StudioFeixenSans `16px`, weight `500`, line-height `24px`.
- Header meta is `12px`, line-height `1.4`, muted, max width `45%`, truncated.
- Body padding `16px`.
- Section row min height `64px`, padding `16px`, gap `16px`.
- Row title is `14px`, weight `500`, line-height `20px`.
- Row text is `14px`, line-height `20px`, muted.

Route panels:

- Buyer Inbox shows thread subject and body preview.
- Order Board shows a fact list for order id, status, and total.
- Buyer Profile shows buyer display name, open orders, and lifetime value.
- Sync Center shows connect account row plus sync facts.
- Locked state adds a project section with safe-state copy above route content.

Fact lists:

- Fact rows are grid `minmax(120px, 0.35fr) 1fr`, min height `36px`.
- Rows use bottom border except last row.
- Labels are `12px`, muted.
- At `max-width: 760px`, fact rows stack into one column with `8px 0` padding.

Project conversation panel:

- Radius `8px`, border, card background, overflow hidden.
- Header min height `48px`, padding `8px 12px 8px 16px`, gap `12px`.
- Title `14px`, weight `500`, line-height `20px`.
- Empty state padding `24px 16px 48px`, centered column.
- Empty icon `32px`, muted at `50%`, margin-bottom `12px`.
- Empty title `16px`, weight `500`, line-height `24px`.
- Empty text `14px`, line-height `20px`, muted.

Project settings card:

- Assistant row has bottom border and a fixed assistant avatar.
- Edit assistant shows guarded warning.
- Files section padding `16px`.
- Files header gap `12px`, margin-bottom `12px`.
- File status row is `12px`, muted, gap `8px`, margin `-4px 0 10px`.
- Empty file drop area min height `132px`, padding `32px 16px`, radius `8px`, dashed border.
- Empty file icon `32px`, muted at `50%`.
- Empty file text `14px`, line-height `20px`.
- Upload and empty file area both show guarded file warning.

## Toasts and Notifications

Toaster:

- Uses `richColors`.
- Visible toasts: `5`.
- Toast background is `--background`.
- Padding is `1rem 0.8rem`.
- Border color is `--border`.
- Text color is `--foreground`.
- Title uses foreground; description uses muted foreground.
- Toasts are non-selectable.

Notification positions:

- Supported values: top-right, top-left, bottom-right, bottom-left.
- Browser default is top-right.
- Tauri Windows default is bottom-right.
- Top positions add a Tauri top safe offset of `48px`.
- Base offset is `8px` from the selected edges.

Guarded toast pattern:

- Guarded actions use `toast.warning(title, { description })`.
- A guarded toast means the control is intentionally visible for parity but the side effect is
  disabled until the relevant Jan store or ClientSense service is connected.

## Accessibility Checklist

Accessibility is part of the shell contract. Future projects should verify these items before
calling the shell reusable.

Keyboard and focus:

- Every interactive control must be reachable by keyboard unless it is intentionally hidden.
- Dialogs and sheets must trap focus while open and return focus to the trigger when closed.
- Dropdowns, popovers, and DropDrawer menus must close on Escape.
- Search dialog input should receive focus immediately on open.
- Model picker search should receive focus after open.
- Focus order should follow visible structure: sidebar, header controls, composer/content,
  route panels, utility content.
- Do not leave focus inside a hidden utility surface after route changes.

Screen reader semantics:

- Icon-only buttons need `aria-label` or visible `sr-only` text.
- Decorative icons must be `aria-hidden="true"`.
- Dialogs require a title; visually hidden titles are acceptable for command dialogs.
- Route panels need clear section labels or headings.
- Disabled controls should communicate disabled or guarded state through label text, aria state,
  or adjacent copy.

Contrast and visual accessibility:

- Standard text should meet WCAG AA contrast, at least 4.5:1.
- Large text and non-text UI indicators should meet at least 3:1.
- Do not rely on color alone for selected, active, guarded, or destructive states.
- Focus-visible indicators must remain visible in light, dark, and accent themes.
- Text truncation must preserve the full meaning elsewhere through title, detail text, or the
  target view.

Responsive and motion:

- At mobile widths, menus that become drawers must remain operable without hover.
- Hover-only affordances need focus-visible equivalents.
- Animated icons should be decorative and should not be required to understand the action.
- Future motion additions should respect reduced-motion preferences.

Acceptance checks:

- Tab through the shell with sidebar expanded and collapsed.
- Open and close every dialog, menu, popover, drawer, and hover card with keyboard where applicable.
- Verify focus returns to the trigger after closing modal surfaces.
- Verify every icon-only button has an accessible name.
- Verify guarded actions announce useful toast text and do not silently fail.

## Overlay Placement Rules

Use these placement rules to reproduce the shell. In this document, popup means any anchored
surface that is not a full modal: dropdown menu, popover, hover card, or desktop DropDrawer.

- Modal search and project dialogs: centered dialog with black `50%` blurred overlay.
- Sidebar project overflow menu: dropdown, side right, align start, width `12rem`.
- Project title overflow menu: dropdown, align end.
- Conversation overflow menu: dropdown, align end.
- Model picker: popover, side bottom, align start.
- Download manager: popover, side bottom, align start, side offset `6px`.
- Composer attachment menu: DropDrawer, desktop dropdown align start, mobile drawer.
- Seller tools menu: DropDrawer, desktop dropdown aligned end from the trigger and placed above.
- Assistant submenu: DropDrawer submenu; desktop flyout, mobile stacked drawer submenu.
- Settings appearance dropdowns: dropdown, align end, min width `170px`.
- Hub sort dropdown: dropdown, align end.
- Hub model fit details: hover card, side left, open delay `150ms`.

All dropdowns and popovers use portal z-index `50`. The app window controls also use z-index `50`,
so top-right overlays must stay collision-aware or avoid the control area.

## Button and Control Trigger Matrix

Window controls:

- Minimize: calls Tauri `appWindow.minimize()`.
- Maximize: calls Tauri `appWindow.toggleMaximize()`.
- Close: calls Tauri `appWindow.close()`.
- These controls do not render in browser preview or non-Windows runtime.

Sidebar and shell:

- Download icon: opens download popover and shows guarded download manager toast.
- Collapse sidebar: toggles sidebar open state; Ctrl/Cmd+B also toggles.
- New Chat: routes to command center and clears utility surface.
- New Project: opens create project dialog.
- Search: opens search dialog; Ctrl/Cmd+F also opens it.
- Hub: opens Hub utility surface.
- Settings: opens Settings utility surface.
- eBay Seller Workspace row: expands/collapses project subnav only.
- Project overflow View Project: expands project subnav and routes to Buyer Inbox.
- Project overflow Edit Project: opens edit project dialog.
- Project overflow Delete Project: opens protected delete dialog.
- Subnav route row: routes to the selected allowed project route.
- Lock eBay / Unlock eBay: toggles lock state.

Composer:

- Model picker trigger: opens model popover.
- Provider settings inside model popover: guarded provider settings toast.
- Model row: selects model, closes popover, clears model search.
- Clear model search: clears query and refocuses search.
- Textarea Enter: submits if no Shift and not composing.
- Textarea Shift+Enter: inserts newline.
- Send: submits non-empty draft; currently guarded by missing provider.
- Plus: opens attachment DropDrawer.
- Add Images: guarded attachment toast.
- Add documents or files: disabled while model does not support tools.
- Use Assistant submenu row: opens assistant submenu in home mode.
- Assistant option: updates selected assistant.
- Seller tools: opens tools DropDrawer in project mode.
- Seller tool row: guarded seller tools toast.

Search dialog:

- New Chat result: routes command center and closes dialog.
- Recent result: navigates to stored route or utility surface.
- Clear recent: removes recent search storage.
- Route result: navigates route, stores recent, closes.
- Utility result: opens Hub or Settings, stores recent, closes.
- Arrow keys: move highlight.
- Enter: activates highlighted result.

Project dialogs:

- Assistant dropdown option: changes assistant label.
- Assistant store option: guarded assistant-store toast.
- Cancel: closes dialog.
- Save/Create: validates project intent and closes only for save intent.
- Delete protected project: guarded protected-project toast.

Settings:

- Settings menu item: switches settings section.
- Add provider: opens Add Provider dialog.
- Provider menu row: switches to model providers and selects provider.
- Theme menu item: sets theme.
- Accent swatch: sets accent color.
- Font size item: sets base font size.
- Notification position item: sets toaster position.
- Reset settings: resets interface settings and shows success toast.
- Local API, attachment, hardware, privacy analytics, connector, proxy, Claude Code, extension,
  provider, update, folder, and external resource actions: guarded warning toasts.

Hub:

- Hub search input: filters visible scaffold models.
- Sort dropdown item: sets sort.
- Downloaded switch: toggles downloaded-only filter.
- Model title: opens model detail.
- Fit unknown pill hover: opens hover card.
- Download button: guarded model-action toast.
- Show/Hide variants: toggles variant list.
- Back to Hub: leaves detail route.
- Show all models: clears Hub filters.

Project workspace:

- Project title more Edit/Delete: guarded protected project toasts.
- Sync Center Connect: invokes `connect_account`; disabled while connecting.
- Conversation menu No conversations / Delete All Threads: guarded conversations toast.
- Assistant Edit: guarded assistant editing toast.
- Files Upload: guarded files toast.
- Empty file area: guarded files toast.

## Governance and Drift Policy

This document should stay close to the code. A design-system doc becomes harmful when it describes
an older shell better than the current one.

Single source rules:

- Use `packages/ui/src` for primitive component defaults.
- Use `apps/desktop/src/App.css` for shell-specific layout and visual rules.
- Use `apps/desktop/src/extension-shell.ts` for route names, mount points, shortcut resolution, and
  intent contracts.
- Use `apps/desktop/src/theme.tsx` for theme persistence, accent palettes, font size options, and
  notification placement.
- Use this `design.md` for explanation, usage rules, acceptance criteria, and implementation order.

Change policy:

- If CSS class dimensions change, update the corresponding measurement in this file.
- If a trigger changes behavior, update the trigger matrix in the same change.
- If a new overlay is introduced, add it to Overlay Placement Rules.
- If a component gains a new state, add it to Component Usage and States.
- If a guarded action becomes live, update both the trigger matrix and accessibility notes.
- If tokens move into `packages/design-tokens`, update Token Hierarchy and Source Map.

Review checklist for pull requests:

- Does the implementation still match documented dimensions and placements?
- Are new hardcoded colors or repeated dimensions candidates for tokens?
- Are keyboard and screen-reader behaviors documented for new controls?
- Are mobile drawer/dropdown differences described?
- Are guarded, loading, empty, and disabled states covered?
- Is text truncation intentional and recoverable?

Future project implementation order:

1. Copy shared primitive package or equivalent Button, Dialog, Dropdown, Popover, Sheet, Drawer,
   Switch, Input, Textarea, Tooltip, and Sidebar primitives.
2. Define theme variables and token aliases.
3. Build the shell layout: outer host, sidebar, route header, content inset.
4. Build utility surfaces with flush content inset behavior.
5. Build composer and model picker.
6. Add overlays and dialogs.
7. Add guarded-action toasts.
8. Add route and shortcut contracts.
9. Verify accessibility checklist.
10. Only then add product-specific panels and data.

## Reuse Checklist

To reuse this shell in another project:

1. Keep the outer `100svh` shell, fixed floating sidebar, and scroll-owned content inset.
2. Preserve the `60px` route/utility headers and `15rem` default sidebar width.
3. Use Inter for body text and StudioFeixenSans for shell headings.
4. Keep buttons compact: 24px icon-xs, 32px icon-sm/small rows, 36px default buttons.
5. Keep primary navigation rows at 32px and subnav rows at 30px.
6. Use centered dialogs for global commands, popovers for selectors, dropdowns for local overflow,
   and DropDrawer for controls that need mobile drawer behavior.
7. Use guarded warning toasts for visible parity actions whose backing store is not connected.
8. Preserve data mount points when implementing extension surfaces.
9. Keep text truncation on sidebar rows, search results, model names, provider rows, and tags.
10. Keep utility surfaces flush inside the content inset; they should manage their own scroll.
