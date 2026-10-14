// Minimal app shell. Returns Result, never panics on bad input.
// Full layout (Scan / Preview / Print panels, theme, icons) is built after
// your working instructions. Design tokens live in design/DESIGN.md.

use crate::{print_job, scan};

/// Run the standalone shell. Headless-safe: no args required.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Validate defaults up front so misconfiguration surfaces as an error,
    // not a crash (PdfCraft "never crash" rule).
    if let Err(msg) = scan::validate(&scan::ScanSettings::default()) {
        return Err(msg.into());
    }
    if let Err(msg) = print_job::validate(&print_job::PrintSettings::default()) {
        return Err(msg.into());
    }

    // Placeholder shell. Replace with eframe/egui wired to pdfcraft-ui-egui
    // theme + palette (see design/DESIGN.md) when behaviour is specified.
    println!("pdfcraft-scanner scaffold ready.");
    println!("  {}", scan::describe_pipeline());
    println!("  {}", print_job::describe_pipeline());
    println!("Awaiting working instructions.");
    Ok(())
}
