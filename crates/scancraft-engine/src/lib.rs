//! Engine facade: every frontend (UI, CLI, MCP) talks only to this crate.
//! Holds open documents, applies edits as named undo steps, and runs scans
//! against any [`ScannerBackend`] via the `scan-core` contract.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeMap;
use std::fmt;

use scancraft_model::{EditOp, ModelError, Page, Project};
use scancraft_scan_core::{CancelToken, PageSink, Progress, RawPage, ScanError, ScanRequest, ScannerBackend};

/// Open-document handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub u64);

/// One named undo step (snapshot-based in M0; journaled ops land in M1).
#[derive(Debug, Clone)]
struct UndoStep {
    label: String,
    project: Project,
}

/// An open document with deep undo/redo history.
#[derive(Debug, Clone)]
pub struct Document {
    project: Project,
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
}

impl Document {
    pub fn new(project: Project) -> Self {
        Self { project, undo: Vec::new(), redo: Vec::new() }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    fn commit(&mut self, label: &str, next: Project) {
        const CAP: usize = 100;
        self.undo.push(UndoStep { label: label.to_owned(), project: std::mem::replace(&mut self.project, next) });
        if self.undo.len() > CAP {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Undo one step. Returns the step label, if any.
    pub fn undo(&mut self) -> Option<String> {
        let step = self.undo.pop()?;
        self.redo.push(UndoStep { label: step.label.clone(), project: self.project.clone() });
        self.project = step.project;
        Some(step.label)
    }

    /// Redo one step. Returns the step label, if any.
    pub fn redo(&mut self) -> Option<String> {
        let step = self.redo.pop()?;
        self.undo.push(UndoStep { label: step.label.clone(), project: self.project.clone() });
        self.project = step.project;
        Some(step.label)
    }

    fn page_index(&self, id: u64) -> Result<usize, EngineError> {
        self.project.pages.iter().position(|p| p.id == id).ok_or(EngineError::Model(ModelError::PageNotFound(id)))
    }

    /// Append scanned/imported pages as one undo step.
    pub fn append_pages(&mut self, pages: Vec<Page>, label: &str) -> Result<(), EngineError> {
        for p in &pages {
            p.validate().map_err(EngineError::Model)?;
        }
        let mut next = self.project.clone();
        next.pages.extend(pages);
        self.commit(label, next);
        Ok(())
    }

    /// Delete pages by id as one undo step.
    pub fn delete_pages(&mut self, ids: &[u64]) -> Result<String, EngineError> {
        let mut next = self.project.clone();
        let before = next.pages.len();
        next.pages.retain(|p| !ids.contains(&p.id));
        if next.pages.len() == before {
            return Err(EngineError::Model(ModelError::PageNotFound(ids.first().copied().unwrap_or(0))));
        }
        let label = format!("Delete {} page(s)", before - next.pages.len());
        self.commit(&label.clone(), next);
        Ok(label)
    }

    /// Move a page to a new position as one undo step.
    pub fn move_page(&mut self, id: u64, to: usize) -> Result<(), EngineError> {
        let from = self.page_index(id)?;
        let mut next = self.project.clone();
        let page = next.pages.remove(from);
        let to = to.min(next.pages.len());
        next.pages.insert(to, page);
        self.commit("Move page", next);
        Ok(())
    }

    /// Reverse page order as one undo step.
    pub fn reverse_order(&mut self) {
        let mut next = self.project.clone();
        next.pages.reverse();
        self.commit("Reverse order", next);
    }

    /// Record one non-destructive edit on a page as a single undo step.
    pub fn record_edit(&mut self, id: u64, edit: EditOp, label: &str) -> Result<(), EngineError> {
        let at = self.page_index(id)?;
        let mut next = self.project.clone();
        if let Some(page) = next.pages.get_mut(at) {
            page.edits.push(edit);
        }
        self.commit(label, next);
        Ok(())
    }

    /// Clear a page's edit stack as a single undo step ("Reset page").
    pub fn clear_edits(&mut self, id: u64) -> Result<(), EngineError> {
        let at = self.page_index(id)?;
        let mut next = self.project.clone();
        if let Some(page) = next.pages.get_mut(at) {
            page.edits.clear();
        }
        self.commit("Reset page", next);
        Ok(())
    }
}

/// Session owning all open documents.
#[derive(Debug, Default)]
pub struct Session {
    docs: BTreeMap<DocId, Document>,
    next: u64,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, project: Project) -> DocId {
        self.next += 1;
        let id = DocId(self.next);
        self.docs.insert(id, Document::new(project));
        id
    }

    pub fn get(&self, id: DocId) -> Result<&Document, EngineError> {
        self.docs.get(&id).ok_or(EngineError::UnknownDocument)
    }

    pub fn get_mut(&mut self, id: DocId) -> Result<&mut Document, EngineError> {
        self.docs.get_mut(&id).ok_or(EngineError::UnknownDocument)
    }

    pub fn close(&mut self, id: DocId) -> Result<(), EngineError> {
        self.docs.remove(&id).map(|_| ()).ok_or(EngineError::UnknownDocument)
    }

    pub fn document_ids(&self) -> Vec<DocId> {
        self.docs.keys().copied().collect()
    }

    /// Run a scan into a document: pages stream in via `events`, then append
    /// as a single undo step. M0 keeps pixels in the sink; the session cache
    /// (disk) lands in M2.
    pub fn scan_into(
        &mut self,
        id: DocId,
        backend: &dyn ScannerBackend,
        request: &ScanRequest,
        events: &mut dyn ScanEvents,
        cancel: &CancelToken,
    ) -> Result<u32, EngineError> {
        let mut sink = CollectingSink::new(events);
        let count = backend.scan(request, &mut sink, cancel).map_err(EngineError::Scan)?;
        let base = self.get(id).map(|d| d.project.pages.len()).unwrap_or(0);
        let pages: Vec<Page> = sink
            .pages
            .into_iter()
            .enumerate()
            .map(|(i, raw)| Page {
                id: (base as u64) + (i as u64) + 1,
                width_px: raw.width_px,
                height_px: raw.height_px,
                dpi: raw.dpi,
                bit_depth: raw.bit_depth,
                side: raw.side,
                storage_key: format!("scan-{:06}", base + i + 1),
                edits: Vec::new(),
            })
            .collect();
        self.get_mut(id)?.append_pages(pages, &format!("Scan {count} page(s)"))?;
        Ok(count)
    }
}

/// UI-facing event sink (progress forwarded; pages buffered for one commit).
pub trait ScanEvents {
    fn progress(&mut self, progress: Progress);
}

struct CollectingSink<'a> {
    events: &'a mut dyn ScanEvents,
    pages: Vec<RawPage>,
}

impl<'a> CollectingSink<'a> {
    fn new(events: &'a mut dyn ScanEvents) -> Self {
        Self { events, pages: Vec::new() }
    }
}

impl PageSink for CollectingSink<'_> {
    fn page(&mut self, page: RawPage) -> Result<(), ScanError> {
        self.pages.push(page);
        Ok(())
    }

    fn progress(&mut self, progress: Progress) {
        self.events.progress(progress);
    }
}

/// Closed engine error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    UnknownDocument,
    Model(ModelError),
    Scan(ScanError),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::UnknownDocument => write!(f, "unknown document"),
            EngineError::Model(e) => write!(f, "{e}"),
            EngineError::Scan(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EngineError {}

#[cfg(test)]
mod tests {
    use super::*;
    use scancraft_model::{BitDepth, PageSide, ScanProfile};
    use scancraft_scan_core::ScanRequest;
    use scancraft_scan_mock::MockScanner;

    struct NullEvents;

    impl ScanEvents for NullEvents {
        fn progress(&mut self, _progress: Progress) {}
    }

    fn page(id: u64) -> Page {
        Page {
            id,
            width_px: 100,
            height_px: 100,
            dpi: 100,
            bit_depth: BitDepth::Color,
            side: PageSide::Front,
            storage_key: format!("p{id}"),
            edits: Vec::new(),
        }
    }

    #[test]
    fn undo_redo_named_steps() {
        let mut session = Session::new();
        let id = session.open(Project { title: "t".to_owned(), pages: vec![page(1)] });
        session.get_mut(id).unwrap().append_pages(vec![page(2)], "Append 1 page(s)").unwrap();
        assert_eq!(session.get(id).unwrap().project.pages.len(), 2);
        assert_eq!(session.get(id).unwrap().undo_label(), Some("Append 1 page(s)"));
        assert_eq!(session.get_mut(id).unwrap().undo(), Some("Append 1 page(s)".to_owned()));
        assert_eq!(session.get(id).unwrap().project.pages.len(), 1);
        assert_eq!(session.get_mut(id).unwrap().redo(), Some("Append 1 page(s)".to_owned()));
        assert_eq!(session.get(id).unwrap().project.pages.len(), 2);
    }

    #[test]
    fn delete_move_reverse_are_single_steps() {
        let mut session = Session::new();
        let id = session.open(Project { title: "t".to_owned(), pages: vec![page(1), page(2), page(3)] });
        session.get_mut(id).unwrap().delete_pages(&[2]).unwrap();
        assert_eq!(session.get(id).unwrap().project.pages.len(), 2);
        session.get_mut(id).unwrap().undo();
        assert_eq!(session.get(id).unwrap().project.pages.len(), 3);

        session.get_mut(id).unwrap().move_page(3, 0).unwrap();
        assert_eq!(session.get(id).unwrap().project.pages[0].id, 3);
        session.get_mut(id).unwrap().reverse_order();
        assert_eq!(session.get(id).unwrap().project.pages[0].id, 2);
    }

    #[test]
    fn scan_into_appends_one_undo_step() {
        let mut session = Session::new();
        let id = session.open(Project::default());
        let profile = ScanProfile { device_id: "mock:demo-scanner".to_owned(), dpi: 100, ..ScanProfile::default() };
        let mut ignored = Vec::new();
        let req = ScanRequest::from_profile(&profile, &mut ignored).unwrap();
        let backend = MockScanner { pages: 2, ..Default::default() };
        let mut events = NullEvents;
        let n = session.scan_into(id, &backend, &req, &mut events, &CancelToken::new()).unwrap();
        assert_eq!(n, 2);
        assert_eq!(session.get(id).unwrap().project.pages.len(), 2);
        session.get_mut(id).unwrap().undo();
        assert!(session.get(id).unwrap().project.pages.is_empty());
    }

    #[test]
    fn record_and_reset_edits() {
        use scancraft_model::EditOp;
        let mut session = Session::new();
        let id = session.open(Project { title: "t".to_owned(), pages: vec![page(1)] });
        session.get_mut(id).unwrap().record_edit(1, EditOp::RotateCw, "Rotate right").unwrap();
        assert_eq!(session.get(id).unwrap().project.pages[0].edits.len(), 1);
        session.get_mut(id).unwrap().undo();
        assert!(session.get(id).unwrap().project.pages[0].edits.is_empty());
        session.get_mut(id).unwrap().redo();
        session.get_mut(id).unwrap().clear_edits(1).unwrap();
        assert!(session.get(id).unwrap().project.pages[0].edits.is_empty());
    }
}
