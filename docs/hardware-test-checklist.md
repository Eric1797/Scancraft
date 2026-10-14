# Hardware test checklist (human-run; everything here is UNVERIFIED-ON-HARDWARE)

> No physical scanner is available to the agent. All backend behavior is
> proven against `scan-mock` (+ a Windows WIA virtual/test device where
> possible). Each item below must be run by the human before the related
> milestone is declared done. Mark `[x]` with date + device when passed.

## WIA (M3)

- [ ] Flatbed color 300 dpi single page — page arrives, DPI/size correct
- [ ] Feeder 10 pages — all pages arrive in order, no loss
- [ ] Duplex run — front/back pairing correct; flip-duplex-pages option
- [ ] Cancel mid-scan — transfer aborts cleanly, partial pages kept, no hang
- [ ] Unplug mid-scan — `Io`/driver error surfaced, app stays alive
- [ ] Empty feeder at start — `PaperEmpty` error with action hint
- [ ] Feeder runs dry at end — treated as success, pages kept
- [ ] Two scanners attached — selection sticks to stable device id
- [ ] 600 dpi A4 color — memory stays bounded (session cache, no full-RAM hold)
- [ ] Driver with no usable WIA 2.0 path — fallback behavior documented

## eSCL (M6)

- [ ] mDNS discovery finds network scanner
- [ ] Scan job create → pages → delete-job on cancel
- [ ] ADF + duplex over network
- [ ] Busy (4xx/5xx) mapped to retry-later message
- [ ] Self-signed printer cert — explicit per-device opt-in only

## TWAIN (M7, behind setting until all pass)

- [ ] 64-bit app + 32-bit driver via worker process
- [ ] Worker crash does not take down the app
- [ ] Native UI (driver dialog) mode
- [ ] Silent/no-UI mode

## General (every milestone touching hardware)

- [ ] No `// UNVERIFIED-ON-HARDWARE` marker left unlisted here
- [ ] Error toasts checked on real errors (no-device, busy, jam, cover open)
- [ ] Recovery file survives a kill during an active scan
