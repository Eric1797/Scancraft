# ScanCraft M0 — Architecture notes (PdfCraft + NAPS2 discovery)

> First deliverable per spec §0 rule 1. Read before writing feature code.
> All PdfCraft facts below were read from the repo, not guessed.
> NAPS2 facts are interface/behavior-level only (see §7 — GPL, no code copying).

## 1. PdfCraft workspace (source of truth for structure)

- Root `Cargo.toml`: `resolver = "3"`, version `0.4.0`, `edition 2024`,
  `rust-version 1.90`, `license MIT OR Apache-2.0`.
- Members: `crates/*`, `apps/*`, `xtask`; `exclude = ["vendor"]`;
  `default-members = ["crates/*", "apps/pdfcraft", "apps/pdfcraft-cli", "xtask"]`.
- `[patch.crates-io]`: `hayro`, `hayro-syntax`, `hayro-interpret`,
  `hayro-jbig2`, `lopdf` → `vendor/*` (patched, meant for upstream).
- Profiles: release = `lto = "thin"`, `codegen-units = 1`;
  dev `opt-level = 1`; dev deps `opt-level = 2`.
  (Spec §2 suggests `lto = "fat"`, `panic = "abort"`, `strip = true` —
  that **differs** from PdfCraft; decision needed, see `decisions.md` D2.
  `panic = "abort"` also conflicts with PdfCraft's never-crash stance.)
- Lints: workspace `unsafe_code = "forbid"`; clippy `dbg_macro`, `todo` warn.
  Clean crates add
  `#![deny(clippy::unwrap_used, expect_used, panic, unimplemented, todo, unreachable)]`.
  `clippy.toml` allows unwrap/expect/panic/indexing in tests only.
- `rustfmt.toml`: `max_width = 150, use_small_heuristics = "Max"`.
- `deny.toml`: `all-features = true`; allowlist
  `MIT, MIT-0, Apache-2.0(+LLVM), BSD-2/3, ISC, Zlib, Unicode-3.0, BSL-1.0, CC0-1.0`;
  `wildcards = deny`; ignores `RUSTSEC-2023-0071`; exception
  `epaint_default_fonts: OFL-1.1, Ubuntu-font-1.0`.
- `.cargo/config.toml`: alias `xtask = "run -p xtask --"`.

## 2. PdfCraft crate map (30 crates, one line each)

| Crate | Role (from its Cargo.toml / README) |
|---|---|
| `cos` | PDF object layer: lazy, tolerant, copy-on-write, incremental |
| `filters` | Flate/LZW/predictors/ASCII85/Hex/RunLength |
| `crypt` | Standard security §7.6 (RC4/AES r2–6) — reuse for PDF encryption |
| `geom` | Points/rects/transforms |
| `fonts` | Metrics/WinAnsi/appearances; `pdfcraft_fonts::CRAFT_FONTS` via `CRAFT_FONTS_DIR` |
| `model` | Page tree, inherited attributes, geometry |
| `content` | Content streams |
| `organize` | Rotate/delete/move/insert/info — closest to page-organization needs |
| `measure` | Calibrated 2D measure |
| `annot` | Comments/builders/appearances |
| `forms` / `xfa` / `xfdf` | AcroForm, XFA, XFDF/FDF |
| `redact` | Marks/apply/verify |
| `create` | Blank docs + **images (PNG/JPEG/TIFF) / text / docx → PDF** — reuse for scan→PDF (verify in M2) |
| `render` | hayro-backed rendering + inspection (bootstrap; being replaced by cos/model) |
| `print` | Sizing, N-up, booklet, poster, `impose()`, `spool::submit()` (CUPS / Windows WMI+PowerShell) |
| `ocr` | **`ocrs 0.13.1` + `rten`** recognition + invisible text layer — pure Rust, reuse behind `scancraft-ocr` |
| `js` | Sandboxed Acrobat JS (`boa`) — not needed for ScanCraft |
| `compare` / `preflight` / `a11y` | Diff, PDF/A verify, accessibility checker — not v1 |
| `optimize` | Optimizer |
| `sign` | Signatures/PAdES — not v1 |
| `export` | HTML/docx/RTF — ScanCraft needs its own image/PDF export instead |
| `edit` | Header/footer/watermark/Bates — not v1 |
| `engine` | **Façade**: `Session` (DocId→Document COW overlay), clone-on-edit + undo stack, incremental working-file refresh, `SaveOptions`, `RenderPool`; re-exports domain types. Frontends never touch parsing/render/edit directly |
| `platform` | OS services (staging, shared) — copy the Reveal/settings-dirs approach |
| `ui-egui` | egui shell (60 files: per-feature `*_ui.rs` + chrome/canvas/panels/control/i18n/portable/recovery/…) |
| `automation` | Headless tool-set + MCP (L7) — the pattern to mirror |

Shared dep versions: `egui/egui_extras/eframe/egui_kittest 0.36`,
`serde 1 {derive,rc}`, `serde_json 1`, `thiserror 2`, `log 0.4`,
`image 0.25`, `png 0.18`, `hayro 0.7.1`, `lopdf 0.45`, `rfd 0.17`.

## 3. UI toolkit, theme, command palette, automation (from code)

- **egui 0.36 + eframe** (`default_fonts, wgpu, accesskit, wayland, x11,
  persistence, links`; web uses `glow`). Helpers: `egui_extras` (svg),
  `egui_kittest` (control-channel tests), `rfd` (file dialogs),
  `arboard` (clipboard), `sys-locale`, `accesskit` (a11y).
- **Theme** (`ui-egui/src/theme.rs`): `ThemeKind{Light,Dark}`,
  `ThemePreference{System,Light,Dark}::resolve()`, `Tokens{…}` mapped onto
  `egui::Visuals` + spacing + text styles (Body/Button 13, Small 11,
  Heading semibold 17). Full token table in `design-tokens.md`.
- **Correction to spec §5.1:** `palette.rs` is the **⌘K command palette**
  (fuzzy `score()` over `engine::commands::COMMANDS` + `catalog::TOOL_GROUPS`,
  top-12, Enter/click runs command), **not** a color palette.
  Colors live in `theme.rs` `Tokens`.
- **Icons** (`icons.rs` + `icon_data.rs`): embedded Lucide SVGs from
  `assets/icons/` (ISC), `currentColor→#ffffff`, stroke 2→1.75, rastered via
  `egui_extras` at exact size, `tint()` per use. Tests assert every
  catalog/command icon exists. Attribution via `ATTRIBUTION.toml` →
  generated `ATTRIBUTION.md`; enforced by `cargo xtask assets`.
- **Fonts**: `assets/fonts/` Inter (+Medium/SemiBold), JetBrains Mono first,
  then egui defaults, then CJK/Arabic/Telugu from `craft-fonts` build input;
  desktop appends one installed system fallback at runtime (never embedded).
  **Never commit a font file** (AGENTS.md §1.4).
- **Engine commands/catalog** (`engine/src/commands.rs`, `catalog.rs`):
  `COMMANDS{id, label, menu, shortcut, icon, Needs(…)}`;
  `TOOL_GROUPS{id, label, icon, availability: Ready|Planned|Provider, sections}`.
  The palette, menus, Tools panel and automation all read these tables.
- **Automation** (`automation/src/{lib,tools,mcp}.rs`): one table
  `ToolDef{name (snake_case), title, description, input_schema (JSON-Schema-2020-12), read_only, destructive, command}`;
  `call(name, args) -> Vec<Content{Json|Png}>`; same table drives
  `pdfcraft-cli run` (`key=value`/JSON, `--script steps.json`, `--out`) and
  the opt-in stdio-only MCP server (`--compact` core + `tool_search/tool_call`,
  `--root` confinement, atomic `write_output`). UI control channel:
  app `--control <file>` (loopback, random token, owner-only file) +
  `pdfcraft-cli ui` (`inspect/click/screenshot/…`); kittest tests.
- **xtask** (14 cmds): `version|layers|wasm|deny|ci|assets|corpus|check|fuzz|
  parity|text-oracle|screenshots|models|demo-pdf`. `xtask ci` =
  `fmt --check`, `clippy -D warnings`, `test`, `layers`, `wasm`, `assets`,
  `deny`, `parity`. OCR models arrive via `xtask models`.
- **CI** (`.github/workflows/ci.yml`, ubuntu/macos/windows):
  fmt, clippy, test (+`CRAFT_FONTS`), layers, assets, no-mcp, wasm, deny,
  nightly fuzz+corpus. Release builds macOS-universal, win x64/x86/arm64,
  linux x86_64/aarch64, flatpak, freebsd, web + SHA256SUMS.
- **Packaging**: `packaging/{linux,macos,windows,freebsd,web}` incl.
  `windows/{pdfcraft.wxs, package.ps1, sign.ps1, portable.txt}` and
  `portable.txt` semantics (settings/logs/recovery next to exe).
- **Localization**: `ui-egui/src/i18n/*.tsv` catalogs compiled in
  (see `ATTRIBUTION.toml` translation entries); settings has language picker.
- **Docs layout**: `docs/{brand,contributors.md,development.md,
  localization.md,releasing.md,images}`; `parity/acrobat-features.toml`
  (~10k lines, statuses planned/partial/shipped/na) enforced by `xtask parity`.

## 4. Reuse candidates for ScanCraft (to verify in M0/M2, not assumed)

| Need | Candidate | Status |
|---|---|---|
| Scan→PDF writer | `pdfcraft-create` (images→PDF) | verify image→page API covers DPI page size + metadata |
| PDF encryption | `pdfcraft-crypt` | likely reusable |
| OCR engine+models | `pdfcraft-ocr` (ocrs+rten) + `xtask models` | likely reusable; same Latin-script limits apply |
| PDF verify round-trip | `pdfcraft-cos`/`render` parser+renderer | reuse in tests |
| Theme/widgets/palette/control-channel | `pdfcraft-ui-egui` modules | path-dep vs shared-crate vs copy — see D1 |
| Page organize ops | `pdfcraft-organize` | model differs (PDF pages vs scan pages); probably reimplement on own model |
| Import PDF pages | `pdfcraft-render` | verify page→image API |

## 5. NAPS2 reference — what it is and how to use it

- Location: `scancraft/` → `naps2-in c++-master #For Reference/`.
- **Correction: it is C#/.NET (33× `*.csproj`, `NAPS2.sln`), not C++.**
  SDK: `NAPS2.Sdk/` (Scan, Images, Pdf, Ocr, ImportExport);
  image core: `NAPS2.Images/` (+22 transforms, +26 bitwise ops);
  backends per platform (`Gdi/Wpf/Gtk/Mac/ImageSharp`);
  apps: WinForms/Gtk/Mac/Console/Worker/PortableLauncher;
  network: `NAPS2.Escl/` (+Server/Usb).
- Driver contract (`IScanDriver`, internal): `GetDevices`,
  `GetCaps`, `Scan` — callback/event style with cancel token.
  Mirrors spec §6.1; our `scan-core` trait should look similar
  (list/capabilities/scan + sink + cancel + closed error enum).
- `ScanOptions` field set (behavior vocabulary, not code): driver, device,
  paper source (flatbed/feeder/duplex), DPI, scale ratio, page size,
  brightness ±1000, contrast ±1000, bit depth, alignment, native UI,
  stretch/crop-to-page, blank exclusion (white threshold **70**,
  coverage threshold **15**), max-quality flag, JPEG quality (**75** default),
  thumbnail size, deskew, auto-crop, correction mode, fixed rotation,
  flip-duplex-pages, OCR params/priority, per-driver option bags
  (Wia/Twain/Sane/Escl), barcode options.
- Scan profiles: `ScanProfile` (+Device/Caps), `ProfileManager` +
  serializer, `EditProfileForm`/`AdvancedProfileForm`.
- Image ops: `Deskewer`, `BlankDetectionImageOp`, 22 transforms
  (crop/rotate/resize/correction/B&W/brightness…), tests alongside.
- PDF: `PdfExporter`/`PdfiumPdfExporter` + params/metadata/encryption/
  compression/PDF-A helper. OCR: `IOcrEngine`/`TesseractOcrEngine`/
  `StubOcrEngine` + queue + language manager.

## 6. Where spec and reality differ (all recorded, PdfCraft wins on structure)

1. NAPS2 is C#, GPL — not C++ (spec §0 rule 8 wording).
2. `palette.rs` = command palette, not colors (spec §5.1).
3. PdfCraft OCR is ocrs+rten, pure Rust — better than spec §8.3 assumes; no Tesseract needed.
4. Release profile differs (`thin`/no-strip vs spec's `fat`/`abort`/`strip`) — D2.
5. Current `scanner/` scaffold ≠ spec §3.1 layout (`apps/`+`crates/` at root) — D3.
6. Spec §4 forbids reading NAPS2 source for logic while user rule 8 asks to
   use it as implementation reference — resolved as: behavior/interface
   reference only (§7).

## 7. License boundary (updated 2026-10-15: project is GPL)

- ScanCraft is **GPL-2.0-or-later** (human decision), same as NAPS2, so
  porting NAPS2 scan logic to Rust is compatible. Ported files carry an
  origin header (see `docs/decisions.md` D4).
- Still excluded from porting: NAPS2's C/C++-dependent parts (pdfium,
  Tesseract) — pure-Rust equivalents only (ultralight rule).
- PdfCraft-derived code (MIT/Apache, e.g. UI theme) inside this GPL project
  is allowed with notices retained (`NOTICE`).
