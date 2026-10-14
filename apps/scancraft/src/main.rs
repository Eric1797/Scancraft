//! ScanCraft desktop shell (M1): home screen, tool panels, page grid,
//! inspector, status bar, mock scanning on a worker thread.
//! Tabs, drag-reorder, command palette and `--control` arrive in M1b.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use scancraft_engine::{DocId, Session};
use scancraft_model::{EditOp, Page, Project, ScanProfile};
use scancraft_scan_core::{CancelToken, PageSink, Progress, RawPage, ScanError, ScanRequest, ScannerBackend};
use scancraft_scan_mock::MockScanner;
use scancraft_ui_kit::{ThemeKind, ThemePreference};

/// Messages from the scan worker thread to the UI thread.
enum ScanMsg {
    Progress(Progress),
    Page(RawPage),
    Finished(Result<u32, ScanError>),
}

struct WorkerSink {
    tx: mpsc::Sender<ScanMsg>,
}

impl PageSink for WorkerSink {
    fn page(&mut self, page: RawPage) -> Result<(), ScanError> {
        self.tx.send(ScanMsg::Page(page)).map_err(|_| ScanError::Cancelled)?;
        Ok(())
    }

    fn progress(&mut self, progress: Progress) {
        let _ = self.tx.send(ScanMsg::Progress(progress));
    }
}

struct ScannerApp {
    session: Session,
    doc: DocId,
    profile: ScanProfile,
    backend: MockScanner,
    theme_pref: ThemePreference,
    theme_applied: ThemeKind,
    selection: BTreeSet<u64>,
    /// As-scanned pixels per page (edits re-render from these).
    originals: HashMap<u64, egui::ColorImage>,
    textures: HashMap<u64, egui::TextureHandle>,
    next_page_id: u64,
    scan_rx: Option<Receiver<ScanMsg>>,
    scan_cancel: Option<CancelToken>,
    scan_progress: Option<Progress>,
    scan_error: Option<String>,
    status: String,
}

impl ScannerApp {
    fn new() -> Self {
        let mut session = Session::new();
        let doc = session.open(Project { title: "Untitled".to_owned(), pages: Vec::new() });
        let profile = ScanProfile { name: "Default (Demo scanner)".to_owned(), device_id: "mock:demo-scanner".to_owned(), ..ScanProfile::default() };
        Self {
            session,
            doc,
            profile,
            backend: MockScanner { pages: 4, ..Default::default() },
            theme_pref: ThemePreference::System,
            theme_applied: ThemeKind::Light,
            selection: BTreeSet::new(),
            originals: HashMap::new(),
            textures: HashMap::new(),
            next_page_id: 1,
            scan_rx: None,
            scan_cancel: None,
            scan_progress: None,
            scan_error: None,
            status: "Ready.".to_owned(),
        }
    }

    fn scanning(&self) -> bool {
        self.scan_rx.is_some()
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        if self.scanning() {
            return;
        }
        let mut ignored = Vec::new();
        let request = match ScanRequest::from_profile(&self.profile, &mut ignored) {
            Ok(r) => r,
            Err(e) => {
                self.scan_error = Some(e.message());
                return;
            }
        };
        self.scan_error = None;
        self.scan_progress = Some(Progress { pages_done: 0, stage: "starting" });
        let cancel = CancelToken::new();
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        self.scan_cancel = Some(cancel.clone());
        let backend = self.backend.clone();
        ctx.request_repaint();
        std::thread::spawn(move || {
            // COM/WIA rule preview: ALL driver calls stay on this one worker
            // thread, never on the UI thread.
            let mut sink = WorkerSink { tx: tx.clone() };
            let result = backend.scan(&request, &mut sink, &cancel);
            let _ = tx.send(ScanMsg::Finished(result));
        });
    }

    fn cancel_scan(&mut self) {
        if let Some(cancel) = &self.scan_cancel {
            cancel.cancel();
        }
        self.status = "Cancelling…".to_owned();
    }

    /// Drain worker messages.
    fn poll_scan(&mut self, ctx: &egui::Context) {
        while self.scan_rx.is_some() {
            let Some(rx) = &self.scan_rx else { break };
            let msg = match rx.try_recv() {
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.scan_rx = None;
                    self.scan_cancel = None;
                    self.scan_progress = None;
                    break;
                }
                Ok(msg) => msg,
            };
            match msg {
                ScanMsg::Progress(p) => self.scan_progress = Some(p),
                ScanMsg::Page(raw) => self.add_scanned_page(ctx, raw),
                ScanMsg::Finished(result) => {
                    self.scan_rx = None;
                    self.scan_cancel = None;
                    self.scan_progress = None;
                    match result {
                        Ok(n) => self.status = format!("Scan finished: {n} page(s)."),
                        Err(ScanError::Cancelled) => self.status = "Scan cancelled.".to_owned(),
                        Err(e) => self.scan_error = Some(e.message()),
                    }
                    break;
                }
            }
        }
    }

    fn add_scanned_page(&mut self, ctx: &egui::Context, raw: RawPage) {
        let id = self.next_page_id;
        self.next_page_id += 1;
        let w = raw.width_px as usize;
        let h = raw.height_px as usize;
        let image = egui::ColorImage::from_rgb([w, h], &raw.pixels_rgb8);
        self.originals.insert(id, image.clone());
        self.textures.insert(id, ctx.load_texture(format!("page-{id}"), image, egui::TextureOptions::LINEAR));
        let page = Page {
            id,
            width_px: raw.width_px,
            height_px: raw.height_px,
            dpi: raw.dpi,
            bit_depth: raw.bit_depth,
            side: raw.side,
            storage_key: format!("scan-{id:06}"),
            edits: Vec::new(),
        };
        if let Ok(doc) = self.session.get_mut(self.doc)
            && doc.append_pages(vec![page], "Scan pages").is_ok()
        {
            self.selection.clear();
            self.selection.insert(id);
        }
    }

    /// Re-render a page's texture from its original + edit stack.
    /// M1 covers rotate/flip; tonal edits arrive with imaging (M2).
    fn refresh_texture(&mut self, ctx: &egui::Context, id: u64) {
        let found = self.session.get(self.doc).ok().and_then(|doc| doc.project().pages.iter().find(|p| p.id == id).cloned());
        let (original, edits) = match (self.originals.get(&id).cloned(), found) {
            (Some(o), Some(p)) => (o, p.edits),
            _ => return,
        };
        let mut image = original;
        for edit in &edits {
            match edit {
                EditOp::RotateCw => image = rotate_cw(&image),
                EditOp::RotateCcw => image = rotate_ccw(&image),
                EditOp::FlipHorizontal => flip_horizontal(&mut image),
                EditOp::FlipVertical => flip_vertical(&mut image),
                _ => {}
            }
        }
        self.textures.insert(id, ctx.load_texture(format!("page-{id}"), image, egui::TextureOptions::LINEAR));
    }

    fn apply_edit(&mut self, ctx: &egui::Context, id: u64, edit: EditOp, label: &str) {
        let ok = self.session.get_mut(self.doc).map(|doc| doc.record_edit(id, edit, label)).is_ok();
        if ok {
            self.refresh_texture(ctx, id);
            self.status = format!("{label} (page {id}).");
        }
    }

    fn reset_page(&mut self, ctx: &egui::Context, id: u64) {
        let ok = self.session.get_mut(self.doc).map(|doc| doc.clear_edits(id)).is_ok();
        if ok {
            self.refresh_texture(ctx, id);
            self.status = format!("Reset page {id}.");
        }
    }

    fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids: Vec<u64> = self.selection.iter().copied().collect();
        let label = self.session.get_mut(self.doc).and_then(|doc| doc.delete_pages(&ids));
        match label {
            Ok(done) => {
                for id in &ids {
                    self.originals.remove(id);
                    self.textures.remove(id);
                }
                self.selection.clear();
                self.status = format!("{done}.");
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let delete = ctx.input(|i| i.key_pressed(egui::Key::Delete));
        if delete {
            self.delete_selection();
        }
        let ctrl = ctx.input(|i| i.modifiers.ctrl);
        if ctrl
            && ctx.input(|i| i.key_pressed(egui::Key::Z))
            && let Ok(d) = self.session.get_mut(self.doc)
        {
            d.undo();
        }
        if ctrl
            && ctx.input(|i| i.key_pressed(egui::Key::Y))
            && let Ok(d) = self.session.get_mut(self.doc)
        {
            d.redo();
        }
        if ctrl
            && ctx.input(|i| i.key_pressed(egui::Key::A))
            && let Ok(d) = self.session.get(self.doc)
        {
            self.selection = d.project().pages.iter().map(|p| p.id).collect();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selection.clear();
        }
    }
}

fn rotate_cw(src: &egui::ColorImage) -> egui::ColorImage {
    let [w, h] = src.size;
    let mut out = egui::ColorImage::new([h, w], vec![egui::Color32::BLACK; w * h]);
    for y in 0..h {
        for x in 0..w {
            if let (Some(px), Some(slot)) = (src.pixels.get(y * w + x), out.pixels.get_mut(x * h + (h - 1 - y))) {
                *slot = *px;
            }
        }
    }
    out
}

fn rotate_ccw(src: &egui::ColorImage) -> egui::ColorImage {
    rotate_cw(&rotate_cw(&rotate_cw(src)))
}

fn flip_horizontal(image: &mut egui::ColorImage) {
    let [w, h] = image.size;
    for y in 0..h {
        for x in 0..w / 2 {
            let (a, b) = (y * w + x, y * w + (w - 1 - x));
            if a < image.pixels.len() && b < image.pixels.len() {
                image.pixels.swap(a, b);
            }
        }
    }
}

fn flip_vertical(image: &mut egui::ColorImage) {
    let [w, h] = image.size;
    for y in 0..h / 2 {
        for x in 0..w {
            let (a, b) = (y * w + x, (h - 1 - y) * w + x);
            if a < image.pixels.len() && b < image.pixels.len() {
                image.pixels.swap(a, b);
            }
        }
    }
}

fn tool_group(ui: &mut egui::Ui, title: &str, tools: &[(&str, bool, &str)]) {
    ui.collapsing(title, |ui| {
        for (label, ready, milestone) in tools {
            ui.add_enabled_ui(*ready, |ui| {
                let mut text = (*label).to_owned();
                if !ready {
                    text.push_str(&format!("  [{milestone}]"));
                }
                let _ = ui.button(text);
            });
        }
    });
}

impl eframe::App for ScannerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Theme follows the saved preference (system by default).
        let kind = self.theme_pref.resolve(Some(ctx.theme()), ThemeKind::Light);
        if kind != self.theme_applied {
            scancraft_ui_kit::apply(&ctx, kind);
            self.theme_applied = kind;
        }

        self.poll_scan(&ctx);
        if self.scanning() {
            ctx.request_repaint();
        }
        self.handle_shortcuts(&ctx);

        let (can_undo, can_redo, undo_label, page_count) = match self.session.get(self.doc) {
            Ok(d) => (d.can_undo(), d.can_redo(), d.undo_label().unwrap_or("").to_owned(), d.project().pages.len()),
            Err(_) => (false, false, String::new(), 0),
        };

        // ---- Top bar ----
        egui::Panel::top("topbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let scanning = self.scanning();
                ui.add_enabled_ui(!scanning, |ui| {
                    if ui.button("⬇ Scan").clicked() {
                        self.start_scan(&ctx);
                    }
                });
                if scanning {
                    if ui.button("✖ Cancel").clicked() {
                        self.cancel_scan();
                    }
                    if let Some(p) = &self.scan_progress {
                        ui.label(format!("Scanning… {} page(s)", p.pages_done));
                    }
                }
                ui.separator();
                if ui.add_enabled(can_undo, egui::Button::new("↩ Undo")).clicked()
                    && let Ok(d) = self.session.get_mut(self.doc)
                {
                    d.undo();
                }
                if ui.add_enabled(can_redo, egui::Button::new("↪ Redo")).clicked()
                    && let Ok(d) = self.session.get_mut(self.doc)
                {
                    d.redo();
                }
                if !undo_label.is_empty() {
                    ui.label(egui::RichText::new(format!("({undo_label})")).small().weak());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_label("").selected_text(format!("Theme: {:?}", self.theme_pref)).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.theme_pref, ThemePreference::System, "System");
                        ui.selectable_value(&mut self.theme_pref, ThemePreference::Light, "Light");
                        ui.selectable_value(&mut self.theme_pref, ThemePreference::Dark, "Dark");
                    });
                });
            });
        });

        // ---- Status bar ----
        egui::Panel::bottom("statusbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Demo scanner (mock) · {} · {} dpi", self.profile.name, self.profile.dpi));
                ui.separator();
                ui.label(format!("{} page(s)", page_count));
                ui.label(format!("{} selected", self.selection.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(&self.status);
                });
            });
        });

        // ---- Tools (left) ----
        egui::Panel::left("tools").show(ui, |ui| {
            ui.heading("Tools");
            tool_group(ui, "Scan", &[("Scan", true, ""), ("Scan again", true, ""), ("Batch scan", false, "M4"), ("Profiles…", false, "M4")]);
            tool_group(
                ui,
                "Pages",
                &[("Delete", true, ""), ("Reverse order", true, ""), ("Interleave", false, "M2"), ("Import files…", false, "M2")],
            );
            tool_group(ui, "Image", &[("Rotate / flip", true, ""), ("Crop", false, "M2"), ("Deskew / auto-crop", false, "M2")]);
            tool_group(ui, "Export", &[("Save PDF", false, "M2"), ("Save TIFF / JPEG / PNG", false, "M2"), ("Print", false, "M8")]);
            tool_group(ui, "OCR", &[("Searchable PDF", false, "M5")]);
        });

        // ---- Inspector (right) ----
        egui::Panel::right("inspector").show(ui, |ui| {
            ui.heading("Inspector");
            let selected: Vec<u64> = self.selection.iter().copied().collect();
            if selected.len() == 1 {
                let id = selected[0];
                let info = self.session.get(self.doc).ok().and_then(|d| d.project().pages.iter().find(|p| p.id == id).cloned());
                if let Some(page) = info {
                    ui.label(format!("Page {id}"));
                    ui.label(format!("{}×{} px @ {} dpi", page.width_px, page.height_px, page.dpi));
                    ui.label(format!("{:?} · {:?} · {} edit(s)", page.bit_depth, page.side, page.edits.len()));
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("⟲ Left").clicked() {
                            self.apply_edit(&ctx, id, EditOp::RotateCcw, "Rotate left");
                        }
                        if ui.button("⟳ Right").clicked() {
                            self.apply_edit(&ctx, id, EditOp::RotateCw, "Rotate right");
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("⇋ Flip H").clicked() {
                            self.apply_edit(&ctx, id, EditOp::FlipHorizontal, "Flip horizontal");
                        }
                        if ui.button("⇅ Flip V").clicked() {
                            self.apply_edit(&ctx, id, EditOp::FlipVertical, "Flip vertical");
                        }
                    });
                    if ui.button("Reset page").clicked() {
                        self.reset_page(&ctx, id);
                    }
                    if ui.button("Delete page").clicked() {
                        self.delete_selection();
                    }
                }
            } else if selected.is_empty() {
                ui.label(format!("{page_count} page(s) in document."));
                ui.label("Select a page to adjust it.");
            } else {
                ui.label(format!("{} pages selected.", selected.len()));
                if ui.button("Delete selected").clicked() {
                    self.delete_selection();
                }
            }
        });

        // ---- Canvas (center) ----
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(err) = &self.scan_error {
                ui.colored_label(egui::Color32::RED, err);
            }
            if page_count == 0 && !self.scanning() {
                // Home / empty state.
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.heading("Welcome to ScanCraft");
                    ui.label("Documents never leave your machine. No account. No telemetry.");
                    ui.add_space(16.0);
                    if ui.button("⬇ Scan with Demo scanner").clicked() {
                        self.start_scan(&ctx);
                    }
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Import files and export arrive in M2.").small().weak());
                });
                return;
            }
            // Collect clicks first (avoids borrow conflicts with texture use).
            let mut clicked: Option<(u64, bool)> = None;
            egui::ScrollArea::both().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let ids: Vec<(u64, u32)> =
                        self.session.get(self.doc).map(|d| d.project().pages.iter().map(|p| (p.id, p.dpi)).collect()).unwrap_or_default();
                    for (id, dpi) in ids {
                        let selected = self.selection.contains(&id);
                        let frame = if selected { egui::Frame::NONE.fill(ui.visuals().selection.bg_fill) } else { egui::Frame::NONE };
                        frame.show(ui, |ui| {
                            ui.vertical(|ui| {
                                if let Some(tex) = self.textures.get(&id) {
                                    let max = 160.0;
                                    let size = tex.size_vec2();
                                    let scale = (max / size.x).min(max / size.y).min(1.0);
                                    let img = egui::Image::new(egui::load::SizedTexture::new(tex.id(), size * scale));
                                    if ui.add(img).clicked() {
                                        let ctrl = ui.ctx().input(|i| i.modifiers.ctrl);
                                        clicked = Some((id, ctrl));
                                    }
                                }
                                ui.label(egui::RichText::new(format!("#{id}  {dpi} dpi")).small());
                            });
                        });
                    }
                });
            });
            if let Some((id, ctrl)) = clicked {
                if ctrl {
                    if !self.selection.remove(&id) {
                        self.selection.insert(id);
                    }
                } else {
                    self.selection.clear();
                    self.selection.insert(id);
                }
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("scancraft [--help] [--version]\n\nScanCraft desktop app (M1 shell: mock scanning + page grid).");
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("scancraft {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 750.0]).with_min_inner_size([800.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ScanCraft",
        options,
        Box::new(|cc| {
            scancraft_ui_kit::install_fonts(&cc.egui_ctx);
            scancraft_ui_kit::apply(&cc.egui_ctx, ThemeKind::Light);
            Ok(Box::new(ScannerApp::new()))
        }),
    )
}
