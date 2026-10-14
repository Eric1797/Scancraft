# ScanCraft — scan documents to PDF and more, as simply as possible

Ultralight, pure-Rust document scanning for Windows (portable core):
scan → organize → export PDF/TIFF/JPEG/PNG with optional OCR text layer.
A sibling of [PdfCraft](https://github.com/storytold/pdfcraft): same visual
language, same command palette, same dark/light themes.

> M0 complete (discovery + skeleton). M1 in progress (app shell + mock scanner).

## Layout

```text
apps/scancraft/          desktop app (thin: wires engine + UI)
apps/scancraft-cli/      headless CLI (devices, profiles; full tool table later)
crates/scancraft-model/      pure data (no I/O)
crates/scancraft-scan-core/  OS-neutral backend contract
crates/scancraft-scan-mock/  deterministic fake scanner (tests + dev)
crates/scancraft-engine/     sessions, documents, undo/redo
crates/scancraft-ui-kit/     theme tokens + shared widgets (from PdfCraft, see NOTICE)
docs/  assets/  scanner/  packaging/ (M8)
```

`scanner/` is the pre-spec scaffold (kept for its `design/` notes);
the workspace above is the real project (see `docs/decisions.md` D3).

## Build

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p scancraft-cli -- devices
cargo run -p scancraft            # desktop shell (M1)
```

Windows x64 exe (release) is also built by GitHub Actions on every push
to `main` (`.github/workflows/exe.yml`) and attached as an artifact.

## Rules

- 100% Rust, no C/C++ deps in the default build. `unsafe` only in OS
  crates with `// SAFETY:` comments; everything else
  `#![forbid(unsafe_code)]`.
- Never crash on untrusted input: `Result` + `?`, no unwrap outside tests.
- NAPS2 reference (`naps2-in c++-master #For Reference/`, GPL) is
  behavior-only: no code from it enters this repo (see
  `docs/architecture-notes.md` §7).
