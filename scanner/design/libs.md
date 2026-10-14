# Lib import map (scanner -> PdfCraft)

PdfCraft checkout: `C:\Users\Eric George\Downloads\pdf craft`
(relative `../../pdf craft/` from `scancraft/scanner/`).

All entries are path dependencies in `scanner/Cargo.toml`
(`path = "../../pdf craft/crates/<name>"`, version `0.4.0`). No source copied.

| Scanner need | PdfCraft crate (path) | Provides |
|---|---|---|
| Print sheets + spool | `../../pdf craft/crates/print` (`pdfcraft-print`) | `select_pages`, `Settings`, `layout()` previews, `impose()` print-ready PDF, `spool::submit()` (CUPS / Windows WMI+PowerShell) |
| Scan images -> PDF | `../../pdf craft/crates/create` (`pdfcraft-create`) | blank docs; PNG/JPEG/TIFF/docx -> PDF (png 0.18, tiff 0.11, image 0.25) |
| Searchable scans | `../../pdf craft/crates/ocr` (`pdfcraft-ocr`) | ocrs+rten text recognition + invisible text layer |
| Page preview | `../../pdf craft/crates/render` (`pdfcraft-render`) | hayro-backed rendering + doc inspection (bootstrap; being replaced by cos/model per CLAUDE.md) |
| Open/save flow | `../../pdf craft/crates/engine` (`pdfcraft-engine`) | session, documents, command/tool catalogue (wraps organize/annot/forms/edit/redact/print/ocr/...) |
| PDF model | `../../pdf craft/crates/model`, `../../pdf craft/crates/cos`, `../../pdf craft/crates/content` | object graph (model), low-level objects (cos), content streams |
| OS services | `../../pdf craft/crates/platform` (`pdfcraft-platform`) | shared desktop/automation OS services |
| Design system | `../../pdf craft/crates/ui-egui` (`pdfcraft-ui-egui`) | egui theme/palette/icons/widgets + `print_ui`, `ocr_ui`, `create_ui` panels |
| Errors/logs | `thiserror 2`, `log 0.4`, `serde/serde_json 1` | same versions as parent workspace |

Hardware acquisition (WIA/TWAIN on Windows, SANE on Linux, ICA on macOS)
is NOT in PdfCraft — that dependency gets added when your working
instructions pick the platform(s) and source(s). Leave a slot for it in
`src/scan.rs`.
