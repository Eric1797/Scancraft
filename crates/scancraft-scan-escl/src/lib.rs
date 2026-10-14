//! eSCL / AirScan network scanner backend.
//!
//! Protocol source: the public Mopria eSCL specification (resources
//! `ScannerCapabilities`, `ScannerStatus`, `ScanJobs`, `NextDocument`,
//! DELETE-to-cancel) and the wire vocabulary confirmed against MIT/Apache
//! client implementations — NOT against any GPL code.
//!
//! Transport is plain HTTP over the LAN (most printers serve eSCL on
//! `http://host/eSCL`). No TLS yet: printers' self-signed certs need the
//! per-device trust UI first (M6b). No mDNS discovery yet: hosts are
//! configured explicitly (M6b adds `_uscan._tcp` discovery).

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use scancraft_model::{BitDepth, PageSide, PageSize, PaperSource};
use scancraft_scan_core::{CancelToken, Capabilities, DeviceId, DeviceInfo, PageSink, Progress, RawPage, ScanError, ScanRequest, ScannerBackend};

// ---------------------------------------------------------------------------
// Minimal blocking HTTP/1.1 client (GET/POST/DELETE, plain HTTP only)
// ---------------------------------------------------------------------------

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_BODY: usize = 512 * 1024 * 1024;

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HttpError {
    UnsupportedScheme,
    BadUrl(String),
    Dns(String),
    Connect(String),
    Io(String),
    BadResponse(String),
    BodyTooLarge,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::UnsupportedScheme => write!(f, "only http:// URLs are supported (no TLS yet)"),
            HttpError::BadUrl(msg) => write!(f, "bad URL: {msg}"),
            HttpError::Dns(msg) => write!(f, "cannot resolve scanner host: {msg}"),
            HttpError::Connect(msg) => write!(f, "cannot reach scanner: {msg}"),
            HttpError::Io(msg) => write!(f, "network error: {msg}"),
            HttpError::BadResponse(msg) => write!(f, "bad HTTP response: {msg}"),
            HttpError::BodyTooLarge => write!(f, "response body exceeds size cap"),
        }
    }
}

impl std::error::Error for HttpError {}

/// Split `http://host[:port][/path]` into (host, port, path).
fn split_url(url: &str) -> Result<(String, u16, String), HttpError> {
    let rest = url.strip_prefix("http://").ok_or(HttpError::UnsupportedScheme)?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_owned()),
        None => (rest, "/".to_owned()),
    };
    if authority.is_empty() {
        return Err(HttpError::BadUrl("empty host".to_owned()));
    }
    let (host, port) = match authority.rfind(':') {
        Some(i) => {
            let port: u16 = authority[i + 1..].parse().map_err(|_| HttpError::BadUrl("bad port".to_owned()))?;
            (authority[..i].to_owned(), port)
        }
        None => (authority.to_owned(), 80),
    };
    if host.is_empty() {
        return Err(HttpError::BadUrl("empty host".to_owned()));
    }
    Ok((host, port, path))
}

fn read_response(stream: &mut TcpStream) -> Result<HttpResponse, HttpError> {
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    // Read until headers complete.
    loop {
        let n = stream.read(&mut buf).map_err(|e| HttpError::Io(trim_err(e)))?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if raw.len() > 1_048_576 {
            return Err(HttpError::BadResponse("headers too large".to_owned()));
        }
        if find_headers_end(&raw).is_some() {
            break;
        }
    }
    let head_end = find_headers_end(&raw).ok_or_else(|| HttpError::BadResponse("no header terminator".to_owned()))?;
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.lines();
    let status_line = lines.next().ok_or_else(|| HttpError::BadResponse("empty response".to_owned()))?;
    let status: u16 =
        status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| HttpError::BadResponse("bad status line".to_owned()))?;
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let resp = HttpResponse { status, headers, body: Vec::new() };
    let mut body = raw[head_end..].to_vec();
    if resp.header("Transfer-Encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        body = read_chunked(stream, body)?;
    } else if let Some(len) = resp.header("Content-Length").and_then(|v| v.parse::<usize>().ok()) {
        if len > MAX_BODY {
            return Err(HttpError::BodyTooLarge);
        }
        while body.len() < len {
            let n = stream.read(&mut buf).map_err(|e| HttpError::Io(trim_err(e)))?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
            if body.len() > MAX_BODY {
                return Err(HttpError::BodyTooLarge);
            }
        }
        body.truncate(len);
    } else {
        // No length and not chunked: read until close.
        loop {
            let n = stream.read(&mut buf).map_err(|e| HttpError::Io(trim_err(e)))?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
            if body.len() > MAX_BODY {
                return Err(HttpError::BodyTooLarge);
            }
        }
    }
    Ok(HttpResponse { status, headers: resp.headers, body })
}

fn find_headers_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn read_chunked(stream: &mut TcpStream, mut pending: Vec<u8>) -> Result<Vec<u8>, HttpError> {
    let mut body: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let line_end = find_line_end(&pending).ok_or_else(|| HttpError::BadResponse("bad chunk".to_owned()))?;
        let line = String::from_utf8_lossy(&pending[..line_end]).into_owned();
        let size_str = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16).map_err(|_| HttpError::BadResponse("bad chunk size".to_owned()))?;
        pending.drain(..line_end + 2);
        if size == 0 {
            break;
        }
        if body.len().saturating_add(size) > MAX_BODY {
            return Err(HttpError::BodyTooLarge);
        }
        while pending.len() < size + 2 {
            let n = stream.read(&mut buf).map_err(|e| HttpError::Io(trim_err(e)))?;
            if n == 0 {
                return Err(HttpError::BadResponse("truncated chunk".to_owned()));
            }
            pending.extend_from_slice(&buf[..n]);
        }
        body.extend_from_slice(&pending[..size]);
        pending.drain(..size + 2);
    }
    Ok(body)
}

fn find_line_end(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\r\n")
}

fn http_request(method: &str, url: &str, extra_headers: &[(&str, &str)], body: Option<&[u8]>, timeout: Duration) -> Result<HttpResponse, HttpError> {
    let (host, port, path) = split_url(url)?;
    let addr: SocketAddr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| HttpError::Dns(trim_err(e)))?
        .next()
        .ok_or_else(|| HttpError::Dns("no address".to_owned()))?;
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|e| HttpError::Connect(trim_err(e)))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| HttpError::Io(trim_err(e)))?;
    stream.set_write_timeout(Some(timeout)).map_err(|e| HttpError::Io(trim_err(e)))?;
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    match body {
        Some(b) => req.push_str(&format!("Content-Length: {}\r\n\r\n", b.len())),
        None => req.push_str("\r\n"),
    }
    stream.write_all(req.as_bytes()).map_err(|e| HttpError::Io(trim_err(e)))?;
    if let Some(b) = body {
        stream.write_all(b).map_err(|e| HttpError::Io(trim_err(e)))?;
    }
    read_response(&mut stream)
}

fn trim_err<E: fmt::Display>(e: E) -> String {
    e.to_string().chars().take(200).collect()
}

// ---------------------------------------------------------------------------
// eSCL XML: lenient local-name parsing + ScanSettings builder
// ---------------------------------------------------------------------------

/// (element path with namespace prefixes stripped, text) pairs.
fn flatten_xml(xml: &str) -> Vec<(String, String)> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                stack.push(name);
            }
            Ok(Event::Text(e)) => {
                if let Ok(text) = e.decode() {
                    let text = text.trim().to_owned();
                    if !text.is_empty() && !stack.is_empty() {
                        out.push((stack.join("/"), text));
                    }
                }
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if !stack.is_empty() {
                    out.push((format!("{}/{}", stack.join("/"), name), String::new()));
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            Ok(_) => {} // comments, declarations, processing instructions
        }
        buf.clear();
    }
    out
}

fn find_text(entries: &[(String, String)], suffix: &str) -> Option<String> {
    entries.iter().rev().find(|(path, _)| path == suffix || path.ends_with(&format!("/{suffix}"))).map(|(_, v)| v.clone())
}

fn find_all(entries: &[(String, String)], suffix: &str) -> Vec<String> {
    entries.iter().filter(|(path, _)| path == suffix || path.ends_with(&format!("/{suffix}"))).map(|(_, v)| v.clone()).collect()
}

/// Parsed capabilities (only what the backend needs; everything optional).
#[derive(Debug, Clone, Default)]
struct EsclCaps {
    make_and_model: String,
    version: String,
    has_platen: bool,
    has_feeder: bool,
    duplex: bool,
    color_modes: Vec<String>,
    doc_formats: Vec<String>,
    resolutions: Vec<u32>,
    max_width_300ths: Option<u32>,
    max_height_300ths: Option<u32>,
}

fn parse_capabilities(xml: &str) -> EsclCaps {
    let entries = flatten_xml(xml);
    let has_platen = entries.iter().any(|(p, _)| p.split('/').any(|s| s.eq_ignore_ascii_case("Platen")));
    let has_feeder = entries.iter().any(|(p, _)| {
        p.split('/').any(|s| {
            let l = s.to_lowercase();
            l.contains("adf") || l.contains("feeder")
        })
    });
    let duplex = entries.iter().any(|(p, v)| p.to_lowercase().contains("duplex") || v.to_lowercase().contains("duplex"));
    let color_modes = find_all(&entries, "ColorMode");
    let mut doc_formats = find_all(&entries, "DocumentFormat");
    doc_formats.extend(find_all(&entries, "DocumentFormatExt"));
    let mut resolutions: Vec<u32> = find_all(&entries, "XResolution").iter().filter_map(|s| s.parse().ok()).collect();
    resolutions.sort_unstable();
    resolutions.dedup();
    // Full-area region in three-hundredths of an inch (prefer platen, else feeder).
    let max_width_300ths = find_text(&entries, "Platen/PlatenInputArea/MaxWidth")
        .or_else(|| find_text(&entries, "Platen/PlatenInputCaps/MaxWidth"))
        .or_else(|| find_text(&entries, "MaxWidth"))
        .and_then(|s| s.parse().ok());
    let max_height_300ths = find_text(&entries, "Platen/PlatenInputArea/MaxHeight")
        .or_else(|| find_text(&entries, "Platen/PlatenInputCaps/MaxHeight"))
        .or_else(|| find_text(&entries, "MaxHeight"))
        .and_then(|s| s.parse().ok());
    EsclCaps {
        make_and_model: find_text(&entries, "MakeAndModel").unwrap_or_default(),
        version: find_text(&entries, "Version").unwrap_or_default(),
        has_platen,
        has_feeder,
        duplex,
        color_modes,
        doc_formats,
        resolutions,
        max_width_300ths,
        max_height_300ths,
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Build the POST body for `/ScanJobs`. Region covers the full advertised
/// area (three-hundredths of an inch); JPEG is requested when advertised.
fn build_scan_settings(caps: &EsclCaps, req: &ScanRequest, full_w: u32, full_h: u32) -> String {
    let version = if caps.version.is_empty() { "2.0".to_owned() } else { caps.version.clone() };
    let input_source = match req.paper_source {
        PaperSource::Flatbed => "Platen",
        PaperSource::Feeder | PaperSource::Duplex => "Feeder",
    };
    let duplex = if req.paper_source == PaperSource::Duplex { "<scan:Duplex>true</scan:Duplex>" } else { "" };
    let color_mode = match req.bit_depth {
        scancraft_model::BitDepth::Color if has_mode(&caps.color_modes, "RGB24") => "<scan:ColorMode>RGB24</scan:ColorMode>",
        scancraft_model::BitDepth::Grayscale if has_mode(&caps.color_modes, "Grayscale8") => "<scan:ColorMode>Grayscale8</scan:ColorMode>",
        scancraft_model::BitDepth::BlackWhite if has_mode(&caps.color_modes, "BlackAndWhite1") => "<scan:ColorMode>BlackAndWhite1</scan:ColorMode>",
        _ => "",
    };
    let wants_jpeg = caps.doc_formats.iter().any(|f| f.eq_ignore_ascii_case("image/jpeg"));
    let doc_format = if wants_jpeg { "<scan:DocumentFormat>image/jpeg</scan:DocumentFormat>" } else { "" };
    let (w, h) = (full_w.max(1), full_h.max(1));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <scan:ScanSettings xmlns:scan=\"http://schemas.hp.com/imaging/escl/2011/05/03\" \
         xmlns:pwg=\"http://www.pwg.org/schemas/2010/12/sm\">\n\
         <pwg:Version>{}</pwg:Version>\n\
         <scan:Intent>Document</scan:Intent>\n\
         <pwg:ScanRegions><pwg:ScanRegion>\
         <pwg:Height>{h}</pwg:Height>\
         <pwg:ContentRegionUnits>escl:ThreeHundredthsOfInches</pwg:ContentRegionUnits>\
         <pwg:Width>{w}</pwg:Width>\
         <pwg:XOffset>0</pwg:XOffset><pwg:YOffset>0</pwg:YOffset>\
         </pwg:ScanRegion></pwg:ScanRegions>\n\
         <pwg:InputSource>{input_source}</pwg:InputSource>\n\
         {duplex}\
         <scan:XResolution>{}</scan:XResolution>\n\
         <scan:YResolution>{}</scan:YResolution>\n\
         {color_mode}\
         {doc_format}\
         </scan:ScanSettings>",
        xml_escape(&version),
        req.dpi,
        req.dpi,
    )
}

fn has_mode(modes: &[String], want: &str) -> bool {
    modes.iter().any(|m| m.eq_ignore_ascii_case(want))
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Network scanner backend. Hosts are plain `http://host[:port]` roots;
/// `/eSCL` is appended automatically when the bare host does not serve it.
#[derive(Debug, Clone, Default)]
pub struct EsclScanner {
    hosts: Vec<String>,
}

impl EsclScanner {
    pub fn with_hosts(hosts: Vec<String>) -> Self {
        Self { hosts }
    }

    /// Probe a host and return the working eSCL base URL, if any.
    fn probe_base(host: &str) -> Option<String> {
        let host = host.trim().trim_end_matches('/');
        if host.is_empty() {
            return None;
        }
        for base in [format!("{host}/eSCL"), host.to_owned()] {
            let url = format!("{base}/ScannerCapabilities");
            if let Ok(resp) = http_request("GET", &url, &[], None, PROBE_TIMEOUT)
                && resp.status == 200
                && !resp.body.is_empty()
            {
                return Some(base);
            }
        }
        None
    }

    fn fetch_caps(base: &str) -> Result<EsclCaps, ScanError> {
        let url = format!("{base}/ScannerCapabilities");
        let resp = http_request("GET", &url, &[], None, IO_TIMEOUT).map_err(map_http)?;
        if resp.status == 503 {
            return Err(ScanError::Busy);
        }
        if resp.status != 200 {
            return Err(ScanError::Driver(format!("capabilities request failed (HTTP {})", resp.status)));
        }
        let xml = String::from_utf8_lossy(&resp.body).into_owned();
        Ok(parse_capabilities(&xml))
    }

    fn nearest_dpi(advertised: &[u32], want: u32) -> u32 {
        if advertised.contains(&want) {
            return want;
        }
        if advertised.is_empty() {
            return want;
        }
        let mut best = advertised[0];
        for dpi in advertised {
            if dpi.abs_diff(want) < best.abs_diff(want) {
                best = *dpi;
            }
        }
        best
    }
}

fn map_http(e: HttpError) -> ScanError {
    match e {
        HttpError::UnsupportedScheme => ScanError::Unsupported("eSCL needs plain http:// (no TLS yet)".to_owned()),
        HttpError::Dns(_) | HttpError::Connect(_) => ScanError::Io(e.to_string()),
        HttpError::BodyTooLarge => ScanError::Driver("scanner response too large".to_owned()),
        HttpError::BadUrl(_) | HttpError::BadResponse(_) | HttpError::Io(_) => ScanError::Io(e.to_string()),
    }
}

fn caps_to_model(caps: &EsclCaps) -> Capabilities {
    let mut sources = Vec::new();
    if caps.has_platen {
        sources.push(PaperSource::Flatbed);
    }
    if caps.has_feeder {
        sources.push(PaperSource::Feeder);
        if caps.duplex {
            sources.push(PaperSource::Duplex);
        }
    }
    if sources.is_empty() {
        sources.push(PaperSource::Flatbed);
    }
    let mut dpis: Vec<u32> = caps.resolutions.iter().copied().filter(|d| (50..=1200).contains(d)).collect();
    dpis.sort_unstable();
    dpis.dedup();
    let mut depths = Vec::new();
    if caps.color_modes.iter().any(|m| m.to_uppercase().contains("RGB")) {
        depths.push(BitDepth::Color);
    }
    if caps.color_modes.iter().any(|m| m.to_uppercase().contains("GRAY")) {
        depths.push(BitDepth::Grayscale);
    }
    if caps.color_modes.iter().any(|m| m.to_uppercase().contains("BLACKANDWHITE")) {
        depths.push(BitDepth::BlackWhite);
    }
    if depths.is_empty() {
        depths.push(BitDepth::Color);
    }
    // Advertised full area → a Custom page size (three-hundredths of an inch).
    let mut page_sizes = Vec::new();
    if let (Some(w), Some(h)) = (caps.max_width_300ths, caps.max_height_300ths) {
        let (w_mm, h_mm) = (f64::from(w) * 25.4 / 300.0, f64::from(h) * 25.4 / 300.0);
        if w_mm > 1.0 && h_mm > 1.0 {
            page_sizes.push(PageSize::Custom { width_mm: w_mm, height_mm: h_mm });
        }
    }
    Capabilities { sources, dpis, bit_depths: depths, page_sizes, duplex: caps.duplex, max_pages_per_run: None }
}

fn detect_image_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(b"%PDF") {
        Some("pdf")
    } else if (bytes.starts_with(b"II") && bytes.get(2..4) == Some(&[42u8, 0][..])) || bytes.starts_with(b"MM\x00*") {
        Some("tiff")
    } else {
        None
    }
}

impl ScannerBackend for EsclScanner {
    fn id(&self) -> &'static str {
        "escl"
    }

    fn list_devices(&self, cancel: &CancelToken) -> Result<Vec<DeviceInfo>, ScanError> {
        let mut out = Vec::new();
        for host in &self.hosts {
            cancel.check()?;
            if let Some(base) = Self::probe_base(host) {
                let name = Self::fetch_caps(&base).map(|c| c.make_and_model).unwrap_or_default();
                let display_name = if name.is_empty() { format!("Network scanner ({base})") } else { format!("{name} ({base})") };
                out.push(DeviceInfo { id: DeviceId(format!("escl:{base}")), backend: "escl", display_name });
            }
        }
        Ok(out)
    }

    fn capabilities(&self, device: &DeviceId) -> Result<Capabilities, ScanError> {
        let base = device.0.strip_prefix("escl:").ok_or_else(|| ScanError::Unsupported("not an eSCL device".to_owned()))?;
        Ok(caps_to_model(&Self::fetch_caps(base)?))
    }

    fn scan(&self, request: &ScanRequest, sink: &mut dyn PageSink, cancel: &CancelToken) -> Result<u32, ScanError> {
        let base = request.device.0.strip_prefix("escl:").ok_or_else(|| ScanError::Unsupported("not an eSCL device".to_owned()))?.to_owned();
        let caps = Self::fetch_caps(&base)?;
        let dpi = Self::nearest_dpi(&caps.resolutions, request.dpi);
        let (full_w, full_h) =
            (caps.max_width_300ths.unwrap_or((210.0 * 300.0 / 25.4) as u32), caps.max_height_300ths.unwrap_or((297.0 * 300.0 / 25.4) as u32));
        let settings = build_scan_settings(&caps, &ScanRequest { dpi, ..request.clone() }, full_w, full_h);

        // Create the job.
        let create_url = format!("{base}/ScanJobs");
        let resp = http_request("POST", &create_url, &[("Content-Type", "text/xml; charset=utf-8")], Some(settings.as_bytes()), IO_TIMEOUT)
            .map_err(map_http)?;
        let location = match resp.status {
            201 => resp.header("Location").ok_or_else(|| ScanError::Driver("scanner created a job without a Location".to_owned()))?.to_owned(),
            503 => return Err(ScanError::Busy),
            400 | 409 | 415 => {
                return Err(ScanError::Unsupported(format!("scanner refused the settings (HTTP {})", resp.status)));
            }
            s => return Err(ScanError::Driver(format!("job creation failed (HTTP {s})"))),
        };
        let job_url = normalize_job_url(&base, &location);

        let outcome = self.fetch_pages(&job_url, dpi, request, sink, cancel);
        // Always release the job (best effort).
        let _ = http_request("DELETE", &job_url, &[], None, IO_TIMEOUT);
        outcome
    }
}

/// Join a possibly-relative Location against the eSCL base.
fn normalize_job_url(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_owned();
    }
    let base = base.trim_end_matches('/');
    let loc = location.trim_start_matches('/');
    if loc.starts_with("eSCL/") { format!("{}/{}", base.trim_end_matches("/eSCL"), loc) } else { format!("{base}/{loc}") }
}

impl EsclScanner {
    fn fetch_pages(&self, job_url: &str, dpi: u32, request: &ScanRequest, sink: &mut dyn PageSink, cancel: &CancelToken) -> Result<u32, ScanError> {
        let mut delivered: u32 = 0;
        let mut busy_retries = 0u32;
        loop {
            cancel.check()?;
            let url = format!("{}/NextDocument", job_url.trim_end_matches('/'));
            let resp = http_request("GET", &url, &[], None, IO_TIMEOUT).map_err(map_http)?;
            match resp.status {
                200 => {
                    busy_retries = 0;
                    let kind = detect_image_format(&resp.body);
                    match kind {
                        Some("jpeg") | Some("png") | Some("tiff") => {
                            let img = scancraft_imaging::Image::decode(&resp.body)
                                .map_err(|e| ScanError::Driver(format!("cannot decode scanned page: {e}")))?;
                            let (w, h) = img.dims();
                            let side = if delivered.is_multiple_of(2) { PageSide::Front } else { PageSide::Back };
                            let page = RawPage { width_px: w, height_px: h, dpi, bit_depth: request.bit_depth, side, pixels_rgb8: img.into_pixels() };
                            page.validate()?;
                            sink.page(page)?;
                            delivered += 1;
                            sink.progress(Progress { pages_done: delivered, stage: "scanning" });
                        }
                        Some("pdf") => {
                            return Err(ScanError::Unsupported(
                                "the scanner returned PDF pages; PDF raster import lands in M2b (save as JPEG on the device for now)".to_owned(),
                            ));
                        }
                        Some(_) | None => return Err(ScanError::Driver("scanner returned an unknown page format".to_owned())),
                    }
                }
                404 => break, // no more documents
                503 if busy_retries < 20 => {
                    busy_retries += 1;
                    std::thread::sleep(Duration::from_millis(500));
                    cancel.check()?;
                }
                503 => return Err(ScanError::Busy),
                s => return Err(ScanError::Driver(format!("page fetch failed (HTTP {s})"))),
            }
        }
        Ok(delivered)
    }
}

// ---------------------------------------------------------------------------
// Tests against an in-process mock eSCL server (std TcpListener, no deps)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use scancraft_model::ScanProfile;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CAPS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<scan:ScannerCapabilities xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm">
<pwg:Version>2.63</pwg:Version>
<pwg:MakeAndModel>MockPrinter M1</pwg:MakeAndModel>
<pwg:SerialNumber>SN123</pwg:SerialNumber>
<scan:UUID>uuid-1</scan:UUID>
<scan:AdminURI>http://mock/</scan:AdminURI>
<scan:Platen>
<scan:PlatenInputArea>
<pwg:MinWidth>0</pwg:MinWidth><pwg:MaxWidth>2550</pwg:MaxWidth>
<pwg:MinHeight>0</pwg:MinHeight><pwg:MaxHeight>3508</pwg:MaxHeight>
</scan:PlatenInputArea>
<scan:SettingProfiles><scan:SettingProfile>
<scan:ColorModes><scan:ColorMode>RGB24</scan:ColorMode><scan:ColorMode>Grayscale8</scan:ColorMode></scan:ColorModes>
<scan:DocumentFormats><scan:DocumentFormat>image/jpeg</scan:DocumentFormat><scan:DocumentFormat>application/pdf</scan:DocumentFormat></scan:DocumentFormats>
<scan:SupportedResolutions><scan:DiscreteResolutions>
<scan:DiscreteResolution><scan:XResolution>100</scan:XResolution><scan:YResolution>100</scan:YResolution></scan:DiscreteResolution>
<scan:DiscreteResolution><scan:XResolution>300</scan:XResolution><scan:YResolution>300</scan:YResolution></scan:DiscreteResolution>
</scan:DiscreteResolutions></scan:SupportedResolutions>
</scan:SettingProfile></scan:SettingProfiles>
</scan:Platen>
<scan:Adf><scan:AdfSimplexInputCaps>
<scan:SettingProfiles><scan:SettingProfile>
<scan:ColorModes><scan:ColorMode>RGB24</scan:ColorMode></scan:ColorModes>
<scan:DocumentFormats><scan:DocumentFormat>image/jpeg</scan:DocumentFormat></scan:DocumentFormats>
<scan:SupportedResolutions><scan:DiscreteResolutions>
<scan:DiscreteResolution><scan:XResolution>300</scan:XResolution><scan:YResolution>300</scan:YResolution></scan:DiscreteResolution>
</scan:DiscreteResolutions></scan:SupportedResolutions>
</scan:SettingProfile></scan:SettingProfiles>
</scan:AdfSimplexInputCaps></scan:Adf>
</scan:ScannerCapabilities>"#;

    fn test_page_bytes(seed: u8) -> Vec<u8> {
        let mut px = vec![0u8; 64 * 48 * 3];
        for (i, v) in px.iter_mut().enumerate() {
            *v = ((i as u8).wrapping_add(seed * 37)) % 200 + 20;
        }
        scancraft_imaging::Image::new(64, 48, px).unwrap().encode_jpeg(75).unwrap()
    }

    /// Minimal scripted server: capabilities, job create, N pages, then 404s.
    struct MockServer {
        base: String,
        _handle: std::thread::JoinHandle<()>,
    }

    fn start_mock(pages: Vec<Vec<u8>>) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let nd_count = Arc::new(AtomicUsize::new(0));
        let nd_clone = nd_count.clone();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming().take(pages.len() + 7) {
                let Ok(mut s) = stream else { break };
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap_or(());
                let mut buf = [0u8; 16384];
                let mut req = Vec::new();
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                    if find_headers_end(&req).is_some() {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&req);
                let first_line = head.lines().next().unwrap_or("").to_owned();
                // Drain a POST body if present.
                if first_line.starts_with("POST")
                    && let Some(cl) = head.lines().find(|l| l.to_lowercase().starts_with("content-length:"))
                {
                    let len: usize = cl.split(':').nth(1).unwrap_or("0").trim().parse().unwrap_or(0);
                    let head_end = find_headers_end(&req).unwrap_or(req.len());
                    let mut have = req.len().saturating_sub(head_end);
                    while have < len {
                        let Ok(n) = s.read(&mut buf) else { break };
                        if n == 0 {
                            break;
                        }
                        have += n;
                    }
                }
                let (status, ctype, body): (&str, &str, Vec<u8>) = if first_line.contains("ScannerCapabilities") {
                    ("200 OK", "text/xml", CAPS_XML.as_bytes().to_vec())
                } else if first_line.starts_with("POST") {
                    ("201 Created", "text/xml", Vec::new())
                } else if first_line.contains("NextDocument") {
                    let i = nd_clone.fetch_add(1, Ordering::SeqCst);
                    if i < pages.len() { ("200 OK", "image/jpeg", pages[i].clone()) } else { ("404 Not Found", "text/plain", Vec::new()) }
                } else if first_line.starts_with("DELETE") {
                    ("200 OK", "text/plain", Vec::new())
                } else {
                    ("404 Not Found", "text/plain", Vec::new())
                };
                let mut resp = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                if first_line.starts_with("POST") {
                    resp.push_str("Location: /eSCL/ScanJobs/job-1\r\n");
                }
                resp.push_str("\r\n");
                let _ = s.write_all(resp.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        MockServer { base: format!("http://127.0.0.1:{port}/eSCL"), _handle: handle }
    }

    fn profile_for(device: &str) -> (ScanRequest, Vec<String>) {
        let mut p = ScanProfile { device_id: device.to_owned(), dpi: 300, ..ScanProfile::default() };
        p.bit_depth = scancraft_model::BitDepth::Color;
        let mut ignored = Vec::new();
        let req = ScanRequest::from_profile(&p, &mut ignored).unwrap();
        (req, ignored)
    }

    struct Collect {
        pages: Vec<RawPage>,
    }

    impl PageSink for Collect {
        fn page(&mut self, page: RawPage) -> Result<(), ScanError> {
            self.pages.push(page);
            Ok(())
        }

        fn progress(&mut self, _p: Progress) {}
    }

    #[test]
    fn discovers_and_scans_two_pages() {
        let server = start_mock(vec![test_page_bytes(1), test_page_bytes(2)]);
        let backend = EsclScanner::with_hosts(vec![server.base.replace("/eSCL", "")]);
        let devices = backend.list_devices(&CancelToken::new()).unwrap();
        assert_eq!(devices.len(), 1);
        assert!(devices[0].display_name.contains("MockPrinter"));
        let caps = backend.capabilities(&devices[0].id).unwrap();
        assert!(caps.sources.contains(&PaperSource::Flatbed));
        assert!(caps.sources.contains(&PaperSource::Feeder));
        assert!(caps.dpis.contains(&300));
        let (req, _) = profile_for(&devices[0].id.0);
        let mut sink = Collect { pages: Vec::new() };
        let n = backend.scan(&req, &mut sink, &CancelToken::new()).unwrap();
        assert_eq!((n, sink.pages.len()), (2, 2));
        assert_eq!(sink.pages[0].dpi, 300);
    }

    #[test]
    fn empty_job_is_success_with_zero_pages() {
        let server = start_mock(vec![]);
        let backend = EsclScanner::with_hosts(vec![server.base.replace("/eSCL", "")]);
        let devices = backend.list_devices(&CancelToken::new()).unwrap();
        let (req, _) = profile_for(&devices[0].id.0);
        let mut sink = Collect { pages: Vec::new() };
        assert_eq!(backend.scan(&req, &mut sink, &CancelToken::new()).unwrap(), 0);
    }

    #[test]
    fn unreachable_host_lists_nothing() {
        let backend = EsclScanner::with_hosts(vec!["http://127.0.0.1:1".to_owned()]);
        assert!(backend.list_devices(&CancelToken::new()).unwrap().is_empty());
    }

    #[test]
    fn settings_xml_has_wire_names() {
        let caps = parse_capabilities(CAPS_XML);
        assert_eq!(caps.make_and_model, "MockPrinter M1");
        assert!(caps.has_platen && caps.has_feeder);
        assert_eq!(caps.max_width_300ths, Some(2550));
        let req = ScanRequest {
            device: DeviceId("escl:x".to_owned()),
            paper_source: PaperSource::Flatbed,
            page_size: PageSize::A4,
            dpi: 300,
            bit_depth: BitDepth::Color,
            brightness: 0,
            contrast: 0,
        };
        let xml = build_scan_settings(&caps, &req, 2550, 3508);
        for needle in [
            "scan:ScanSettings",
            "pwg:Version",
            "scan:Intent",
            "pwg:ScanRegions",
            "escl:ThreeHundredthsOfInches",
            "pwg:InputSource",
            "Platen",
            "scan:XResolution>300",
            "scan:ColorMode>RGB24",
            "scan:DocumentFormat>image/jpeg",
        ] {
            assert!(xml.contains(needle), "missing {needle} in:\n{xml}");
        }
    }
}
