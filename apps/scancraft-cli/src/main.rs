//! `scancraft-cli` — headless front door to the engine.
//! M2: `devices`, `profiles`, and `scan --out` (mock backend → file).
//! Full JSON tool table + `--script` + MCP arrive with automation (M8).
//! No unwraps on user input: every failure is a message + exit code 2.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::PathBuf;

use scancraft_export::{PdfPage, TiffPage, write_atomic, write_jpeg, write_pdf, write_png, write_tiff};
use scancraft_imaging::Image;
use scancraft_model::ScanProfile;
use scancraft_scan_core::{CancelToken, PageSink, Progress, RawPage, ScanError, ScanRequest, ScannerBackend};
use scancraft_scan_mock::MockScanner;

fn usage() -> &'static str {
    "scancraft-cli [--help] [--version] <command>\n\nCommands:\n  devices    list scanners (mock backend)\n  profiles   list built-in profiles\n  scan --pages N --dpi D --out FILE [--format pdf|tiff|jpg|png] [--quality Q] [--force]\n             scan with the mock backend straight to a file\n"
}

fn list_devices() -> Result<(), String> {
    let backend = MockScanner::default();
    let devices = backend.list_devices(&CancelToken::new()).map_err(|e| e.message())?;
    if devices.is_empty() {
        println!("no scanners found");
        return Ok(());
    }
    for d in devices {
        println!("{} [{}] {}", d.id.0, d.backend, d.display_name);
    }
    Ok(())
}

fn list_profiles() -> Result<(), String> {
    let profile = ScanProfile::default();
    profile.validate().map_err(|e| e.to_string())?;
    println!("{} driver={} dpi={} source={:?}", profile.name, profile.driver, profile.dpi, profile.paper_source);
    Ok(())
}

struct Collect {
    pages: Vec<RawPage>,
}

impl PageSink for Collect {
    fn page(&mut self, page: RawPage) -> Result<(), ScanError> {
        self.pages.push(page);
        Ok(())
    }

    fn progress(&mut self, _progress: Progress) {}
}

/// Parse `--key value` / `--flag` args into (key, Option<value>) pairs.
fn options(argv: &[String]) -> Result<Vec<(String, Option<String>)>, String> {
    let mut out = Vec::new();
    let mut i = 2;
    while i < argv.len() {
        let arg = &argv[i];
        if !arg.starts_with("--") {
            return Err(format!("expected an option, got: {arg}"));
        }
        let key = arg.trim_start_matches('-').to_owned();
        let value = if i + 1 < argv.len() && !argv[i + 1].starts_with("--") {
            i += 1;
            Some(argv[i].clone())
        } else {
            None
        };
        out.push((key, value));
        i += 1;
    }
    Ok(out)
}

fn opt_value(opts: &[(String, Option<String>)], key: &str) -> Result<Option<String>, String> {
    let mut found = None;
    for (k, v) in opts {
        if k == key {
            found = Some(v.clone().ok_or_else(|| format!("--{key} needs a value"))?);
        }
    }
    Ok(found)
}

fn has_flag(opts: &[(String, Option<String>)], key: &str) -> bool {
    opts.iter().any(|(k, _)| k == key)
}

fn parse_u32(value: &Option<String>, key: &str, fallback: u32) -> Result<u32, String> {
    match value {
        None => Ok(fallback),
        Some(s) => s.parse::<u32>().map_err(|_| format!("--{key} must be a number, got: {s}")),
    }
}

fn scan_to_file(argv: &[String]) -> Result<(), String> {
    let opts = options(argv)?;
    let pages = parse_u32(&opt_value(&opts, "pages")?, "pages", 2)?;
    let dpi = parse_u32(&opt_value(&opts, "dpi")?, "dpi", 300)?;
    let quality = parse_u32(&opt_value(&opts, "quality")?, "quality", 85)?;
    let out = opt_value(&opts, "out")?.ok_or("--out FILE is required")?;
    let force = has_flag(&opts, "force");
    if pages == 0 || pages > 10_000 {
        return Err("--pages must be 1..=10000".to_owned());
    }

    let profile = ScanProfile { device_id: "mock:demo-scanner".to_owned(), dpi, ..ScanProfile::default() };
    let mut ignored = Vec::new();
    let request = ScanRequest::from_profile(&profile, &mut ignored).map_err(|e| e.message())?;

    let backend = MockScanner { pages, ..Default::default() };
    let mut sink = Collect { pages: Vec::new() };
    backend.scan(&request, &mut sink, &CancelToken::new()).map_err(|e| e.message())?;
    if sink.pages.is_empty() {
        return Err("scan produced no pages".to_owned());
    }

    let format = match opt_value(&opts, "format")?.as_deref() {
        Some("pdf") | None if out.ends_with(".pdf") => "pdf",
        Some("tiff") | None if out.ends_with(".tiff") || out.ends_with(".tif") => "tiff",
        Some("jpg") | Some("jpeg") | None if out.ends_with(".jpg") || out.ends_with(".jpeg") => "jpg",
        Some("png") | None if out.ends_with(".png") => "png",
        Some(other) => return Err(format!("unknown --format: {other} (pdf|tiff|jpg|png)")),
        None => return Err("cannot guess the format: use --format pdf|tiff|jpg|png".to_owned()),
    };

    let images: Vec<(Image, u32)> = sink
        .pages
        .into_iter()
        .map(|raw| Image::new(raw.width_px, raw.height_px, raw.pixels_rgb8).map(|img| (img, raw.dpi)).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;

    let bytes = match format {
        "pdf" => {
            let pdf_pages: Vec<PdfPage> = images.into_iter().map(|(image, dpi)| PdfPage { image, dpi }).collect();
            write_pdf(pdf_pages, &scancraft_export::PdfMeta::default()).map_err(|e| e.to_string())?
        }
        "tiff" => {
            let tiff_pages: Vec<TiffPage> = images.into_iter().map(|(image, dpi)| TiffPage { image, dpi }).collect();
            write_tiff(tiff_pages).map_err(|e| e.to_string())?
        }
        "jpg" => {
            let (image, _) = images.into_iter().next().ok_or("no pages")?;
            write_jpeg(&image, quality.min(100) as u8).map_err(|e| e.to_string())?
        }
        _ => {
            let (image, _) = images.into_iter().next().ok_or("no pages")?;
            write_png(&image).map_err(|e| e.to_string())?
        }
    };

    let path = PathBuf::from(&out);
    if path.exists() && !force {
        return Err(format!("refusing to overwrite without --force: {out}"));
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create folder: {e}"))?;
    }
    write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

fn run(argv: &[String]) -> Result<(), String> {
    if argv.len() < 2 {
        return Err(usage().to_owned());
    }
    match argv[1].as_str() {
        "--help" | "-h" | "help" => {
            print!("{}", usage());
            Ok(())
        }
        "--version" | "-V" => {
            println!("scancraft-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "devices" => list_devices(),
        "profiles" => list_profiles(),
        "scan" => scan_to_file(argv),
        other => Err(format!("unknown command: {other}\n{}", usage())),
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if let Err(msg) = run(&argv) {
        eprintln!("{msg}");
        std::process::exit(2);
    }
}
