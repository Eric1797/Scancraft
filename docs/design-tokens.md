# Design tokens (extracted from PdfCraft code — reuse, don't re-eyeball)

> Source: `pdfcraft/crates/ui-egui/src/theme.rs`, `icons.rs`, `icon_data.rs`.
> Re-verify against the files before M1 screenshots; tokens live in one place
> (`theme.rs` `Tokens`) and ScanCraft must reuse the same struct shape.

## 1. Toolkit

- `egui 0.36`, `eframe 0.36` with features
  `default_fonts, wgpu, accesskit, wayland, x11, persistence, links`
  (app crate; web swaps in `glow`).
- Helpers: `egui_extras 0.36` (`svg` feature — icon rastering),
  `egui_kittest 0.36` (UI tests), `rfd 0.17` (native file dialogs),
  `arboard` (clipboard), `accesskit` (a11y tree), `sys-locale` (Windows).

## 2. Theme struct (`theme.rs`)

- `ThemeKind { Light, Dark }`, `ThemePreference { System, Light, Dark }`
  with `resolve()` (system default).
- `Tokens` fields: `titlebar, chrome, panel, pasteboard, card, border,
  divider, text (+muted/faint), icon, hover/pressed/selected,
  accent (+text/soft), field, badge_new, page_shadow, radius`.
- `Tokens::for_kind(kind)` → `apply(ctx, kind)` maps tokens onto
  `egui::Visuals` + spacing + text styles. ScanCraft: reuse the same
  `Tokens` shape (shared crate or attributed copy — D1).

## 3. Typography

- Text styles: Body/Button **13pt**, Small **11pt**, Heading semibold **17pt**.
- Families: Inter Regular first, Inter Medium / SemiBold as custom
  `medium`/`semibold` families, JetBrains Mono Regular for mono/code,
  then egui defaults, then CJK (Hans-first in Chinese mode) / Arabic /
  Telugu from the `craft-fonts` build input, then **one** installed system
  fallback at runtime (desktop only, never embedded, off with
  `PDFCRAFT_SYSTEM_FONTS=0`, validated via `skrifa`).
- Rule: points, never pixels (HiDPI). Min window 800×560.

## 4. Icons

- Lucide set, ISC license (`assets/icons/*.svg` + `LICENSE-lucide.txt`).
- Pipeline (`icon_data.rs`): `currentColor` → `#ffffff`,
  stroke-width 2 → 1.75, raster via `egui_extras` at exact display size,
  `tint()` per usage site.
- Tests assert every catalog/command icon resolves — mirror that test.
- Scanner needs (see `design/icons-needed.md`): `scan`, `scan-text`,
  `file-scan`, `camera`, `image`, `image-plus`, `printer`, `file-down`,
  `folder-open`, `settings-2`, `zoom-in/out`, `rotate-cw/ccw`, `trash-2`, `info`.

## 5. Components & conventions to mirror

- Docking panels (Tools left / Canvas center / Inspector right), tab strip
  with unsaved-dot, status bar with background-task progress + cancel,
  toasts, confirm dialogs, progress dialogs, empty states — all from the
  same widget set (`widgets.rs`, `pickers.rs`, `dialogs.rs`).
- Home screen: welcome + recommended actions + tool catalogue + privacy note
  + recents.
- Command palette: fuzzy over translated + English labels + command id,
  top-12, shortcut shown (see `palette.rs`).
- Sliders: double-click resets, numeric field for typing, live preview on a
  downscaled proxy, commit on release.
- Every interactive widget gets a **stable unique label** (accesskit +
  `--control` UI tests).
- Light and dark are both first-class; follow-system default.
- Animation: none documented in theme — keep motion minimal until PdfCraft
  defines conventions (record if added).

## 6. What ScanCraft adds (only if PdfCraft lacks it)

- Page-grid with virtualized thumbnails, insertion-marker drag-reorder,
  crop overlay + rotate handles on canvas, scan progress strip.
  New widgets go into the shared kit (D1) so both apps stay identical.
