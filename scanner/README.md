# scanner (standalone scaffold) — new root: scancraft/

Standalone printer / scanner app. It **imports** PdfCraft libs by path — no
PdfCraft source was forked/copied — and reuses the PdfCraft design system by
reference (see `design/`). Working behaviour (scan source, buttons, flows)
lands when you provide the instructions.

PdfCraft checkout lives at `C:\Users\Eric George\Downloads\pdf craft`
(relative `../../pdf craft/` from here).

## Layout

```text
scancraft/
  scanner/
    Cargo.toml        standalone package (own [workspace]), path deps on ../../pdf craft/crates/*
    src/main.rs       CLI (--help/--version) + shell entry, Result-based, no unwrap
    src/app.rs        minimal shell; full egui layout wired later
    src/scan.rs       planned scan pipeline (create + ocr), settings + validate
    src/print_job.rs  planned print pipeline (print impose + spool), settings
    design/DESIGN.md        design system notes + pointers (no assets copied)
    design/libs.md          lib import map (which PdfCraft crate does what)
    design/icons-needed.md  Lucide icons to use, referenced from PdfCraft assets/
    README.md           this file
  naps2-in c++-master #For Reference/   (pre-existing reference, untouched)
```

## Libs (path imports, see design/libs.md)

- `pdfcraft-print` — impose + `spool::submit` (CUPS / Windows)
- `pdfcraft-create` — scanned images -> PDF
- `pdfcraft-ocr` — searchable text layer
- `pdfcraft-render` — page previews
- `pdfcraft-engine` — session/document/commands
- `pdfcraft-model/cos/content` — PDF object graph
- `pdfcraft-platform` — OS services
- `pdfcraft-ui-egui` — theme, palette, icons, print/ocr/create panels

## Design (references, not copies)

Per PdfCraft `AGENTS.md` §1 (asset policy + `cargo xtask assets` hash check),
no icon/font/brand files were duplicated here. Design tokens are described
in `design/DESIGN.md` and point at the originals in the PdfCraft checkout:

- `../../pdf craft/assets/icons/*.svg` (Lucide, ISC)
- `../../pdf craft/crates/ui-egui/src/theme.rs`, `palette.rs`, `icons.rs`
- `../../pdf craft/crates/ui-egui/src/print_ui.rs`, `ocr_ui.rs`, `create_ui.rs`
- `../../pdf craft/crates/print/README.md` (imposition rules)

Fonts are never committed here (`AGENTS.md` §1.4); they come from
`craft-fonts` via `CRAFT_FONTS_DIR` at build time, same as PdfCraft.
Brand logos in the PdfCraft `docs/brand/` are trademarked — do not reuse
them for the scanner; give the scanner its own name/mark.

## Build

Needs the PdfCraft checkout above (deps are `../../pdf craft/crates/*`):

```powershell
cargo run --manifest-path C:\Users\Eric` George\Downloads\scancraft\scanner\Cargo.toml -- --help
```

Detached `[workspace]` in `scanner/Cargo.toml` keeps it standalone: cargo
inside `scanner/` does not inherit the PdfCraft workspace.

## Next

Give the working instructions (scan source: WIA/TWAIN/SANE? ADF/duplex/DPI?
print UI: printer picker, copies, N-up? save flow?) and the shell in
`src/app.rs` + `scan.rs` / `print_job.rs` gets implemented end to end.
