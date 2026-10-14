// Print side: PDF -> print-ready sheets -> OS spooler.
// pdfcraft-print already handles page selection, sizing (fit/actual/shrink),
// N-up, booklet, poster, and CUPS / Windows spool submission. This module
// only names that path until your working instructions define the UI.

/// Human-readable description of the planned print pipeline.
pub fn describe_pipeline() -> &'static str {
    // pdfcraft-print: layout() for geometry previews, impose() for the
    // print-ready PDF, spool::submit() for CUPS / Windows spoolers.
    // pdfcraft-render: page previews shown before printing.
    "print: PDF -> pdfcraft-print impose() -> spool::submit() (CUPS / Windows)"
}

/// Planned print settings. Fields mirror pdfcraft-print concepts.
#[derive(Debug, Clone)]
pub struct PrintSettings {
    /// Printer name, or None for the system default.
    pub printer: Option<String>,
    /// Copies requested.
    pub copies: u32,
    /// True for greyscale output.
    pub monochrome: bool,
}

impl Default for PrintSettings {
    fn default() -> Self {
        Self {
            printer: None,
            copies: 1,
            monochrome: false,
        }
    }
}

/// Validate settings without panicking; spool wiring comes later.
pub fn validate(settings: &PrintSettings) -> Result<(), String> {
    if settings.copies == 0 || settings.copies > 999 {
        return Err(format!("unsupported copies: {}", settings.copies));
    }
    Ok(())
}
