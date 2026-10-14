// pdfcraft-scanner entry point.
// Standalone printer/scanner shell over the PdfCraft engine libs.
// Behaviour (scan source, OCR pipeline, print spool wiring) lands when you
// provide the working instructions. This file only proves the imports resolve
// and opens a minimal window / CLI without panicking on bad input.

mod app;
mod print_job;
mod scan;

use std::env;

fn usage() -> String {
    "pdfcraft-scanner [--help] [--version] [FILE]\n\nStandalone printer/scanner (scaffold).\nBehaviour arrives later; today it only shows the shell.".to_owned()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("pdfcraft-scanner {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Touch the PdfCraft libs so a missing path dep fails at build time,
    // not later at feature time. No document is opened here.
    let _ = scan::describe_pipeline();
    let _ = print_job::describe_pipeline();

    // Minimal native shell. Full egui layout (theme/palette/icons from
    // pdfcraft-ui-egui, see design/DESIGN.md) gets wired per your instructions.
    app::run()
}
