# Feature matrix (M0 — honest baseline; verify vs naps2.com before M1)

> Columns: feature | NAPS2 behavior (from reference names; **UNVERIFIED** vs
> live docs until checked) | ScanCraft status | milestone | test.
> Status values: `missing`, `planned`, `partial`, `shipped`.

## Scanning

| Feature | NAPS2 behavior (UNVERIFIED vs naps2.com) | Status | MS | Test |
|---|---|---|---|---|
| Device discovery (WIA/TWAIN/eSCL) | Drivers enumerated per platform; TWAIN 32-bit via worker | missing | M3/M6/M7 | mock + hardware list |
| Profiles (named/default/quick-scan) | `ScanProfile` + manager + edit/advanced dialogs | missing | M4 | settings migration |
| Sources flatbed/feeder/duplex, DPI 50–1200, presets | `ScanOptions` fields (see arch notes §5) | missing | M3 | mock matrix |
| Bit depth color/gray/BW, scale 1:1–1:8 | Per NAPS2 option bags | missing | M3 | mock |
| Brightness/contrast ±1000, deskew, crop, blank-detect (70/15) | Post-scan pipeline options | missing | M2/M3 | property + golden |
| Rescan selected / scan-again append | Replace-in-place, append runs | missing | M3 | engine test |
| Batch (prompt/delay/none, scan-N-times) | Batch dialog | missing | M4 | engine test |
| Auto-save (pattern, per-page split, clear-after) | Auto-save settings | missing | M4 | template tests |
| Feeder errors (empty/jam/cover/multifeed) | Mapped user-readable errors | missing | M3 | mock injection |

## Pages / image tools

| Feature | NAPS2 behavior (UNVERIFIED) | Status | MS | Test |
|---|---|---|---|---|
| Grid + single view, virtualized thumbs | Thumbnail cache + size slider | missing | M1 | UI control tests |
| Select/reorder/delete/reverse/interleave | Edit ops incl. alternate (de)interleave, split/merge | missing | M1/M2 | engine undo tests |
| Rotate/flip/crop/adjust/B&W/deskew/auto-crop | Non-destructive edit stack | missing | M2 | golden + property |
| Import PDF/TIFF/JPEG/PNG/BMP/GIF/WEBP | Import pipeline | missing | M2 | corrupt-input sweep |

## Export

| Feature | NAPS2 behavior (UNVERIFIED) | Status | MS | Test |
|---|---|---|---|---|
| PDF (DPI MediaBox, metadata, JPEG passthrough, G4 BW, encryption, OCR layer) | `PdfExporter` params | missing | M2/M5 | qpdf + PdfCraft round-trip |
| TIFF multipage / JPEG quality / PNG | Per-format options + DPI tags | missing | M2 | round-trip |
| Filename placeholders + collision policy | Template tokens, never overwrite silently | missing | M2 | template tests |
| Email via default client | Low priority | missing | M8 | manual |
| Print | Later | missing | M8 | — |

## App-level

| Feature | NAPS2 behavior (UNVERIFIED) | Status | MS | Test |
|---|---|---|---|---|
| Recovery autosave 60s + crash offer | Mirror PdfCraft pattern | missing | M2 | kill-and-recover |
| Portable mode (`portable.txt`) | Same semantics as PdfCraft | missing | M4 | path tests |
| Preferences, themes, i18n pipeline | Mirror PdfCraft | missing | M4/M8 | screenshot EN+1 |
| MSI + portable zip, signing hooks | Mirror `packaging/` | missing | M8 | CI artifacts |
| CLI + MCP + `--control` UI channel | Mirror automation pattern | missing | M1/M8 | control tests |

## Verification debt (must clear before M1)

- [ ] Check feature list against https://www.naps2.com/support + wiki; fix deltas here.
- [ ] Confirm PdfCraft reuse APIs exist (D4/D6 table) by reading signatures.
- [ ] Answer §14 questions; close D1/D2/D3/D8.
