# Feature matrix (M2 — updated; verify NAPS2 column vs naps2.com before M3)

> Columns: feature | NAPS2 behavior (from reference names; **UNVERIFIED** vs
> live docs until checked) | ScanCraft status | milestone | test.
> Status values: `missing`, `planned`, `partial`, `shipped`.

## Scanning

| Feature | NAPS2 behavior (UNVERIFIED vs naps2.com) | Status | MS | Test |
|---|---|---|---|---|
| Device discovery (WIA/TWAIN/eSCL) | Drivers enumerated per platform; TWAIN 32-bit via worker | partial (mock only) | M3/M6/M7 | mock + hardware list |
| Profiles (named/default/quick-scan) | `ScanProfile` + manager + edit/advanced dialogs | partial (model only, no UI) | M4 | settings migration |
| Sources flatbed/feeder/duplex, DPI 50–1200, presets | `ScanOptions` fields (see arch notes §5) | partial (mock honors request) | M3 | mock matrix |
| Bit depth color/gray/BW, scale 1:1–1:8 | Per NAPS2 option bags | partial | M3 | mock |
| Brightness/contrast ±1000, deskew, crop, blank-detect (70/15) | Post-scan pipeline options | partial (imaging fns done, no scan-time wiring) | M2/M3 | property + golden |
| Rescan selected / scan-again append | Replace-in-place, append runs | partial (append works) | M3 | engine test |
| Batch (prompt/delay/none, scan-N-times) | Batch dialog | missing | M4 | engine test |
| Auto-save (pattern, per-page split, clear-after) | Auto-save settings | missing | M4 | template tests |
| Feeder errors (empty/jam/cover/multifeed) | Mapped user-readable errors | partial (mock injection) | M3 | mock injection |

## Pages / image tools

| Feature | NAPS2 behavior (UNVERIFIED) | Status | MS | Test |
|---|---|---|---|---|
| Grid + single view, virtualized thumbs | Thumbnail cache + size slider | partial (grid + single + zoom, no virtualization) | M1 | UI manual |
| Select (click/Ctrl/Ctrl+A/Esc), drag-reorder, delete | Edit ops | shipped | M1 | engine undo tests |
| Reverse / interleave / de-interleave | Page ops | shipped | M1 | engine tests |
| Rotate/flip (non-destructive stack) | Edit stack | shipped | M2 | edit-stack test |
| Crop (interactive + numeric), brightness/contrast/B&W, sharpen | Image tools | partial (fns + tests, no UI yet) | M2 | imaging tests |
| Deskew / auto-crop / blank detect | Auto tools | partial (fns + tests, no UI/scan wiring) | M2 | imaging tests |
| Undo/redo named steps | Deep history | shipped | M1 | engine tests |
| Import PDF/TIFF/JPEG/PNG/BMP/GIF/WEBP | Import pipeline | missing | M2 | corrupt-input sweep |

## Export

| Feature | NAPS2 behavior (UNVERIFIED) | Status | MS | Test |
|---|---|---|---|---|
| PDF (DPI MediaBox, metadata, Flate RGB) | `PdfExporter` params | shipped (limits in D10) | M2 | xref-validated test |
| TIFF multipage + DPI tags (uncompressed) | Per-format options | shipped (limits in D10) | M2 | image-crate round-trip |
| JPEG quality / PNG | Per-format options | shipped (no DPI tags yet) | M2 | round-trip |
| Filename placeholders + no-overwrite | Template tokens, never overwrite silently | shipped | M2 | template + collision tests |
| Atomic writes | Temp + rename | shipped | M2 | atomic test |
| CLI `scan --out` (pdf/tiff/jpg/png, --force) | Console app parity | shipped | M2 | manual e2e |
| Save dialog (format tabs, metadata, quality, pattern, folder) | Save dialog | shipped | M2 | manual |
| Email via default client | Low priority | missing | M8 | manual |
| Print | Later | missing | M8 | — |

## App-level

| Feature | NAPS2 behavior (UNVERIFIED) | Status | MS | Test |
|---|---|---|---|---|
| Tabs (multi-doc, dirty dot) | Multi-doc UI | shipped (no close-confirm yet) | M1 | manual |
| Command palette (Ctrl+K) | Command search | shipped (no icons yet) | M1 | manual |
| Themes (system/light/dark, PdfCraft tokens) | Theming | shipped | M1 | contrast test |
| Recovery autosave 60s + crash offer | Mirror PdfCraft pattern | missing | M2 | kill-and-recover |
| Portable mode (`portable.txt`) | Same semantics as PdfCraft | missing | M4 | path tests |
| Preferences, i18n pipeline | Mirror PdfCraft | missing | M4/M8 | screenshot EN+1 |
| MSI + portable zip, signing hooks | Mirror `packaging/` | missing | M8 | CI artifacts |
| MCP + `--control` UI channel | Mirror automation pattern | missing | M8 | control tests |

## Verification debt

- [ ] Check feature list against https://www.naps2.com/support + wiki.
- [ ] Screenshots light/dark vs PdfCraft (M1c) + `cargo xtask screenshots`.
- [ ] `qpdf --check` + poppler + PdfCraft round-trip on exported PDFs.
- [ ] Close-confirm dialog once save exists (now possible — M2b).
- [ ] Icons (Lucide set) in palette/tools/grid.
- [ ] Answer remaining: xtask (layers/assets/size gates) in M1c.
