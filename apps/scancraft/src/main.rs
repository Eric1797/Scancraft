//! ScanCraft desktop shell (M1b): tabs, command palette, drag-reorder,
//! single-page view, mock scanning on a worker thread.
//! `--control` UI channel, screenshots and crop overlay arrive in M1c/M2.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use scancraft_engine::commands::COMMANDS;
use scancraft_engine::{DocId, Session};
use scancraft_export::{PdfMeta, PdfPage, Stamp, TiffPage, render_filename, unique_path, write_atomic, write_jpeg, write_pdf, write_png, write_tiff};
use scancraft_imaging::Image as ScanImage;
use scancraft_model::{EditOp, Page, Project, ScanProfile};
use scancraft_scan_core::{CancelToken, PageSink, Progress, RawPage, ScanError, ScanRequest, ScannerBackend};
use scancraft_scan_mock::MockScanner;
use scancraft_ui_kit::{ThemeKind, ThemePreference, palette};

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Grid,
    Single,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveFormat {
    Pdf,
    Tiff,
    Jpeg,
    Png,
}

impl SaveFormat {
    fn ext(self) -> &'static str {
        match self {
            SaveFormat::Pdf => "pdf",
            SaveFormat::Tiff => "tiff",
            SaveFormat::Jpeg => "jpg",
            SaveFormat::Png => "png",
        }
    }

    fn label(self) -> &'static str {
        match self {
            SaveFormat::Pdf => "PDF",
            SaveFormat::Tiff => "TIFF",
            SaveFormat::Jpeg => "JPEG",
            SaveFormat::Png => "PNG",
        }
    }
}

struct SaveDialog {
    open: bool,
    format: SaveFormat,
    jpeg_quality: i32,
    pattern: String,
    folder: String,
    title: String,
    author: String,
    subject: String,
    keywords: String,
    selected_only: bool,
}

impl Default for SaveDialog {
    fn default() -> Self {
        Self {
            open: false,
            format: SaveFormat::Pdf,
            jpeg_quality: 85,
            pattern: "$(YYYY)-$(MM)-$(DD) $(n)".to_owned(),
            folder: default_folder(),
            title: String::new(),
            author: String::new(),
            subject: String::new(),
            keywords: String::new(),
            selected_only: false,
        }
    }
}

fn default_folder() -> String {
    std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// One fully-rendered page ready for the save worker.
struct RenderedPage {
    image: ScanImage,
    dpi: u32,
}

enum SaveMsg {
    Done(Result<String, String>),
}

struct ScannerApp {
    session: Session,
    docs: Vec<DocId>,
    active: usize,
    profile: ScanProfile,
    backend: MockScanner,
    theme_pref: ThemePreference,
    theme_applied: ThemeKind,
    view: View,
    zoom: f32,
    selection: BTreeSet<u64>,
    /// As-scanned pixels per page (edits re-render from these).
    originals: HashMap<u64, egui::ColorImage>,
    textures: HashMap<u64, egui::TextureHandle>,
    next_page_id: u64,
    scan_rx: Option<Receiver<ScanMsg>>,
    scan_cancel: Option<CancelToken>,
    scan_progress: Option<Progress>,
    scan_error: Option<String>,
    palette_open: bool,
    palette_query: String,
    dragging: Option<u64>,
    save: SaveDialog,
    save_rx: Option<Receiver<SaveMsg>>,
    status: String,
}

impl ScannerApp {
    fn new() -> Self {
        let mut session = Session::new();
        let doc = session.open(Project { title: "Untitled".to_owned(), pages: Vec::new() });
        let profile = ScanProfile { name: "Default (Demo scanner)".to_owned(), device_id: "mock:demo-scanner".to_owned(), ..ScanProfile::default() };
        Self {
            session,
            docs: vec![doc],
            active: 0,
            profile,
            backend: MockScanner { pages: 4, ..Default::default() },
            theme_pref: ThemePreference::System,
            theme_applied: ThemeKind::Light,
            view: View::Grid,
            zoom: 1.0,
            selection: BTreeSet::new(),
            originals: HashMap::new(),
            textures: HashMap::new(),
            next_page_id: 1,
            scan_rx: None,
            scan_cancel: None,
            scan_progress: None,
            scan_error: None,
            palette_open: false,
            palette_query: String::new(),
            dragging: None,
            save: SaveDialog::default(),
            save_rx: None,
            status: "Ready.".to_owned(),
        }
    }

    fn doc(&self) -> DocId {
        self.docs[self.active]
    }

    fn page_count(&self) -> usize {
        self.session.get(self.doc()).map(|d| d.project().pages.len()).unwrap_or(0)
    }

    fn scanning(&self) -> bool {
        self.scan_rx.is_some()
    }

    // ---------- documents / tabs ----------

    fn new_doc(&mut self) {
        let id = self.session.open(Project { title: "Untitled".to_owned(), pages: Vec::new() });
        self.docs.push(id);
        self.active = self.docs.len() - 1;
        self.selection.clear();
        self.status = "New document.".to_owned();
    }

    fn close_doc(&mut self) {
        let id = self.doc();
        let dirty = self.session.get(id).map(|d| d.dirty()).unwrap_or(false);
        let _ = self.session.close(id);
        self.docs.retain(|d| *d != id);
        if self.docs.is_empty() {
            let fresh = self.session.open(Project { title: "Untitled".to_owned(), pages: Vec::new() });
            self.docs.push(fresh);
        }
        self.active = self.active.min(self.docs.len() - 1);
        self.selection.clear();
        self.status = if dirty { "Closed document (unsaved changes discarded).".to_owned() } else { "Closed document.".to_owned() };
    }

    fn switch_tab(&mut self, index: usize) {
        if index < self.docs.len() {
            self.active = index;
            self.selection.clear();
        }
    }

    // ---------- scanning (worker thread) ----------

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
        if let Ok(doc) = self.session.get_mut(self.doc())
            && doc.append_pages(vec![page], "Scan pages").is_ok()
        {
            self.selection.clear();
            self.selection.insert(id);
        }
    }

    // ---------- edits ----------

    /// Re-render a page's texture from its original + edit stack.
    /// M1 covers rotate/flip; tonal edits arrive with imaging (M2).
    fn refresh_texture(&mut self, ctx: &egui::Context, id: u64) {
        let found = self.session.get(self.doc()).ok().and_then(|doc| doc.project().pages.iter().find(|p| p.id == id).cloned());
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
        let ok = self.session.get_mut(self.doc()).map(|doc| doc.record_edit(id, edit, label)).is_ok();
        if ok {
            self.refresh_texture(ctx, id);
            self.status = format!("{label} (page {id}).");
        }
    }

    fn apply_edit_to_selection(&mut self, ctx: &egui::Context, edit: EditOp, label: &str) {
        let ids: Vec<u64> = self.selection.iter().copied().collect();
        let n = ids.len();
        for id in ids {
            if self.session.get_mut(self.doc()).map(|doc| doc.record_edit(id, edit.clone(), label)).is_ok() {
                self.refresh_texture(ctx, id);
            }
        }
        // Collapse the per-page commits into one undo step label is M2 work;
        // for now each page is its own step (honest limitation, Undo undoes one page).
        self.status = format!("{label} ({n} page(s)).");
    }

    fn reset_selection(&mut self, ctx: &egui::Context) {
        let ids: Vec<u64> = self.selection.iter().copied().collect();
        for id in ids {
            if self.session.get_mut(self.doc()).map(|doc| doc.clear_edits(id)).is_ok() {
                self.refresh_texture(ctx, id);
            }
        }
        self.status = "Reset selected pages.".to_owned();
    }

    fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids: Vec<u64> = self.selection.iter().copied().collect();
        let label = self.session.get_mut(self.doc()).and_then(|doc| doc.delete_pages(&ids));
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

    fn reorder(&mut self, id: u64, to: usize) {
        if self.session.get_mut(self.doc()).map(|doc| doc.move_page(id, to)).is_ok() {
            self.status = format!("Moved page {id}.");
        }
    }

    // ---------- export ----------

    fn open_save(&mut self, format: SaveFormat) {
        if self.page_count() == 0 {
            self.status = "Nothing to save: the document is empty.".to_owned();
            return;
        }
        self.save.format = format;
        self.save.open = true;
    }

    /// Render selected (or all) pages through their edit stacks. Done on the
    /// UI thread: mock pages are small; full-size rendering moves off-thread
    /// with the session cache (M2b).
    fn collect_rendered_pages(&self) -> Result<Vec<RenderedPage>, String> {
        let doc = self.session.get(self.doc()).map_err(|e| e.to_string())?;
        let use_selected = self.save.selected_only && !self.selection.is_empty();
        let mut out = Vec::new();
        for page in &doc.project().pages {
            if use_selected && !self.selection.contains(&page.id) {
                continue;
            }
            let original = self.originals.get(&page.id).ok_or_else(|| format!("missing pixels for page {}", page.id))?;
            let base = colorimage_to_imaging(original).ok_or_else(|| format!("bad pixels for page {}", page.id))?;
            let rendered = base.apply_edits(&page.edits).map_err(|e| e.to_string())?;
            out.push(RenderedPage { image: rendered, dpi: page.dpi });
        }
        if out.is_empty() {
            return Err("No pages match the current selection.".to_owned());
        }
        Ok(out)
    }

    fn do_save(&mut self) {
        let pages = match self.collect_rendered_pages() {
            Ok(p) => p,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let dir = std::path::PathBuf::from(&self.save.folder);
        if !dir.is_dir() {
            self.status = format!("Folder does not exist: {}", dir.display());
            return;
        }
        let format = self.save.format;
        let pattern = self.save.pattern.clone();
        let quality = self.save.jpeg_quality.clamp(0, 100) as u8;
        let meta = PdfMeta {
            title: self.save.title.clone(),
            author: self.save.author.clone(),
            subject: self.save.subject.clone(),
            keywords: self.save.keywords.clone(),
        };
        let stamp = Stamp::now();
        let (tx, rx) = mpsc::channel();
        self.save_rx = Some(rx);
        self.save.open = false;
        self.status = "Saving…".to_owned();
        std::thread::spawn(move || {
            let result = save_worker(&dir, &pattern, stamp, format, quality, &meta, pages);
            let _ = tx.send(SaveMsg::Done(result));
        });
    }

    fn poll_save(&mut self) {
        let done = match &self.save_rx {
            Some(rx) => match rx.try_recv() {
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("Save worker went away.".to_owned())),
                Ok(SaveMsg::Done(r)) => Some(r),
            },
            None => None,
        };
        if let Some(result) = done {
            self.save_rx = None;
            match result {
                Ok(path) => {
                    if let Ok(doc) = self.session.get_mut(self.doc()) {
                        doc.mark_saved();
                    }
                    self.status = format!("Saved to {path}.");
                }
                Err(e) => self.status = format!("Save failed: {e}."),
            }
        }
    }

    fn show_save_dialog(&mut self, ctx: &egui::Context) {
        if !self.save.open {
            return;
        }
        let mut close = false;
        let mut do_browse = false;
        let mut do_save = false;
        egui::Window::new("Save / Export").collapsible(false).resizable(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for fmt in [SaveFormat::Pdf, SaveFormat::Tiff, SaveFormat::Jpeg, SaveFormat::Png] {
                    if ui.selectable_label(self.save.format == fmt, fmt.label()).clicked() {
                        self.save.format = fmt;
                    }
                }
            });
            ui.separator();
            match self.save.format {
                SaveFormat::Pdf => {
                    ui.label("Metadata (optional)");
                    ui.horizontal(|ui| {
                        ui.label("Title");
                        ui.text_edit_singleline(&mut self.save.title);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Author");
                        ui.text_edit_singleline(&mut self.save.author);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Subject");
                        ui.text_edit_singleline(&mut self.save.subject);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Keywords");
                        ui.text_edit_singleline(&mut self.save.keywords);
                    });
                    ui.label(egui::RichText::new("Encryption and the OCR text layer arrive in later milestones.").small().weak());
                }
                SaveFormat::Jpeg => {
                    ui.horizontal(|ui| {
                        ui.label("Quality");
                        ui.add(egui::Slider::new(&mut self.save.jpeg_quality, 1..=100));
                    });
                }
                SaveFormat::Tiff => {
                    ui.label("Multipage TIFF, uncompressed. Compression options arrive later.");
                }
                SaveFormat::Png => {
                    ui.label("One PNG per page (lossless).");
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Folder");
                ui.text_edit_singleline(&mut self.save.folder);
                if ui.button("Browse…").clicked() {
                    do_browse = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.save.pattern);
            });
            ui.label(
                egui::RichText::new("Tokens: $(YYYY) $(MM) $(DD) $(hh) $(mm) $(ss) $(n). Never overwrites: a counter is appended.").small().weak(),
            );
            ui.checkbox(&mut self.save.selected_only, "Selected pages only");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.button("Save").clicked() {
                    do_save = true;
                }
            });
        });
        if do_browse && let Some(folder) = rfd::FileDialog::new().pick_folder() {
            self.save.folder = folder.to_string_lossy().into_owned();
        }
        if close {
            self.save.open = false;
        }
        if do_save {
            self.do_save();
        }
    }

    // ---------- commands ----------

    fn command_ready(&self, id: &str) -> bool {
        let pages = self.page_count();
        let selected = !self.selection.is_empty();
        match id {
            "scan" => !self.scanning(),
            "undo" => self.session.get(self.doc()).map(|d| d.can_undo()).unwrap_or(false),
            "redo" => self.session.get(self.doc()).map(|d| d.can_redo()).unwrap_or(false),
            "select-all" => pages > 0,
            "clear-selection" => selected,
            "delete-pages" | "rotate-left" | "rotate-right" | "flip-h" | "flip-v" | "reset-page" => selected,
            "reverse-order" | "interleave" | "deinterleave" => pages >= 2,
            "save-pdf" | "save-tiff" | "save-jpeg" | "save-png" => pages > 0,
            "new-doc" | "close-doc" | "next-tab" | "prev-tab" | "toggle-view" | "toggle-theme" | "open-palette" => true,
            _ => false, // M4/M5/M8 work
        }
    }

    fn run_command(&mut self, ctx: &egui::Context, id: &str) {
        match id {
            "scan" => self.start_scan(ctx),
            "undo" => {
                if let Ok(d) = self.session.get_mut(self.doc()) {
                    d.undo();
                }
            }
            "redo" => {
                if let Ok(d) = self.session.get_mut(self.doc()) {
                    d.redo();
                }
            }
            "new-doc" => self.new_doc(),
            "close-doc" => self.close_doc(),
            "next-tab" => self.switch_tab((self.active + 1) % self.docs.len()),
            "prev-tab" => self.switch_tab((self.active + self.docs.len() - 1) % self.docs.len()),
            "toggle-view" => {
                self.view = if self.view == View::Grid { View::Single } else { View::Grid };
            }
            "toggle-theme" => {
                self.theme_pref = match self.theme_pref {
                    ThemePreference::Light => ThemePreference::Dark,
                    _ => ThemePreference::Light,
                };
            }
            "select-all" => {
                if let Ok(d) = self.session.get(self.doc()) {
                    self.selection = d.project().pages.iter().map(|p| p.id).collect();
                }
            }
            "clear-selection" => self.selection.clear(),
            "delete-pages" => self.delete_selection(),
            "reverse-order" => {
                if let Ok(d) = self.session.get_mut(self.doc()) {
                    d.reverse_order();
                    self.status = "Reversed page order.".to_owned();
                }
            }
            "interleave" => {
                let r = self.session.get_mut(self.doc()).and_then(|d| d.interleave());
                self.status = r.map(|_| "Interleaved pages.".to_owned()).unwrap_or_else(|e| e.to_string());
            }
            "deinterleave" => {
                let r = self.session.get_mut(self.doc()).and_then(|d| d.deinterleave());
                self.status = r.map(|_| "De-interleaved pages.".to_owned()).unwrap_or_else(|e| e.to_string());
            }
            "rotate-left" => self.apply_edit_to_selection(ctx, EditOp::RotateCcw, "Rotate left"),
            "rotate-right" => self.apply_edit_to_selection(ctx, EditOp::RotateCw, "Rotate right"),
            "flip-h" => self.apply_edit_to_selection(ctx, EditOp::FlipHorizontal, "Flip horizontal"),
            "flip-v" => self.apply_edit_to_selection(ctx, EditOp::FlipVertical, "Flip vertical"),
            "reset-page" => self.reset_selection(ctx),
            "save-pdf" => self.open_save(SaveFormat::Pdf),
            "save-tiff" => self.open_save(SaveFormat::Tiff),
            "save-jpeg" => self.open_save(SaveFormat::Jpeg),
            "save-png" => self.open_save(SaveFormat::Png),
            "batch-scan" | "edit-profiles" => self.status = "Arrives in M4.".to_owned(),
            "ocr-run" => self.status = "OCR arrives in M5.".to_owned(),
            "open-palette" => self.palette_open = true,
            _ => self.status = format!("Unknown command: {id}."),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.palette_open {
            return; // palette owns Esc/Enter
        }
        let ctrl = ctx.input(|i| i.modifiers.ctrl);
        if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.delete_selection();
        }
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::K)) {
            self.palette_open = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.start_scan(ctx);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F6)) {
            self.view = if self.view == View::Grid { View::Single } else { View::Grid };
        }
        if ctrl
            && ctx.input(|i| i.key_pressed(egui::Key::Z))
            && let Ok(d) = self.session.get_mut(self.doc())
        {
            d.undo();
        }
        if ctrl
            && ctx.input(|i| i.key_pressed(egui::Key::Y))
            && let Ok(d) = self.session.get_mut(self.doc())
        {
            d.redo();
        }
        if ctrl
            && ctx.input(|i| i.key_pressed(egui::Key::A))
            && let Ok(d) = self.session.get(self.doc())
        {
            self.selection = d.project().pages.iter().map(|p| p.id).collect();
        }
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::N)) {
            self.new_doc();
        }
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::W)) {
            self.close_doc();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selection.clear();
        }
    }
}

/// Worker-thread export: filenames are planned here (never overwrites —
/// `unique_path` appends a counter) and every write is atomic.
fn save_worker(
    dir: &std::path::Path,
    pattern: &str,
    stamp: Stamp,
    format: SaveFormat,
    quality: u8,
    meta: &PdfMeta,
    pages: Vec<RenderedPage>,
) -> Result<String, String> {
    match format {
        SaveFormat::Pdf => {
            let pdf_pages: Vec<PdfPage> = pages.into_iter().map(|p| PdfPage { image: p.image, dpi: p.dpi }).collect();
            let bytes = write_pdf(pdf_pages, meta).map_err(|e| e.to_string())?;
            let stem = render_filename(pattern, stamp, 1).map_err(|e| e.to_string())?;
            let path = unique_path(dir, &stem, "pdf", 10_000).map_err(|e| e.to_string())?;
            write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
            Ok(path.to_string_lossy().into_owned())
        }
        SaveFormat::Tiff => {
            let tiff_pages: Vec<TiffPage> = pages.into_iter().map(|p| TiffPage { image: p.image, dpi: p.dpi }).collect();
            let bytes = write_tiff(tiff_pages).map_err(|e| e.to_string())?;
            let stem = render_filename(pattern, stamp, 1).map_err(|e| e.to_string())?;
            let path = unique_path(dir, &stem, "tiff", 10_000).map_err(|e| e.to_string())?;
            write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
            Ok(path.to_string_lossy().into_owned())
        }
        SaveFormat::Jpeg | SaveFormat::Png => {
            let ext = format.ext();
            let mut last = String::new();
            for (i, page) in pages.into_iter().enumerate() {
                let n = (i + 1) as u32;
                let bytes = match format {
                    SaveFormat::Jpeg => write_jpeg(&page.image, quality).map_err(|e| e.to_string())?,
                    _ => write_png(&page.image).map_err(|e| e.to_string())?,
                };
                let stem = render_filename(pattern, stamp, n).map_err(|e| e.to_string())?;
                let path = unique_path(dir, &stem, ext, 10_000).map_err(|e| e.to_string())?;
                write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
                last = path.to_string_lossy().into_owned();
            }
            Ok(last)
        }
    }
}

fn colorimage_to_imaging(img: &egui::ColorImage) -> Option<ScanImage> {
    let [w, h] = img.size;
    if w == 0 || h == 0 || img.pixels.len() != w * h {
        return None;
    }
    let mut px = Vec::with_capacity(w * h * 3);
    for p in &img.pixels {
        px.extend_from_slice(&[p.r(), p.g(), p.b()]);
    }
    ScanImage::new(w as u32, h as u32, px).ok()
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

/// A tool group; returns the clicked command id, if any.
fn tool_group(ui: &mut egui::Ui, title: &str, tools: &[(&str, bool, &str, Option<&'static str>)]) -> Option<&'static str> {
    let mut clicked = None;
    ui.collapsing(title, |ui| {
        for (label, ready, milestone, cmd) in tools {
            ui.add_enabled_ui(*ready, |ui| {
                let mut text = (*label).to_owned();
                if !ready {
                    text.push_str(&format!("  [{milestone}]"));
                }
                if ui.button(text).clicked() {
                    clicked = *cmd;
                }
            });
        }
    });
    clicked
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
        self.poll_save();
        if self.save_rx.is_some() {
            ctx.request_repaint();
        }
        self.handle_shortcuts(&ctx);

        // Command palette (Ctrl+K) owns its overlay.
        let entries: Vec<palette::Entry> = COMMANDS
            .iter()
            .map(|c| palette::Entry { id: c.id, label: c.label.to_owned(), detail: c.detail.to_owned(), ready: self.command_ready(c.id) })
            .collect();
        if let Some(id) = palette::show(&ctx, &mut self.palette_open, &mut self.palette_query, &entries) {
            self.run_command(&ctx, id);
        }
        self.show_save_dialog(&ctx);

        let (can_undo, can_redo, undo_label, page_count, dirty) = match self.session.get(self.doc()) {
            Ok(d) => (d.can_undo(), d.can_redo(), d.undo_label().unwrap_or("").to_owned(), d.project().pages.len(), d.dirty()),
            Err(_) => (false, false, String::new(), 0, false),
        };
        let _ = dirty;

        // ---- Tab strip ----
        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut switch_to: Option<usize> = None;
                let mut close_at: Option<usize> = None;
                for (i, id) in self.docs.clone().iter().enumerate() {
                    let is_active = i == self.active;
                    let dot = self.session.get(*id).map(|d| d.dirty()).unwrap_or(false);
                    let title = format!("Document {} {}", i + 1, if dot { "●" } else { "" });
                    if ui.selectable_label(is_active, title).clicked() {
                        switch_to = Some(i);
                    }
                    if ui.small_button("×").clicked() {
                        close_at = Some(i);
                    }
                }
                if ui.small_button("+").clicked() {
                    self.new_doc();
                }
                if let Some(i) = switch_to {
                    self.switch_tab(i);
                }
                if let Some(i) = close_at {
                    self.switch_tab(i);
                    self.close_doc();
                }
            });
        });

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
                    && let Ok(d) = self.session.get_mut(self.doc())
                {
                    d.undo();
                }
                if ui.add_enabled(can_redo, egui::Button::new("↪ Redo")).clicked()
                    && let Ok(d) = self.session.get_mut(self.doc())
                {
                    d.redo();
                }
                if !undo_label.is_empty() {
                    ui.label(egui::RichText::new(format!("({undo_label})")).small().weak());
                }
                ui.separator();
                let view_label = if self.view == View::Grid { "▦ Grid" } else { "▤ Single" };
                if ui.button(view_label).clicked() {
                    self.view = if self.view == View::Grid { View::Single } else { View::Grid };
                }
                if ui.button("⌘K").clicked() {
                    self.palette_open = true;
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
        let mut tool_cmd: Option<&'static str> = None;
        egui::Panel::left("tools").show(ui, |ui| {
            ui.heading("Tools");
            if let Some(cmd) = tool_group(
                ui,
                "Scan",
                &[
                    ("Scan", true, "", Some("scan")),
                    ("Scan again", true, "", Some("scan")),
                    ("Batch scan", false, "M4", None),
                    ("Profiles…", false, "M4", None),
                ],
            ) {
                tool_cmd = Some(cmd);
            }
            if let Some(cmd) = tool_group(
                ui,
                "Pages",
                &[
                    ("Delete", !self.selection.is_empty(), "", Some("delete-pages")),
                    ("Reverse order", page_count >= 2, "", Some("reverse-order")),
                    ("Interleave", page_count >= 2, "", Some("interleave")),
                    ("De-interleave", page_count >= 2, "", Some("deinterleave")),
                    ("Import files…", false, "M2", None),
                ],
            ) {
                tool_cmd = Some(cmd);
            }
            if let Some(cmd) = tool_group(
                ui,
                "Image",
                &[
                    ("Rotate left", !self.selection.is_empty(), "", Some("rotate-left")),
                    ("Rotate right", !self.selection.is_empty(), "", Some("rotate-right")),
                    ("Crop", false, "M2", None),
                    ("Deskew / auto-crop", false, "M2", None),
                ],
            ) {
                tool_cmd = Some(cmd);
            }
            if let Some(cmd) = tool_group(
                ui,
                "Export",
                &[
                    ("Save PDF…", page_count > 0, "", Some("save-pdf")),
                    ("Save TIFF…", page_count > 0, "", Some("save-tiff")),
                    ("Save JPEG…", page_count > 0, "", Some("save-jpeg")),
                    ("Save PNG…", page_count > 0, "", Some("save-png")),
                    ("Print", false, "M8", None),
                ],
            ) {
                tool_cmd = Some(cmd);
            }
            if let Some(cmd) = tool_group(ui, "OCR", &[("Searchable PDF…", false, "M5", None)]) {
                tool_cmd = Some(cmd);
            }
        });
        if let Some(cmd) = tool_cmd {
            self.run_command(&ctx, cmd);
        }

        // ---- Inspector (right) ----
        egui::Panel::right("inspector").show(ui, |ui| {
            ui.heading("Inspector");
            let selected: Vec<u64> = self.selection.iter().copied().collect();
            if selected.len() == 1 {
                let id = selected[0];
                let info = self.session.get(self.doc()).ok().and_then(|d| d.project().pages.iter().find(|p| p.id == id).cloned());
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
                        self.reset_selection(&ctx);
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
                    ui.label(egui::RichText::new("Press Ctrl+K for the command palette.").small().weak());
                });
                return;
            }
            if self.view == View::Single {
                self.show_single(ui, &ctx);
            } else {
                self.show_grid(ui, &ctx);
            }
        });
    }
}

impl ScannerApp {
    fn show_grid(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        // Collect drop edits after the loop (borrow discipline).
        let mut clicked: Option<(u64, bool)> = None;
        let mut drop_commit: Option<(u64, usize)> = None;
        egui::ScrollArea::both().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let ids: Vec<(u64, u32)> =
                    self.session.get(self.doc()).map(|d| d.project().pages.iter().map(|p| (p.id, p.dpi)).collect()).unwrap_or_default();
                let mut hover_target: Option<(u64, bool)> = None;
                for (pos, (id, dpi)) in ids.iter().enumerate() {
                    let id = *id;
                    let selected = self.selection.contains(&id);
                    let frame = if selected { egui::Frame::NONE.fill(ui.visuals().selection.bg_fill) } else { egui::Frame::NONE };
                    let mut cell_clicked: Option<(u64, bool)> = None;
                    frame.show(ui, |ui| {
                        ui.vertical(|ui| {
                            if let Some(tex) = self.textures.get(&id) {
                                let max = 160.0;
                                let size = tex.size_vec2();
                                let scale = (max / size.x).min(max / size.y).min(1.0);
                                let img = egui::Image::new(egui::load::SizedTexture::new(tex.id(), size * scale));
                                let resp = ui.add(img);
                                if resp.drag_started() {
                                    self.dragging = Some(id);
                                }
                                if resp.clicked() {
                                    let ctrl = ui.ctx().input(|i| i.modifiers.ctrl);
                                    cell_clicked = Some((id, ctrl));
                                }
                                // Insertion marker while another page is dragged over.
                                if self.dragging.is_some_and(|drag| drag != id) && resp.hovered() {
                                    let pointer_x = ui.ctx().pointer_latest_pos().map(|p| p.x).unwrap_or(0.0);
                                    let before = pointer_x < resp.rect.center().x;
                                    hover_target = Some((id, before));
                                    let x = if before { resp.rect.left() } else { resp.rect.right() };
                                    let marker =
                                        egui::Rect::from_min_max(egui::pos2(x - 2.0, resp.rect.top()), egui::pos2(x + 2.0, resp.rect.bottom()));
                                    ui.painter().rect_filled(marker, 2.0, ui.visuals().selection.stroke.color);
                                }
                            }
                            ui.label(egui::RichText::new(format!("#{id}  {dpi} dpi")).small());
                        });
                    });
                    if cell_clicked.is_some() {
                        clicked = cell_clicked;
                    }
                    let _ = pos;
                }
                // Release outside any cell cancels; release over a cell commits.
                if self.dragging.is_some() && ui.ctx().input(|i| i.pointer.any_released()) {
                    if let (Some(drag), Some((target, before))) = (self.dragging, hover_target) {
                        let ids: Vec<u64> =
                            self.session.get(self.doc()).map(|d| d.project().pages.iter().map(|p| p.id).collect()).unwrap_or_default();
                        if let (Some(from_pos), Some(at)) = (ids.iter().position(|p| *p == drag), ids.iter().position(|p| *p == target)) {
                            // `to` is the target index, shifted when inserting after;
                            // removal of the dragged page shifts later indices left.
                            let mut to = if before { at } else { at + 1 };
                            if from_pos < to {
                                to -= 1;
                            }
                            if from_pos != to {
                                drop_commit = Some((drag, to));
                            }
                        }
                    }
                    self.dragging = None;
                }
            });
        });
        if let Some((id, to)) = drop_commit {
            self.reorder(id, to);
        }
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
    }

    fn show_single(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let ids: Vec<u64> = self.session.get(self.doc()).map(|d| d.project().pages.iter().map(|p| p.id).collect()).unwrap_or_default();
        if ids.is_empty() {
            return;
        }
        let current = self.selection.iter().copied().find(|id| ids.contains(id)).unwrap_or(ids[0]);
        ui.horizontal(|ui| {
            let pos = ids.iter().position(|id| *id == current).unwrap_or(0);
            if ui.add_enabled(pos > 0, egui::Button::new("◀ Prev")).clicked() && pos > 0 {
                self.selection.clear();
                self.selection.insert(ids[pos - 1]);
            }
            ui.label(format!("Page {} of {}", pos + 1, ids.len()));
            if ui.add_enabled(pos + 1 < ids.len(), egui::Button::new("Next ▶")).clicked() {
                self.selection.clear();
                self.selection.insert(ids[pos + 1]);
            }
            ui.separator();
            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut self.zoom, 0.25..=3.0).logarithmic(true));
            if ui.button("Grid view").clicked() {
                self.view = View::Grid;
            }
        });
        egui::ScrollArea::both().show(ui, |ui| {
            if let Some(tex) = self.textures.get(&current) {
                let size = tex.size_vec2() * self.zoom;
                ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), size)));
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
