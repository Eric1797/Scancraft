// Scan side: acquire pages -> build PDF -> optional OCR layer.
// Real acquisition (WIA/TWAIN/SANE, flatbed/ADF, DPI, duplex) is wired later
// per your working instructions. This module only names the intended
// PdfCraft libs so the dependency set stays correct.

/// Human-readable description of the planned scan pipeline.
pub fn describe_pipeline() -> &'static str {
    // pdfcraft-create: images (PNG/JPEG/TIFF) -> PDF (scan output).
    // pdfcraft-ocr:    invisible searchable text layer over page images.
    // pdfcraft-engine: session/document/command catalogue (open/save flow).
    // pdfcraft-model / pdfcraft-cos / pdfcraft-content: PDF object graph.
    "scan: acquire image -> pdfcraft-create (PDF) -> pdfcraft-ocr (searchable layer)"
}

/// Planned scan settings. Filled in from your instructions later.
#[derive(Debug, Clone)]
pub struct ScanSettings {
    /// Dots per inch requested from the hardware.
    pub dpi: u32,
    /// True for colour, false for greyscale/mono (exact meaning TBD).
    pub colour: bool,
    /// True when an ADF / multi-page run should produce one PDF.
    pub multi_page: bool,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            dpi: 300,
            colour: false,
            multi_page: true,
        }
    }
}

/// Validate settings without panicking; hardware wiring comes later.
pub fn validate(settings: &ScanSettings) -> Result<(), String> {
    if settings.dpi == 0 || settings.dpi > 1200 {
        return Err(format!("unsupported dpi: {}", settings.dpi));
    }
    Ok(())
}
