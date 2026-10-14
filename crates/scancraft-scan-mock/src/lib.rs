//! Deterministic mock scanner. Produces generated pages (gradient plus
//! index bars, optional blank pages) with configurable count, duplex,
//! delays and injected failures. All engine/UI tests run against it.
//! Ships only as a dev-only "Demo scanner".

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use scancraft_model::{BitDepth, PageSide, PageSize, PaperSource};
use scancraft_scan_core::{CancelToken, Capabilities, DeviceId, DeviceInfo, PageSink, Progress, RawPage, ScanError, ScanRequest, ScannerBackend};

/// Failure to inject into a mock run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockFailure {
    /// Report paper-empty after `n` pages (0 = before the first page).
    PaperEmptyAfter(u32),
    JamOnPage(u32),
    Busy,
}

/// Configurable fake scanner.
#[derive(Debug, Clone)]
pub struct MockScanner {
    pub pages: u32,
    pub duplex: bool,
    pub blank_every: u32,
    pub failure: Option<MockFailure>,
}

impl Default for MockScanner {
    fn default() -> Self {
        Self { pages: 3, duplex: false, blank_every: 0, failure: None }
    }
}

impl MockScanner {
    pub fn demo_device() -> DeviceInfo {
        DeviceInfo { id: DeviceId("mock:demo-scanner".to_owned()), backend: "mock", display_name: "Demo scanner (mock)".to_owned() }
    }

    /// Deterministic RGB8 page: vertical gradient plus index bars.
    /// No randomness — identical bytes for the same (index, w, h).
    fn render_page(index: u32, width_px: u32, height_px: u32, blank: bool) -> Vec<u8> {
        let w = width_px.min(512);
        let h = height_px.min(512);
        let mut px = Vec::with_capacity((w as usize) * (h as usize) * 3);
        for y in 0..h {
            for x in 0..w {
                let (r, g, b) = if blank {
                    (245u8, 245u8, 245u8)
                } else {
                    let shade = (x.wrapping_mul(255).wrapping_div(w.max(1))) as u8;
                    let bar = if (y / 16 + index).is_multiple_of(2) { 255u8 } else { 200u8 };
                    (shade, bar, 255u8.wrapping_sub(shade))
                };
                px.push(r);
                px.push(g);
                px.push(b);
            }
        }
        // Upscale note: tests use small sizes; real sizes stay bounded by
        // RawPage::validate. Full-size rendering lands with imaging (M2).
        let _ = (width_px, height_px);
        px
    }

    fn page_size_px(size: PageSize, dpi: u32) -> (u32, u32) {
        let dpi_f = f64::from(dpi);
        let dims = size.dims_mm().unwrap_or((210.0, 297.0));
        let w = (dims.0 / 25.4 * dpi_f).round().clamp(1.0, 512.0) as u32;
        let h = (dims.1 / 25.4 * dpi_f).round().clamp(1.0, 512.0) as u32;
        (w.max(1), h.max(1))
    }
}

impl ScannerBackend for MockScanner {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn list_devices(&self, cancel: &CancelToken) -> Result<Vec<DeviceInfo>, ScanError> {
        cancel.check()?;
        if self.failure == Some(MockFailure::Busy) {
            return Err(ScanError::Busy);
        }
        Ok(vec![Self::demo_device()])
    }

    fn capabilities(&self, _device: &DeviceId) -> Result<Capabilities, ScanError> {
        Ok(Capabilities {
            sources: vec![PaperSource::Flatbed, PaperSource::Feeder, PaperSource::Duplex],
            dpis: vec![100, 150, 200, 300, 600],
            bit_depths: vec![BitDepth::Color, BitDepth::Grayscale, BitDepth::BlackWhite],
            page_sizes: vec![PageSize::A4, PageSize::Letter, PageSize::Legal, PageSize::A5],
            duplex: true,
            max_pages_per_run: None,
        })
    }

    fn scan(&self, request: &ScanRequest, sink: &mut dyn PageSink, cancel: &CancelToken) -> Result<u32, ScanError> {
        if self.failure == Some(MockFailure::Busy) {
            return Err(ScanError::Busy);
        }
        let (w, h) = Self::page_size_px(request.page_size, request.dpi);
        let mut delivered: u32 = 0;
        for index in 0..self.pages {
            cancel.check()?;
            if let Some(MockFailure::PaperEmptyAfter(n)) = self.failure
                && index >= n
            {
                // Empty at start = error; dry at end = success (feeder rule).
                if delivered == 0 {
                    return Err(ScanError::PaperEmpty);
                }
                break;
            }
            if let Some(MockFailure::JamOnPage(n)) = self.failure
                && index == n
            {
                return Err(ScanError::PaperJam);
            }
            let blank = self.blank_every != 0 && (index + 1) % self.blank_every == 0;
            let sides: &[PageSide] = if self.duplex { &[PageSide::Front, PageSide::Back] } else { &[PageSide::Front] };
            for side in sides {
                cancel.check()?;
                let page = RawPage {
                    width_px: w,
                    height_px: h,
                    dpi: request.dpi,
                    bit_depth: request.bit_depth,
                    side: *side,
                    pixels_rgb8: Self::render_page(index, w, h, blank),
                };
                page.validate()?;
                sink.page(page)?;
                delivered += 1;
                sink.progress(Progress { pages_done: delivered, stage: "scanning" });
            }
        }
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scancraft_model::ScanProfile;

    struct Collect {
        pages: Vec<RawPage>,
        progress: Vec<Progress>,
    }

    impl PageSink for Collect {
        fn page(&mut self, page: RawPage) -> Result<(), ScanError> {
            self.pages.push(page);
            Ok(())
        }

        fn progress(&mut self, progress: Progress) {
            self.progress.push(progress);
        }
    }

    fn request(pages_profile: &ScanProfile) -> (ScanRequest, Vec<String>) {
        let mut ignored = Vec::new();
        let req = ScanRequest::from_profile(pages_profile, &mut ignored).unwrap();
        (req, ignored)
    }

    fn profile() -> ScanProfile {
        ScanProfile { device_id: "mock:demo-scanner".to_owned(), dpi: 100, ..ScanProfile::default() }
    }

    #[test]
    fn delivers_configured_pages() {
        let backend = MockScanner { pages: 3, ..Default::default() };
        let (req, _) = request(&profile());
        let mut sink = Collect { pages: Vec::new(), progress: Vec::new() };
        let n = backend.scan(&req, &mut sink, &CancelToken::new()).unwrap();
        assert_eq!((n, sink.pages.len()), (3, 3));
        assert!(!sink.progress.is_empty());
    }

    #[test]
    fn duplex_doubles_sides() {
        let backend = MockScanner { pages: 2, duplex: true, ..Default::default() };
        let (req, _) = request(&profile());
        let mut sink = Collect { pages: Vec::new(), progress: Vec::new() };
        let n = backend.scan(&req, &mut sink, &CancelToken::new()).unwrap();
        assert_eq!(n, 4);
        assert_eq!(sink.pages[1].side, PageSide::Back);
    }

    #[test]
    fn empty_at_start_is_error_dry_at_end_is_success() {
        let backend = MockScanner { pages: 4, failure: Some(MockFailure::PaperEmptyAfter(0)), ..Default::default() };
        let (req, _) = request(&profile());
        let mut sink = Collect { pages: Vec::new(), progress: Vec::new() };
        assert_eq!(backend.scan(&req, &mut sink, &CancelToken::new()), Err(ScanError::PaperEmpty));

        let backend = MockScanner { pages: 4, failure: Some(MockFailure::PaperEmptyAfter(2)), ..Default::default() };
        let mut sink = Collect { pages: Vec::new(), progress: Vec::new() };
        let n = backend.scan(&req, &mut sink, &CancelToken::new()).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn cancel_between_pages() {
        struct Cancelling {
            inner: Collect,
            token: CancelToken,
            after: usize,
        }

        impl PageSink for Cancelling {
            fn page(&mut self, page: RawPage) -> Result<(), ScanError> {
                self.inner.page(page)?;
                if self.inner.pages.len() >= self.after {
                    self.token.cancel();
                }
                Ok(())
            }

            fn progress(&mut self, p: Progress) {
                self.inner.progress(p);
            }
        }

        let backend = MockScanner { pages: 10, ..Default::default() };
        let (req, _) = request(&profile());
        let mut sink = Cancelling { inner: Collect { pages: Vec::new(), progress: Vec::new() }, token: CancelToken::new(), after: 2 };
        // The backend checks the *passed-in* token; emulate by sharing it.
        let shared = sink.token.clone();
        let result = backend.scan(&req, &mut sink, &shared);
        assert_eq!(result, Err(ScanError::Cancelled));
        assert_eq!(sink.inner.pages.len(), 2);
    }

    #[test]
    fn deterministic_bytes() {
        let a = MockScanner::render_page(3, 64, 64, false);
        let b = MockScanner::render_page(3, 64, 64, false);
        assert_eq!(a, b);
    }
}
