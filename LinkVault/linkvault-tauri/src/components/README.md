# LinkVault UI system

LinkVault uses local primitives in `primitives.tsx` for Jan-style desktop UI. New pages should compose these primitives before adding page-specific CSS.

Use these defaults:

- `Button`, `IconButton`, `Input`, `Textarea`, `Select`, `Checkbox`, and `Switch` for controls.
- `Panel`, `SectionHeader`, `DataTable`, `DataTableHeader`, `DataTableRow`, `EmptyRow`, `StatusBadge`, `SummaryChip`, and `ActivityEventRow` for app surfaces.
- `Tooltip`, `Popover`, and `Dialog` for overlays. `Popover` is viewport-clamped and portal-rendered, so sidebar/footer popups do not clip past the window edge.
- `SidebarItem` for navigation rows.
- `cn` from `src/lib/cn.ts` for variants and conditional classes.

Shell rules:

- The left rail is the elevated plane. Keep sidebar depth on `--rail-shadow`, not on page cards.
- The left rail is a floating rounded rectangle inside a transparent shell slot, with width controlled by `--sidebar-width` and a `.lv-sidebar-rail` drag handle.
- The sidebar supports Jan-style offcanvas collapse through `data-sidebar-state`; keep a visible reopen trigger outside the hidden rail.
- Sidebar content should keep a subtle top/bottom scroll mask and global thin scrollbars.
- Sidebar-triggered popovers should open from the rail into the workspace with `side="right"` and `align="start"` or `align="end"`, matching Jan's left-panel menus instead of floating over the rail.
- The main area is one continuous workspace ground. Use section borders and table/list dividers instead of raised card shadows for primary page regions.
- Use modal/popover shadows only for overlays and transient surfaces.
- Buttons that wait on persistence or native work should use the shared `loading` state so the spinner, disabled state, and label behavior stay consistent.
- Select/dropdown fields should use the shared `Select` primitive so the Jan-style shell, chevron, hover, and popover option colors stay consistent.

Keep reusable size, color, border, hover, and shadow values in `index.css` tokens or primitive variants. Page CSS should only describe page layout.
