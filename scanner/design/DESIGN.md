# Scanner design (from PdfCraft, by reference)

> PdfCraft checkout: `C:\Users\Eric George\Downloads\pdf craft`
> (relative `../../pdf craft/` from `scancraft/scanner/`).
> Nothing below duplicates an asset file. Paths point at the originals so
> `cargo xtask assets` / `ATTRIBUTION.toml` stay valid (AGENTS.md §1).
> Copying an `.svg`/font/png in here would fail the assets check —
> keep referencing, don't copy.

## 1. UI framework

- `pdfcraft-ui-egui` (`../../pdf craft/crates/ui-egui`, egui 0.36 + eframe) is the design system.
- Reuse, in order:
  1. `theme.rs` — app theme (light/dark, spacing, corners).
  2. `palette.rs` — colour tokens.
  3. `icons.rs` + `icon_data.rs` — Lucide icon embedding
     (`../../pdf craft/assets/icons/*.svg`, ISC).
  4. `widgets.rs`, `pickers.rs`, `dialogs.rs` — controls.
  5. Panels to mirror: `print_ui.rs` (print dialog), `ocr_ui.rs` (scan/OCR),
     `create_ui.rs` (images -> PDF), `home.rs` (landing), `chrome.rs` (frame).

## 2. Scanner layout (suggested, adapt to instructions)

```text
+--------------------------------------------------+
| Scanner  [Scan v] [Preview] [OCR] [Print]   [_][X]|
+----------------------+---------------------------+
| Sources              | Preview (pdfcraft-render) |
| - flatbed / ADF      |  page image, zoom,        |
| - DPI 150/300/600    |  rotate/reorder           |
| - colour/mono/grey   |                           |
| - duplex on/off      |                           |
+----------------------+---------------------------+
| Output               | Print (pdfcraft-print)    |
| - PDF path           |  printer, copies,         |
| - OCR toggle + lang  |  fit/actual/N-up,         |
| - [Scan] [Save PDF]  |  [Print] [Save sheets]    |
+----------------------+---------------------------+
```

- Layering (CLAUDE.md): nothing below L7 depends on egui — keep
  `scan.rs`/`print_job.rs` UI-free; `app.rs` is the only egui layer.
- Never-crash (AGENTS.md §4): all hardware/file input is untrusted —
  `Result` + `?`, no `unwrap/expect/panic`, checked arithmetic, capped
  allocations, bounded recursion.

## 3. Print rules (from `../../pdf craft/crates/print/README.md`)

- Size: fit (sheet minus 18pt margin), actual, shrink-oversized, custom %;
  centred; auto-orientation for landscape pages.
- Multiple: 2/4/6/9/16 per sheet, order horizontal/vertical/reversed,
  page borders, auto-rotate mismatched cells; cut-and-stack order supported.
- Booklet: saddle-stitch, multiple of 4, both/front/back, left/right bind.
- Poster: tile scale + overlap + cut marks.
- Sheets place each source page as a Form XObject (content wrapped q/Q +
  printable annotations), fresh unencrypted file.
- Spool: CUPS (`lpstat`/`lp` piped on stdin, never a temp file); Windows via
  WMI printer list + 200-dpi raster streamed to PowerShell PrintDocument
  (`PDFCRAFT_*` env, never script text).

## 4. Scan -> PDF rules (from `pdfcraft-create` + `pdfcraft-ocr`)

- `pdfcraft-create`: blank docs, from images (PNG/JPEG), text, docx;
  scanner output = images -> PDF path (PNG 0.18, tiff 0.11, image 0.25).
- `pdfcraft-ocr` (ocrs 0.13.1 + rten): recognise page images, write invisible
  searchable text layer; needs `pdfcraft-fonts`.
- Fonts: never commit a font here. Latin UI faces live in
  `../../pdf craft/assets/fonts/` (Inter, JetBrains Mono, Dancing Script); Japanese comes
  from `CRAFT_FONTS_DIR` (craft-fonts repo) at build time. Scanner previews
  must not screenshot proprietary/system fonts (AGENTS.md §1.2/§1.4).

## 5. Brand

- `docs/brand/*` (ArtCraft wordmark/logos) is
  `LicenseRef-Storyteller-Trademark` — PdfCraft-only, unmodified, `kind="logo"`.
  Do NOT reuse for the scanner. Give the scanner its own name + Lucide-based
  mark (e.g. `scan` / `printer` glyph, own colours).
- App icon: do not copy `../../pdf craft/assets/app-icon/pdfcraft.*`; generate a new one for
  the scanner (and its own licence/attribution entries if committed).

## 6. What was deliberately NOT copied

- `../../pdf craft/assets/icons/*.svg`, `assets/fonts/*`, `assets/app-icon/*`,
  `../../pdf craft/docs/brand/*` — referenced only (asset policy).
- `../../pdf craft/crates/ui-egui/src/*.rs` — imported as `pdfcraft-ui-egui` lib, not forked.
- `../../pdf craft/crates/print|ocr|create|render|engine/...` — path dependencies in
  `scanner/Cargo.toml`, not copies.
