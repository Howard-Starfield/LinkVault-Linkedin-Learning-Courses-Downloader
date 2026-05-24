# Reference Contract

## Required Inputs

Every UI implementation pass must inspect:

- `../design.md`
- `../reference.png`
- Attached Image #1 from the planning conversation, if available in the session

## `design.md` Rules To Preserve

From `../design.md`:

- Use a full-height `100svh` shell.
- Use a persistent sidebar around `15rem`.
- Use a route/header height around `60px`.
- Keep controls compact: default buttons around `36px`, smaller icon controls where appropriate.
- Use neutral card surfaces, thin borders, and small radii.
- Keep page sections unframed unless they are actual tool panels or repeated cards.
- Implement keyboard and screen-reader accessibility for all interactive controls.
- Keep overlay/popover behavior explicit, collision-aware, and Escape-closeable.
- Update design documentation when dimensions, trigger behavior, overlays, or component states change.

## `reference.png` First-Screen Requirements

The first desktop viewport must show:

- Left sidebar with LinkVault brand.
- Active `LinkedIn Courses` route.
- Top page title row with `LinkedIn Courses`.
- Status pill similar to `Downloader online`.
- Settings icon button.
- `Course Setup` panel.
- URL textarea with one URL per line.
- Download folder field and browse button.
- Token field with clear/import affordance.
- Video resolution select defaulting to `1080p (Best available)`.
- Browser token source select.
- Delay input.
- Download videos, exercise files, and subtitles checkboxes.
- Import Token, Start Download, and Cancel actions.
- Right Activity panel with live progress, recent activity, and completed area.
- Download Queue panel with per-course and per-artifact progress rows.

## MVP Scope Override

The screenshot shows `Generic Video`, `Tools`, `History`, `LinkedIn Scraper`, and `Settings` nav rows. For the MVP:

- `LinkedIn Courses` is the only working downloader route.
- `Settings` may be implemented if needed for tokens/cache/theme.
- `Tools` may exist only if it supports LinkedIn downloader dependencies.
- `History` may map to SQLite job history.
- `Generic Video` must be hidden, disabled, or explicitly unavailable.
- `LinkedIn Scraper` must remain out of scope.

## Visual Acceptance Gates

Desktop:

- `1536x1024` reference viewport must fit without overlapping controls.
- Activity panel remains visible beside the setup panel.
- Download queue remains visible below setup.
- Progress rows do not resize their container when percentages change.
- Token value is hidden, clipped, or password-style masked.

Responsive:

- `1280x800` must still show setup and enough queue/activity context to feel like the same app.
- `390x844` must stack without horizontal scroll.
- Long labels and URLs must not overflow buttons, cards, or sidebars.

Interaction:

- Start Download is disabled or guarded until required fields are present.
- Cancel is available only when work can be cancelled.
- Import Token shows loading, success, empty, and failure states.
- File/folder picker failures produce Sonner toasts and do not corrupt settings.

