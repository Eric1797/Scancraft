## Resolutions from human (§14, 2026-10-14)

- **R1. Name**: keep **ScanCraft**; agent picks a Lucide-based mark
  (scan/printer geometry, own colours — no PdfCraft brand reuse).
- **R2. PdfCraft reuse**: "copy whatever u want … look and super fast rust
  implementation, ui looks like pdfcraft". ⇒ Copy UI theme/widget modules
  with attribution into a ScanCraft UI kit crate (D1 option c for UI);
  reuse engine libs (create/crypt/ocr/render) via path deps for M1–M2
  speed, revisit git-pinning before M8. Speed of implementation wins.
- **R3. Release**: **max speed** (`opt-level = 3`); accept bigger binary,
  still report size vs 12 MB aim / 25 MB hard fail.
- **R4. TWAIN**: M7 behind a setting; WIA + eSCL first. v1 signs off
  without TWAIN.

## D1. PdfCraft code reuse: path vs git vs copy — PENDING HUMAN (§14 Q3)

Options:
- (a) **Path deps** (monorepo-style): simplest now; ScanCraft root keeps
  `../../pdf craft/crates/*` deps. Fragile across machines (space in path,
  requires the checkout). Current `scanner/` scaffold uses this.
- (b) **Git deps**: needs PdfCraft published at a pinned rev; cleanest for
  release/CI. PdfCraft is a public GitHub repo — feasible.
- (c) **Copy modules** (theme, widgets, control channel): only where a
  shared crate is refused; each copy needs attribution + a sync note, and
  `ATTRIBUTION.toml`-style entries.
- UI theme: prefer a tiny shared crate (`scancraft-ui-kit` or upstream a
  PdfCraft theme crate) over copying `theme.rs`. If inseparable, attributed
  copy with tokens in one file.
- Default if human defers: (a) for M1–M2, revisit before M8 packaging.

## D2. Release profile: size vs speed — PENDING HUMAN (§14 Q4)

- PdfCraft ships `lto = "thin"`, `codegen-units = 1` (no `panic = "abort"`,
  no `strip`). Spec §2 proposes `fat`/`abort`/`strip`/`opt-level 3-or-s`.
- `panic = "abort"` conflicts with never-crash + recovery-file story
  (recovery writer must survive worker panics). Lean: keep `unwind`,
  measure `opt-level 3` vs `"s"` for the ≤12 MB aim / 25 MB hard fail.
- Decision after first size measurement (`xtask size` lands in M1).

## D3. Workspace layout: `scanner/` scaffold vs spec §3.1 — PENDING HUMAN

- Current: `scancraft/scanner/` (Cargo package: `pdfcraft-scanner` bin +
  `design/` notes + `docs/`). Spec wants `scancraft/` as workspace root
  with `apps/scancraft`, `apps/scancraft-cli`, `crates/scancraft-*/`,
  `xtask/`, `docs/`, `packaging/`, `assets/`.
- Restructure to spec layout at M0-close once Q3/Q1 answered; the scaffold
  `src/{main,app,scan,print_job}.rs` becomes the seed of
  `apps/scancraft/` + `crates/scancraft-engine/` sketch, not thrown away.

## D4. NAPS2 reference is behavior-only (decided, binding)

- The reference snapshot is **C#/.NET under GPL-2.0-or-later** (not C++).
  Target license MIT OR Apache-2.0 ⇒ **no code/structure/comments ported**.
- Allowed: feature/option names, ranges, defaults (NAPS2 facts);
  driver concepts; test scenarios.
- Resolves spec §4 ("don't read source for logic") vs "use as reference":
  interface/behavior level only. Interface shapes noted in
  `architecture-notes.md` §5; logic re-derived from vendor specs.

## D5. OCR: reuse PdfCraft's ocrs+rten stack (decided, to verify in M5)

- No Tesseract (C++ dep violates ultralight). `pdfcraft-ocr` + `xtask models`
  pattern reused behind `scancraft-ocr`. Same Latin-script limits as PdfCraft,
  stated in docs, not over-promised.

## D6. PDF writer: reuse `pdfcraft-create` if its API covers scans (to verify M2)

- Scan→PDF = image-per-page + DPI `/MediaBox` + metadata + encryption
  (`pdfcraft-crypt`) + OCR layer. Only if gaps appear do we write a minimal
  writer in `scancraft-export` (Catalog→Pages→Page→image XObject;
  DCT/Flate/CCITT pass-through).

## D7. Threading: std threads + channels, no async runtime (decided)

- Matches "no async runtime unless unavoidable". UI never blocks; COM/WIA on
  one dedicated STA thread; every long op cancellable + progress-reporting.
- `rayon` only if imaging benchmarks justify it (record here if added).

## D8. Settings format: follow PdfCraft's choice (to confirm in M0-close)

- PdfCraft uses JSON (`serde_json`) for settings + TOML for manifests;
  profiles/settings will be versioned + migrated + atomic + `.bak` fallback.
  Confirm exact files during skeleton work.
