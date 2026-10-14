//! Export: one image per page to PDF (minimal writer), multipage TIFF,
//! JPEG/PNG singles, filename templates, atomic writes.
//!
//! M2 scope notes (honest limits, all recorded in `docs/feature-matrix.md`):
//! - PDF images are Flate RGB; JPEG pass-through and CCITT-G4 1-bit land later.
//! - TIFF is uncompressed RGB (LZW/G4 later); carries DPI tags.
//! - JPEG/PNG singles do not embed DPI metadata yet (M2b).
//! - No encryption yet (M2b reuses the PdfCraft crypt approach); no OCR
//!   layer yet (M5); no PDF/A yet.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use scancraft_imaging::{Image, ImgError};

/// Cap so a runaway selection cannot build a gigantic file in RAM.
pub const MAX_PAGES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    EmptyDocument,
    TooManyPages,
    BadDpi(u32),
    BadTemplate(String),
    Image(ImgError),
    Io(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::EmptyDocument => write!(f, "nothing to export: no pages"),
            ExportError::TooManyPages => write!(f, "too many pages for one export"),
            ExportError::BadDpi(dpi) => write!(f, "unsupported DPI: {dpi}"),
            ExportError::BadTemplate(msg) => write!(f, "bad filename pattern: {msg}"),
            ExportError::Image(e) => write!(f, "{e}"),
            ExportError::Io(msg) => write!(f, "file error: {msg}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<ImgError> for ExportError {
    fn from(e: ImgError) -> Self {
        ExportError::Image(e)
    }
}

fn check_pages<T>(pages: &[T]) -> Result<(), ExportError> {
    if pages.is_empty() {
        return Err(ExportError::EmptyDocument);
    }
    if pages.len() > MAX_PAGES {
        return Err(ExportError::TooManyPages);
    }
    Ok(())
}

fn check_dpi(dpi: u32) -> Result<(), ExportError> {
    if !(50..=2400).contains(&dpi) { Err(ExportError::BadDpi(dpi)) } else { Ok(()) }
}

// ---------------------------------------------------------------------------
// PDF
// ---------------------------------------------------------------------------

/// One PDF page: full image + resolution.
pub struct PdfPage {
    pub image: Image,
    pub dpi: u32,
}

/// Document metadata (`/Info`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PdfMeta {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
}

fn pdf_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => out.push_str(&format!("\\{:03o}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).unwrap_or(());
    enc.finish().unwrap_or_default()
}

/// Minimal correct PDF: Catalog → Pages → Page → RGB image XObject +
/// content stream, FlateDecode, MediaBox from pixels ÷ DPI × 72.
pub fn write_pdf(pages: Vec<PdfPage>, meta: &PdfMeta) -> Result<Vec<u8>, ExportError> {
    check_pages(&pages)?;
    for p in &pages {
        check_dpi(p.dpi)?;
    }

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    // Object numbers: 1 catalog, 2 pages, then per page: page, content, image.
    let mut offsets: Vec<usize> = vec![0];
    let emit = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, num: u32, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };

    let n = pages.len();
    let page_nums: Vec<u32> = (0..n as u32).map(|i| 3 + i * 3).collect();
    let kids: String = page_nums.iter().map(|p| format!("{p} 0 R")).collect::<Vec<_>>().join(" ");
    emit(&mut out, &mut offsets, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    emit(&mut out, &mut offsets, 2, format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").as_bytes());

    for (i, page) in pages.iter().enumerate() {
        let (w_px, h_px) = page.image.dims();
        let (w_pt, h_pt) = (f64::from(w_px) * 72.0 / f64::from(page.dpi), f64::from(h_px) * 72.0 / f64::from(page.dpi));
        let (pno, cno, ino) = (page_nums[i], page_nums[i] + 1, page_nums[i] + 2);
        emit(
            &mut out,
            &mut offsets,
            pno,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w_pt:.2} {h_pt:.2}] /Resources << /XObject << /Im0 {ino} 0 R >> >> /Contents {cno} 0 R >>"
            )
            .as_bytes(),
        );
        let content = format!("q {w_pt:.2} 0 0 {h_pt:.2} 0 0 cm /Im0 Do Q");
        emit(&mut out, &mut offsets, cno, format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).as_bytes());
        let raw = page.image.pixels();
        let data = deflate(raw);
        let (iw, ih) = (page.image.dims().0, page.image.dims().1);
        let mut img = format!(
            "<< /Type /XObject /Subtype /Image /Width {iw} /Height {ih} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} /Filter /FlateDecode >>\nstream\n",
            data.len()
        )
        .into_bytes();
        img.extend_from_slice(&data);
        img.extend_from_slice(b"\nendstream");
        emit(&mut out, &mut offsets, ino, &img);
    }

    let info_num = 3 + n as u32 * 3;
    let mut info_ref = String::new();
    let mut info_body = String::new();
    for (key, val) in [("Title", &meta.title), ("Author", &meta.author), ("Subject", &meta.subject), ("Keywords", &meta.keywords)] {
        if !val.is_empty() {
            info_body.push_str(&format!(" /{key} ({})", pdf_escape(val)));
        }
    }
    if !info_body.is_empty() {
        emit(&mut out, &mut offsets, info_num, format!("<<{info_body} >>").as_bytes());
        info_ref = format!(" /Info {info_num} 0 R");
    }

    let size = offsets.len() as u32;
    let xref_at = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets.iter().skip(1) {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R{info_ref} >>\nstartxref\n{xref_at}\n%%EOF").as_bytes());
    Ok(out)
}

// ---------------------------------------------------------------------------
// TIFF (multipage, uncompressed RGB, DPI tags)
// ---------------------------------------------------------------------------

/// Multipage little-endian TIFF, one strip per page, X/YResolution in inches.
pub fn write_tiff(pages: Vec<TiffPage>) -> Result<Vec<u8>, ExportError> {
    check_pages(&pages)?;
    for p in &pages {
        check_dpi(p.dpi)?;
    }
    // Layout: header, then per page: IFD (with placeholders), then after all
    // IFDs the rational blobs + strip data. Two passes keep offsets simple.
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"II*\0"); // little-endian magic
    let first_ifd_at = out.len();
    out.extend_from_slice(&[0u8; 4]); // patched below

    struct Fixup {
        ifd_at: usize,
        next_entry_at: usize,
        strip_offset_entry_at: usize,
        xres_entry_at: usize,
        yres_entry_at: usize,
    }

    let mut fixups: Vec<Fixup> = Vec::new();
    for page in &pages {
        let (w, h) = page.image.dims();
        let ifd_at = out.len();
        // 11 entries: W,H,BitsPerSample,Compression,Photometric,SamplesPerPixel,
        // StripOffsets,RowsPerStrip,StripByteCounts,XResolution,YResolution.
        const ENTRIES: u16 = 11;
        out.extend_from_slice(&ENTRIES.to_le_bytes());
        let base = out.len();
        out.extend_from_slice(&[0u8; (ENTRIES as usize) * 12]);
        let next_entry_at = out.len();
        out.extend_from_slice(&[0u8; 4]);
        let entry = |i: usize| base + i * 12 + 8; // offset of value field
        fixups.push(Fixup { ifd_at, next_entry_at, strip_offset_entry_at: entry(6), xres_entry_at: entry(9), yres_entry_at: entry(10) });
        let mut e = |i: usize, id: u16, typ: u16, count: u32, v: u32| {
            let at = base + i * 12;
            out[at..at + 2].copy_from_slice(&id.to_le_bytes());
            out[at + 2..at + 4].copy_from_slice(&typ.to_le_bytes());
            out[at + 4..at + 8].copy_from_slice(&count.to_le_bytes());
            out[at + 8..at + 12].copy_from_slice(&v.to_le_bytes());
        };
        e(0, 256, 3, 1, w); // ImageWidth SHORT
        e(1, 257, 3, 1, h.min(65535)); // ImageLength SHORT (patched to LONG when taller)
        e(2, 258, 3, 3, 0); // BitsPerSample -> offset patched (needs 3 SHORTs)
        e(3, 259, 3, 1, 1); // Compression = none
        e(4, 262, 3, 1, 2); // Photometric = RGB
        e(5, 277, 3, 1, 3); // SamplesPerPixel = 3
        e(6, 273, 4, 1, 0); // StripOffsets LONG (patched)
        e(7, 278, 4, 1, h); // RowsPerStrip LONG
        e(8, 279, 4, 1, (w as usize * h as usize * 3) as u32); // StripByteCounts
        e(9, 282, 5, 1, 0); // XResolution RATIONAL (patched)
        e(10, 283, 5, 1, 0); // YResolution RATIONAL (patched)
    }
    // Height can exceed SHORT: patch ImageLength to LONG when needed.
    // (Tag 257 entry index 1; rewrite as LONG in place.)
    for (fi, page) in pages.iter().enumerate() {
        let (_, h) = page.image.dims();
        if h > 65535 {
            let base = fixups[fi].ifd_at + 2;
            let at = base + 12;
            out[at + 2..at + 4].copy_from_slice(&4u16.to_le_bytes()); // LONG
            out[at + 8..at + 12].copy_from_slice(&h.to_le_bytes());
        }
    }
    // Chain IFDs.
    for i in 0..fixups.len() {
        let next = if i + 1 < fixups.len() { fixups[i + 1].ifd_at as u32 } else { 0 };
        let at = fixups[i].next_entry_at;
        out[at..at + 4].copy_from_slice(&next.to_le_bytes());
    }
    out[first_ifd_at..first_ifd_at + 4].copy_from_slice(&(fixups[0].ifd_at as u32).to_le_bytes());

    // Blobs: BitsPerSample(8,8,8), X/YResolution rationals, then strip data.
    // BitsPerSample offsets (entry index 2).
    for (fi, _page) in pages.iter().enumerate() {
        let base = fixups[fi].ifd_at + 2;
        let at = base + 2 * 12;
        let blob_at = out.len() as u32;
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out[at + 2..at + 4].copy_from_slice(&3u16.to_le_bytes()); // SHORT
        out[at + 4..at + 8].copy_from_slice(&3u32.to_le_bytes()); // count
        out[at + 8..at + 12].copy_from_slice(&blob_at.to_le_bytes());
    }
    for (fi, page) in pages.iter().enumerate() {
        for entry_at in [fixups[fi].xres_entry_at, fixups[fi].yres_entry_at] {
            let blob_at = out.len() as u32;
            out.extend_from_slice(&page.dpi.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
            out[entry_at..entry_at + 4].copy_from_slice(&blob_at.to_le_bytes());
        }
    }
    for (fi, page) in pages.iter().enumerate() {
        let strip_at = out.len() as u32;
        out.extend_from_slice(page.image.pixels());
        let at = fixups[fi].strip_offset_entry_at;
        out[at..at + 4].copy_from_slice(&strip_at.to_le_bytes());
    }
    Ok(out)
}

/// One TIFF page.
pub struct TiffPage {
    pub image: Image,
    pub dpi: u32,
}

// ---------------------------------------------------------------------------
// JPEG / PNG singles
// ---------------------------------------------------------------------------

pub fn write_jpeg(image: &Image, quality: u8) -> Result<Vec<u8>, ExportError> {
    if quality > 100 {
        return Err(ExportError::Image(ImgError::Unsupported("jpeg quality must be 0..=100")));
    }
    Ok(image.encode_jpeg(quality)?)
}

pub fn write_png(image: &Image) -> Result<Vec<u8>, ExportError> {
    Ok(image.encode_png()?)
}

// ---------------------------------------------------------------------------
// Filename templates
// ---------------------------------------------------------------------------

/// Civil date/time parts (UTC) for `$(...)` placeholders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Stamp {
    /// Build from unix seconds (UTC) without external date crates.
    pub fn from_unix(secs: u64) -> Self {
        let days = (secs / 86_400) as i64;
        let sod = (secs % 86_400) as u32;
        // Howard Hinnant's civil_from_days.
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = (z - era * 146_097) as u32;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Self { year: (if m <= 2 { y + 1 } else { y }) as i32, month: m, day: d, hour: sod / 3_600, minute: (sod % 3_600) / 60, second: sod % 60 }
    }

    pub fn now() -> Self {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        Self::from_unix(secs)
    }
}

/// Render `$(YYYY) $(MM) $(DD) $(hh) $(mm) $(ss) $(n)` then sanitize for
/// Windows filenames. Unknown `$(...)` tokens are an error (typo-proof).
pub fn render_filename(pattern: &str, stamp: Stamp, n: u32) -> Result<String, ExportError> {
    if pattern.trim().is_empty() {
        return Err(ExportError::BadTemplate("pattern is empty".to_owned()));
    }
    let mut out = String::with_capacity(pattern.len() + 16);
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'(') {
            let Some(end) = pattern[i..].find(')') else {
                return Err(ExportError::BadTemplate("unterminated $(…)".to_owned()));
            };
            let token = &pattern[i + 2..i + end];
            match token {
                "YYYY" => out.push_str(&format!("{:04}", stamp.year)),
                "MM" => out.push_str(&format!("{:02}", stamp.month)),
                "DD" => out.push_str(&format!("{:02}", stamp.day)),
                "hh" => out.push_str(&format!("{:02}", stamp.hour)),
                "mm" => out.push_str(&format!("{:02}", stamp.minute)),
                "ss" => out.push_str(&format!("{:02}", stamp.second)),
                "n" => out.push_str(&format!("{n}")),
                _ => return Err(ExportError::BadTemplate(format!("unknown token $({token})"))),
            }
            i += end + 1;
        } else {
            if let Some(c) = pattern[i..].chars().next() {
                out.push(c);
                i += c.len_utf8();
            } else {
                break;
            }
        }
    }
    Ok(sanitize_filename(&out))
}

fn sanitize_filename(name: &str) -> String {
    let mut out: String =
        name.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20 { '_' } else { c }).collect();
    // Windows dislikes trailing dots/spaces.
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    if out.is_empty() {
        out.push('_');
    }
    // Reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9).
    let upper = out.to_ascii_uppercase();
    let stem = upper.split('.').next().unwrap_or("");
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
        "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved.contains(&stem) {
        out = format!("_{out}");
    }
    // Cap length (leave room for suffix + extension).
    if out.len() > 180 {
        let mut cut = 180;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

/// Pick a non-colliding path: `stem.ext`, `stem (2).ext`, … Never overwrites.
pub fn unique_path(dir: &Path, stem: &str, ext: &str, max_tries: u32) -> Result<PathBuf, ExportError> {
    for i in 1..=max_tries.max(1) {
        let name = if i == 1 { format!("{stem}.{ext}") } else { format!("{stem} ({i}).{ext}") };
        let path = dir.join(name);
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(ExportError::Io("could not find a free filename".to_owned()))
}

// ---------------------------------------------------------------------------
// Atomic writes
// ---------------------------------------------------------------------------

static ATOMIC_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Write all bytes atomically: temp file in the same directory + rename.
/// The parent directory must exist (explicit, no silent mkdir).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| ExportError::Io("path has no parent".to_owned()))?;
    if !parent.is_dir() {
        return Err(ExportError::Io(format!("folder does not exist: {}", parent.display())));
    }
    let tag = ATOMIC_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("out");
    let tmp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let tmp = tmp.with_extension(format!("{tag}.tmp"));
    std::fs::write(&tmp, bytes).map_err(|e| ExportError::Io(format!("write failed: {e}")))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ExportError::Io(format!("rename failed: {e}"))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(w: u32, h: u32) -> Image {
        let mut px = vec![0u8; (w as usize) * (h as usize) * 3];
        for (i, v) in px.iter_mut().enumerate() {
            *v = (i % 251) as u8;
        }
        Image::new(w, h, px).unwrap()
    }

    #[test]
    fn pdf_structure_validates() {
        let pages = vec![PdfPage { image: rgb(100, 140), dpi: 200 }, PdfPage { image: rgb(50, 50), dpi: 100 }];
        let meta = PdfMeta { title: "T (x)".to_owned(), ..Default::default() };
        let pdf = write_pdf(pages, &meta).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.7"));
        assert!(pdf.ends_with(b"%%EOF"));
        assert!(windows_contains(&pdf, b"/Count 2"));
        assert!(windows_contains(&pdf, b"/Title (T \\(x\\))"));
        // MediaBox: 100px @200dpi = 36pt; 50px @100dpi = 36pt.
        assert!(windows_contains(&pdf, b"/MediaBox [0 0 36.00 50.40]"));
        check_xref(&pdf);
    }

    #[test]
    fn pdf_rejects_bad_input() {
        assert_eq!(write_pdf(vec![], &PdfMeta::default()), Err(ExportError::EmptyDocument));
        assert_eq!(write_pdf(vec![PdfPage { image: rgb(8, 8), dpi: 0 }], &PdfMeta::default()), Err(ExportError::BadDpi(0)));
    }

    #[test]
    fn tiff_multipage_roundtrip_first_frame() {
        let pages = vec![TiffPage { image: rgb(64, 48), dpi: 300 }, TiffPage { image: rgb(32, 32), dpi: 150 }];
        let tiff = write_tiff(pages).unwrap();
        assert_eq!(&tiff[0..4], b"II*\0");
        // Two IFDs chained.
        let first = u32::from_le_bytes(tiff[4..8].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(tiff[first..first + 2].try_into().unwrap());
        assert_eq!(count, 11);
        let next_at = first + 2 + 11 * 12;
        let second = u32::from_le_bytes(tiff[next_at..next_at + 4].try_into().unwrap()) as usize;
        assert!(second > first);
        // First frame decodes via the `image` crate with right dims + DPI.
        let dynimg = match image::load_from_memory_with_format(&tiff, image::ImageFormat::Tiff) {
            Ok(img) => img,
            Err(e) => panic!("tiff decode failed: {e}"),
        };
        assert_eq!((dynimg.width(), dynimg.height()), (64, 48));
    }

    #[test]
    fn jpeg_png_roundtrip() {
        let img = rgb(40, 30);
        let jpg = write_jpeg(&img, 85).unwrap();
        assert_eq!(&jpg[0..2], &[0xFF, 0xD8]);
        let back = Image::decode(&jpg).unwrap();
        assert_eq!(back.dims(), (40, 30));
        let png = write_png(&img).unwrap();
        assert_eq!(Image::decode(&png).unwrap(), img);
        assert_eq!(write_jpeg(&img, 101), Err(ExportError::Image(ImgError::Unsupported("jpeg quality must be 0..=100"))));
    }

    #[test]
    fn templates_render_and_reject() {
        // 2026-10-14 19:00:00 UTC = 1792004400.
        let stamp = Stamp::from_unix(1_792_004_400);
        assert_eq!((stamp.year, stamp.month, stamp.day, stamp.hour), (2026, 10, 14, 19));
        let name = render_filename("scan $(YYYY)-$(MM)-$(DD) $(hh)$(mm)$(ss) $(n)", stamp, 7).unwrap();
        assert_eq!(name, "scan 2026-10-14 190000 7");
        assert!(render_filename("x $(bogus)", stamp, 1).is_err());
        assert!(render_filename("", stamp, 1).is_err());
        assert_eq!(render_filename("a/b:c", stamp, 1).unwrap(), "a_b_c");
        assert_eq!(render_filename("CON", stamp, 1).unwrap(), "_CON");
    }

    #[test]
    fn atomic_write_and_collision() {
        let dir = std::env::temp_dir().join(format!("scancraft-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p1 = unique_path(&dir, "doc", "pdf", 10).unwrap();
        write_atomic(&p1, b"one").unwrap();
        assert_eq!(std::fs::read(&p1).unwrap(), b"one");
        let p2 = unique_path(&dir, "doc", "pdf", 10).unwrap();
        assert!(p2 != p1);
        assert!(p2.to_string_lossy().contains("(2)"));
        write_atomic(&p2, b"two").unwrap();
        assert_eq!(std::fs::read(&p2).unwrap(), b"two");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn windows_contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// Parse the xref table and check every offset points at "N 0 obj".
    fn check_xref(pdf: &[u8]) {
        let tail = pdf.len().saturating_sub(64);
        let tail_str = std::str::from_utf8(&pdf[tail..]).unwrap();
        let sx_pos = tail_str.rfind("startxref").unwrap();
        let start: usize = tail_str[sx_pos + 9..].trim().trim_end_matches("%%EOF").trim().parse().unwrap();
        assert_eq!(&pdf[start..start + 4], b"xref");
        let mut lines = std::str::from_utf8(&pdf[start..]).unwrap().lines();
        lines.next(); // xref
        let header: Vec<&str> = lines.next().unwrap().split_whitespace().collect();
        assert_eq!(header[0], "0");
        let count: usize = header[1].parse().unwrap();
        lines.next(); // free entry
        for obj in 1..count {
            let line = lines.next().unwrap();
            let off: usize = line[..10].parse().unwrap();
            assert!(pdf[off..].starts_with(format!("{obj} 0 obj").as_bytes()), "obj {obj} at {off}");
        }
    }
}
