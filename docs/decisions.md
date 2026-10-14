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

## D2. Release profile: size vs speed — RESOLVED (human 2026-10-14, updated)

- Max speed (`opt-level = 3`); keep `unwind` (never-crash + recovery).
- Size budget: **no limit until 60 MB** (human decision); hard fail above
  60 MB, report the number every release. The old 12 MB aim / 25 MB fail
  are dropped. Current: `scancraft.exe` 16.3 MB, `scancraft-cli.exe` 370 KB.

## D3. Workspace layout: `scanner/` scaffold vs spec §3.1 — PENDING HUMAN

- Current: `scancraft/scanner/` (Cargo package: `pdfcraft-scanner` bin +
  `design/` notes + `docs/`). Spec wants `scancraft/` as workspace root
  with `apps/scancraft`, `apps/scancraft-cli`, `crates/scancraft-*/`,
  `xtask/`, `docs/`, `packaging/`, `assets/`.
- Restructure to spec layout at M0-close once Q3/Q1 answered; the scaffold
  `src/{main,app,scan,print_job}.rs` becomes the seed of
  `apps/scancraft/` + `crates/scancraft-engine/` sketch, not thrown away.

## D4. NAPS2 reference is behavior-only (decided, binding)

- SUPERSEDED 2026-10-15 by human decision: the whole project is now
  **GPL-2.0-or-later** (same as NAPS2), so porting NAPS2 scan logic to Rust
  is license-compatible. Rules for ported code:
  - every ported file carries a header naming its NAPS2 origin;
  - port logic/structure, never bulk-copy comments verbatim without need;
  - C/C++-dependent NAPS2 parts (pdfium, Tesseract) are NOT ported —
    PdfCraft-style pure-Rust equivalents are used instead (ultralight rule);
  - the reference snapshot stays in-repo, still read-only.
- Original D4 text (pre-GPL, kept for history): the reference snapshot is
  C#/.NET under GPL-2.0-or-later (not C++); target was MIT OR Apache-2.0,
  so no code porting was allowed.

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

## D9. Imaging codecs: `image` + `flate2` + `rfd` (decided M2)

- `image 0.25` (features png/jpeg/tiff/bmp/gif only): decode/encode breadth
  without C deps. Same crate PdfCraft already vets. Justification for CI:
  pure Rust, no libtiff/libjpeg-turbo, minimal feature set.
- `flate2 1.1` (`rust_backend`): PDF FlateDecode streams.
- `rfd 0.17`: native folder picker in the Save dialog (Windows-first;
  Linux CI already installs GTK).
- No async runtime, no rayon (std threads + channels only, per D7).

## D10. Export limits shipped knowingly in M2 (all in feature-matrix)
- PDF images are Flate RGB (no JPEG pass-through yet, no G4 1-bit yet).
- TIFF is uncompressed RGB with DPI tags (LZW/G4 later).
- JPEG/PNG singles carry no DPI metadata yet (image-crate encoders lack
  the fields; M2b).
- No encryption (M2b), no OCR layer (M5), no PDF/A (later).
- Save runs on a worker thread; edit-stack rendering still on the UI
  thread for small pages (moves off-thread with the session cache, M2b).

## D11. eSCL backend: hand-rolled HTTP, spec vocabulary (decided M6-start)

- Protocol facts come from the public Mopria spec text + MIT/Apache client
  implementations (endpoint shapes, `ScanSettings`/`ScannerCapabilities`
  element names, three-hundredths-of-an-inch regions). No NAPS2 code read
  for logic; the reference folder stays behavior-only per D4.
- HTTP is hand-rolled blocking HTTP/1.1 over `TcpStream` (GET/POST/DELETE,
  Content-Length + chunked, caps, timeouts): zero new HTTP deps, no TLS
  stack, no async runtime. Plain HTTP only — printers serve eSCL over the
  LAN unencrypted; TLS needs the per-device trust UI first (M6b).
- `quick-xml 0.41` (same version PdfCraft vets) for capabilities; parsing
  is namespace-tolerant (local names) for vendor dialects.
- JPEG requested when advertised; PDF bytes from a device surface as an
  actionable `Unsupported` error (PDF raster import is M2b).
- `<scan:Duplex>` element for duplex requests is best-effort (wire name
  uncertain; devices refusing it map to `Unsupported`). Marked
  `UNVERIFIED-ON-HARDWARE` until a real AirScan device runs it.
- Proven by an in-process mock eSCL server (capabilities → job → pages →
  404 → DELETE) in crate tests. mDNS `_uscan._tcp` discovery is M6b;
  hosts are explicit for now.
