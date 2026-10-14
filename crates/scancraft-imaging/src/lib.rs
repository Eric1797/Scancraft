//! Image pipeline: decode/encode plus the edit-stack operations.
//! All buffers are RGB8; sizes are capped so a hostile file cannot OOM the
//! app. Downscaled proxies drive analysis (deskew/crop/blank); full pixels
//! are touched once per committed edit and once at export.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;

use scancraft_model::EditOp;

/// Hard caps (a 600 dpi A4 page is ~100 MB raw; analysis never needs it).
pub const MAX_DIM: u32 = 20_000;
pub const MAX_BYTES: usize = 512 * 1024 * 1024;

/// RGB8 image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

/// Axis-aligned rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImgError {
    Empty,
    TooLarge,
    BadLength,
    OutOfBounds,
    Decode(String),
    Encode(String),
    Unsupported(&'static str),
}

impl fmt::Display for ImgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImgError::Empty => write!(f, "image has zero width or height"),
            ImgError::TooLarge => write!(f, "image exceeds size caps"),
            ImgError::BadLength => write!(f, "pixel buffer length does not match dimensions"),
            ImgError::OutOfBounds => write!(f, "crop rectangle is out of bounds"),
            ImgError::Decode(msg) => write!(f, "could not decode image: {msg}"),
            ImgError::Encode(msg) => write!(f, "could not encode image: {msg}"),
            ImgError::Unsupported(what) => write!(f, "unsupported image: {what}"),
        }
    }
}

impl std::error::Error for ImgError {}

fn check_dims(w: u32, h: u32) -> Result<(), ImgError> {
    if w == 0 || h == 0 {
        return Err(ImgError::Empty);
    }
    if w > MAX_DIM || h > MAX_DIM {
        return Err(ImgError::TooLarge);
    }
    let bytes = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(3)).ok_or(ImgError::TooLarge)?;
    if bytes > MAX_BYTES {
        return Err(ImgError::TooLarge);
    }
    Ok(())
}

impl Image {
    pub fn new(w: u32, h: u32, px: Vec<u8>) -> Result<Self, ImgError> {
        check_dims(w, h)?;
        if px.len() != (w as usize) * (h as usize) * 3 {
            return Err(ImgError::BadLength);
        }
        Ok(Self { w, h, px })
    }

    pub fn filled(w: u32, h: u32, rgb: [u8; 3]) -> Result<Self, ImgError> {
        check_dims(w, h)?;
        let mut px = vec![0u8; (w as usize) * (h as usize) * 3];
        for chunk in px.chunks_mut(3) {
            chunk.copy_from_slice(&rgb);
        }
        Ok(Self { w, h, px })
    }

    pub fn dims(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    pub fn pixels(&self) -> &[u8] {
        &self.px
    }

    pub fn into_pixels(self) -> Vec<u8> {
        self.px
    }

    fn at(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.w || y >= self.h {
            return None;
        }
        let i = ((y as usize) * (self.w as usize) + (x as usize)) * 3;
        let b = self.px.get(i..i + 3)?;
        Some([b[0], b[1], b[2]])
    }

    /// Decode any supported format to RGB8 (drops alpha).
    pub fn decode(bytes: &[u8]) -> Result<Self, ImgError> {
        if bytes.len() > MAX_BYTES.saturating_add(1024) {
            return Err(ImgError::TooLarge);
        }
        let dynimg = image::load_from_memory(bytes).map_err(|e| ImgError::Decode(trim_err(e)))?;
        let rgb = dynimg.to_rgb8();
        let (w, h) = rgb.dimensions();
        Self::new(w, h, rgb.into_raw())
    }

    /// Encode as PNG (lossless; session cache + export).
    pub fn encode_png(&self) -> Result<Vec<u8>, ImgError> {
        let mut out = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut out);
        image::ImageEncoder::write_image(encoder, &self.px, self.w, self.h, image::ExtendedColorType::Rgb8)
            .map_err(|e| ImgError::Encode(trim_err(e)))?;
        Ok(out)
    }

    /// Encode as JPEG with quality 0–100.
    pub fn encode_jpeg(&self, quality: u8) -> Result<Vec<u8>, ImgError> {
        let mut out = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
        image::ImageEncoder::write_image(encoder, &self.px, self.w, self.h, image::ExtendedColorType::Rgb8)
            .map_err(|e| ImgError::Encode(trim_err(e)))?;
        Ok(out)
    }

    /// Exact 90° clockwise rotation.
    pub fn rotate90_cw(&self) -> Self {
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = vec![0u8; self.px.len()];
        for y in 0..h {
            for x in 0..w {
                let src = (y * w + x) * 3;
                let dst = (x * h + (h - 1 - y)) * 3;
                if let (Some(s), Some(d)) = (self.px.get(src..src + 3), out.get_mut(dst..dst + 3)) {
                    d.copy_from_slice(s);
                }
            }
        }
        Self { w: self.h, h: self.w, px: out }
    }

    pub fn rotate90_ccw(&self) -> Self {
        self.rotate90_cw().rotate90_cw().rotate90_cw()
    }

    pub fn flip_h(&self) -> Self {
        let mut out = self.clone();
        let w = self.w as usize;
        for row in out.px.chunks_exact_mut(w * 3) {
            let mut i = 0;
            let mut j = w - 1;
            while i < j {
                for c in 0..3 {
                    row.swap(i * 3 + c, j * 3 + c);
                }
                i += 1;
                j -= 1;
            }
        }
        out
    }

    pub fn flip_v(&self) -> Self {
        let mut out = self.clone();
        let stride = (self.w as usize) * 3;
        let h = self.h as usize;
        for y in 0..h / 2 {
            for x in 0..stride {
                out.px.swap(y * stride + x, (h - 1 - y) * stride + x);
            }
        }
        out
    }

    /// Crop to `rect` (clamped check: must fit fully inside).
    pub fn crop(&self, rect: Rect) -> Result<Self, ImgError> {
        let (x2, y2) = (rect.x.checked_add(rect.w).ok_or(ImgError::OutOfBounds)?, rect.y.checked_add(rect.h).ok_or(ImgError::OutOfBounds)?);
        if rect.w == 0 || rect.h == 0 || x2 > self.w || y2 > self.h {
            return Err(ImgError::OutOfBounds);
        }
        check_dims(rect.w, rect.h)?;
        let mut px = vec![0u8; (rect.w as usize) * (rect.h as usize) * 3];
        for y in 0..rect.h as usize {
            let src = (((rect.y as usize) + y) * (self.w as usize) + (rect.x as usize)) * 3;
            let dst = y * (rect.w as usize) * 3;
            if let (Some(s), Some(d)) = (self.px.get(src..src + (rect.w as usize) * 3), px.get_mut(dst..dst + (rect.w as usize) * 3)) {
                d.copy_from_slice(s);
            }
        }
        Ok(Self { w: rect.w, h: rect.h, px })
    }

    /// Bilinear resize (also backs thumbnails).
    pub fn resize(&self, dst_w: u32, dst_h: u32) -> Result<Self, ImgError> {
        check_dims(dst_w, dst_h)?;
        if self.w == dst_w && self.h == dst_h {
            return Ok(self.clone());
        }
        let (sw, sh, dw, dh) = (self.w as f64, self.h as f64, dst_w as usize, dst_h as usize);
        let mut px = vec![0u8; dw * dh * 3];
        for y in 0..dh {
            for x in 0..dw {
                let sx = (x as f64 + 0.5) * sw / (dst_w as f64) - 0.5;
                let sy = (y as f64 + 0.5) * sh / (dst_h as f64) - 0.5;
                let c = bilinear(self, sx, sy);
                let o = (y * dw + x) * 3;
                if let Some(d) = px.get_mut(o..o + 3) {
                    d.copy_from_slice(&c);
                }
            }
        }
        Ok(Self { w: dst_w, h: dst_h, px })
    }

    /// Thumbnail with longest side `max_side` (aspect preserved, never upscale).
    pub fn thumbnail(&self, max_side: u32) -> Result<Self, ImgError> {
        let side = self.w.max(self.h);
        if side <= max_side.max(1) {
            return Ok(self.clone());
        }
        let scale = (max_side.max(1) as f64) / (side as f64);
        let dw = ((self.w as f64) * scale).round().clamp(1.0, MAX_DIM as f64) as u32;
        let dh = ((self.h as f64) * scale).round().clamp(1.0, MAX_DIM as f64) as u32;
        self.resize(dw.max(1), dh.max(1))
    }

    /// Brightness/contrast in NAPS2 units (-1000..=1000 each).
    pub fn brightness_contrast(&self, brightness: i32, contrast: i32) -> Self {
        let b = (brightness.clamp(-1000, 1000) as f32) / 1000.0 * 64.0;
        let c = 1.0 + (contrast.clamp(-1000, 1000) as f32) / 1000.0 * 1.5;
        let mut out = self.px.clone();
        for v in out.iter_mut() {
            let adj = ((*v as f32) - 128.0) * c + 128.0 + b;
            *v = adj.round().clamp(0.0, 255.0) as u8;
        }
        Self { w: self.w, h: self.h, px: out }
    }

    /// Luminosity grayscale (kept RGB8 so the pipeline stays one format).
    pub fn grayscale(&self) -> Self {
        let mut out = Vec::with_capacity(self.px.len());
        for rgb in self.px.chunks(3) {
            let y = (0.2126 * f64::from(rgb[0]) + 0.7152 * f64::from(rgb[1]) + 0.0722 * f64::from(rgb[2])).round().clamp(0.0, 255.0) as u8;
            out.extend_from_slice(&[y, y, y]);
        }
        Self { w: self.w, h: self.h, px: out }
    }

    /// Global threshold to black & white.
    pub fn threshold(&self, t: u8) -> Self {
        let gray = self.grayscale();
        let mut out = Vec::with_capacity(gray.px.len());
        for rgb in gray.px.chunks(3) {
            let v = if rgb[0] >= t { 255u8 } else { 0u8 };
            out.extend_from_slice(&[v, v, v]);
        }
        Self { w: gray.w, h: gray.h, px: out }
    }

    /// Mild unsharp-mask sharpen (3x3, fixed strength).
    pub fn sharpen(&self) -> Self {
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = self.px.clone();
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                for c in 0..3 {
                    let center = self.px[(y * w + x) * 3 + c] as i32;
                    let mut blur = 0i32;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let nx = (x as i32 + dx) as usize;
                            let ny = (y as i32 + dy) as usize;
                            blur += self.px[(ny * w + nx) * 3 + c] as i32;
                        }
                    }
                    blur /= 9;
                    let sharp = center + ((center - blur) / 2);
                    if let Some(slot) = out.get_mut((y * w + x) * 3 + c) {
                        *slot = sharp.clamp(0, 255) as u8;
                    }
                }
            }
        }
        Self { w: self.w, h: self.h, px: out }
    }

    /// Arbitrary clockwise rotation (bilinear, paper-white fill).
    pub fn rotate_arbitrary(&self, degrees_cw: f64) -> Result<Self, ImgError> {
        if !degrees_cw.is_finite() {
            return Err(ImgError::Unsupported("rotation must be finite"));
        }
        let norm = ((degrees_cw % 360.0) + 360.0) % 360.0;
        let quarter = norm / 90.0;
        if (quarter - quarter.round()).abs() < 1e-9 {
            let mut img = self.clone();
            for _ in 0..(quarter.round() as usize % 4) {
                img = img.rotate90_cw();
            }
            return Ok(img);
        }
        let rad = norm.to_radians();
        let (sin, cos) = (rad.sin(), rad.cos());
        let (sw, sh) = (self.w as f64, self.h as f64);
        let dw = (sw * cos.abs() + sh * sin.abs()).round().clamp(1.0, MAX_DIM as f64) as u32;
        let dh = (sw * sin.abs() + sh * cos.abs()).round().clamp(1.0, MAX_DIM as f64) as u32;
        check_dims(dw, dh)?;
        let (cx, cy, dx, dy) = (sw / 2.0 - 0.5, sh / 2.0 - 0.5, dw as f64 / 2.0 - 0.5, dh as f64 / 2.0 - 0.5);
        let mut px = vec![255u8; (dw as usize) * (dh as usize) * 3];
        for y in 0..dh as usize {
            for x in 0..dw as usize {
                // Inverse-map destination → source (clockwise rotation).
                let sx = cos * (x as f64 - dx) - sin * (y as f64 - dy) + cx;
                let sy = sin * (x as f64 - dx) + cos * (y as f64 - dy) + cy;
                let c = bilinear(self, sx, sy);
                if let Some(d) = px.get_mut((y * (dw as usize) + x) * 3..(y * (dw as usize) + x) * 3 + 3) {
                    d.copy_from_slice(&c);
                }
            }
        }
        Ok(Self { w: dw, h: dh, px })
    }

    /// Downscaled grayscale luma for analysis.
    fn probe_gray(&self, max_side: u32) -> Vec<u8> {
        let side = self.w.max(self.h).max(1);
        let scale = ((max_side as f64) / (side as f64)).min(1.0);
        let dw = ((self.w as f64) * scale).round().clamp(1.0, 4096.0) as usize;
        let dh = ((self.h as f64) * scale).round().clamp(1.0, 4096.0) as usize;
        let mut out = vec![0u8; dw * dh];
        for y in 0..dh {
            for x in 0..dw {
                let sx = ((x as f64 + 0.5) / (dw as f64) * (self.w as f64) - 0.5).round() as i64;
                let sy = ((y as f64 + 0.5) / (dh as f64) * (self.h as f64) - 0.5).round() as i64;
                let sx = sx.clamp(0, self.w as i64 - 1) as usize;
                let sy = sy.clamp(0, self.h as i64 - 1) as usize;
                if let Some(rgb) = self.px.get((sy * (self.w as usize) + sx) * 3..(sy * (self.w as usize) + sx) * 3 + 3) {
                    out[y * dw + x] = (0.2126 * f64::from(rgb[0]) + 0.7152 * f64::from(rgb[1]) + 0.0722 * f64::from(rgb[2])) as u8;
                }
            }
        }
        out
    }

    /// Fraction of pixels darker than `white_threshold` (0.0–1.0).
    /// NAPS2 defaults: white 70 (inverted scale), coverage 15 — here the
    /// threshold is a plain 0–255 luma and coverage a 0.0–1.0 fraction.
    pub fn ink_coverage(&self, white_threshold: u8) -> f32 {
        let gray = self.probe_gray(256);
        if gray.is_empty() {
            return 0.0;
        }
        let dark = gray.iter().filter(|v| **v < white_threshold).count();
        (dark as f32) / (gray.len() as f32)
    }

    /// True when the page counts as blank under both thresholds.
    pub fn is_blank(&self, white_threshold: u8, max_coverage: f32) -> bool {
        self.ink_coverage(white_threshold) <= max_coverage
    }

    /// Bounding box of non-background (luma < `bg_threshold`), expanded by
    /// `margin` px and clamped. `None` when the page is (near-)blank.
    pub fn content_box(&self, bg_threshold: u8, margin: u32) -> Option<Rect> {
        let dw = 256.min(self.w) as usize;
        let dh = ((self.h as f64) * (dw as f64) / (self.w as f64)).round().clamp(1.0, 4096.0) as usize;
        let (mut x0, mut y0, mut x1, mut y1) = (dw, dh, 0usize, 0usize);
        for y in 0..dh {
            for x in 0..dw {
                let sx = (x * (self.w as usize) / dw) as u32;
                let sy = (y * (self.h as usize) / dh) as u32;
                let dark = self.at(sx, sy).map(|rgb| rgb[0].min(rgb[1]).min(rgb[2]) < bg_threshold).unwrap_or(false);
                if dark {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if x1 < x0 || y1 < y0 {
            return None;
        }
        // Scale back to full resolution.
        let sx = (self.w as f64) / (dw as f64);
        let sy = (self.h as f64) / (dh as f64);
        let m = margin;
        let rx = ((x0 as f64) * sx).round() as u32;
        let ry = ((y0 as f64) * sy).round() as u32;
        let rw = (((x1 - x0 + 1) as f64) * sx).round() as u32;
        let rh = (((y1 - y0 + 1) as f64) * sy).round() as u32;
        let x = rx.saturating_sub(m);
        let y = ry.saturating_sub(m);
        let w = (rw + m * 2).min(self.w.saturating_sub(x));
        let h = (rh + m * 2).min(self.h.saturating_sub(y));
        if w == 0 || h == 0 { None } else { Some(Rect { x, y, w, h }) }
    }

    /// Auto-crop to content (background = near-white). `None` when blank or
    /// when the box already covers ~everything (nothing to do).
    pub fn auto_crop(&self, bg_threshold: u8, margin: u32) -> Option<Rect> {
        let b = self.content_box(bg_threshold, margin)?;
        let area_full = (self.w as u64) * (self.h as u64);
        let area_box = (b.w as u64) * (b.h as u64);
        if area_box * 100 >= area_full * 98 { None } else { Some(b) }
    }

    /// Render a non-destructive [`EditOp`] stack onto a copy. `Reset`
    /// restarts from the base image (drops earlier ops). Used at export and
    /// for full-size previews; thumbnails use the same path on proxies.
    pub fn apply_edits(&self, edits: &[EditOp]) -> Result<Self, ImgError> {
        let mut img = self.clone();
        for edit in edits {
            img = match edit {
                EditOp::RotateCw => img.rotate90_cw(),
                EditOp::RotateCcw => img.rotate90_ccw(),
                EditOp::FlipHorizontal => img.flip_h(),
                EditOp::FlipVertical => img.flip_v(),
                EditOp::RotateDegrees(deg) => img.rotate_arbitrary(*deg)?,
                EditOp::Crop { x, y, w, h } => img.crop(Rect { x: *x, y: *y, w: *w, h: *h })?,
                EditOp::BrightnessContrast { brightness, contrast } => img.brightness_contrast(*brightness, *contrast),
                EditOp::Reset => self.clone(),
            };
        }
        Ok(img)
    }
}

/// Skew estimate in degrees clockwise (−10..=10) via projection profiles on a
/// downscaled grayscale: the angle with the most near-pure rows (fully dark
/// or fully blank) wins. Ties prefer the smaller angle.
pub fn detect_skew(image: &Image) -> f32 {
    let gray_full = image.probe_gray(300);
    let side_w = 300.min(image.w) as usize;
    let side_h = ((image.h as f64) * (side_w as f64) / (image.w as f64)).round().clamp(1.0, 4096.0) as usize;
    if gray_full.len() != side_w * side_h || side_w == 0 || side_h == 0 {
        return 0.0;
    }
    let dark: Vec<u8> = gray_full.iter().map(|v| u8::from(*v < 128)).collect();
    let mut best_angle = 0.0f32;
    let mut best_score = f32::MIN;
    // Outward from 0 so ties settle on the smaller angle.
    let mut k = 0u32;
    while k <= 20 {
        for angle in if k == 0 { vec![0.0f32] } else { vec![-(k as f32) * 0.5, (k as f32) * 0.5] } {
            let score = profile_score(&dark, side_w, side_h, angle);
            if score > best_score {
                best_score = score;
                best_angle = angle;
            }
        }
        k += 1;
    }
    // Quantize noise to zero: tiny estimates mean "straight".
    if best_angle.abs() < 0.75 { 0.0 } else { best_angle }
}

/// Shear-sampled row purity for one candidate angle: fraction of rows that
/// are (near-)fully dark or fully blank. A clockwise tilt is undone by
/// sampling each row along the matching diagonal (vertical shear), so the
/// best angle is the correction directly. Aligned text maximizes this;
/// diagonals mix every row and score low.
fn profile_score(dark: &[u8], w: usize, h: usize, angle_deg: f32) -> f32 {
    let shear = angle_deg.to_radians().tan();
    let mut pure = 0u32;
    for y in 0..h {
        let mut sum = 0i32;
        let mut n = 0i32;
        for x in 0..w {
            let sy = ((y as f32) + shear * ((x as f32) - (w as f32) / 2.0)).round() as isize;
            if sy >= 0 && (sy as usize) < h {
                sum += i32::from(dark[(sy as usize) * w + x]);
                n += 1;
            }
        }
        if n > 0 {
            let mean = (sum as f32) / (n as f32);
            // Row is pure when nearly all blank or all dark; rows clipped
            // below half their samples are ignored (edge shear artifacts).
            if (n as f32) >= (w as f32) * 0.5 && (mean <= 0.02 || mean >= 0.98) {
                pure += 1;
            }
        }
    }
    (pure as f32) / (h as f32)
}

fn bilinear(img: &Image, sx: f64, sy: f64) -> [u8; 3] {
    let (w, h) = (img.w as i64, img.h as i64);
    let x0 = sx.floor() as i64;
    let y0 = sy.floor() as i64;
    let (fx, fy) = ((sx - x0 as f64).clamp(0.0, 1.0), (sy - y0 as f64).clamp(0.0, 1.0));
    let sample = |x: i64, y: i64| -> [f64; 3] {
        let x = x.clamp(0, w - 1) as usize;
        let y = y.clamp(0, h - 1) as usize;
        match img.px.get((y * (w as usize) + x) * 3..(y * (w as usize) + x) * 3 + 3) {
            Some(b) => [f64::from(b[0]), f64::from(b[1]), f64::from(b[2])],
            None => [0.0, 0.0, 0.0],
        }
    };
    let (a, b, c, d) = (sample(x0, y0), sample(x0 + 1, y0), sample(x0, y0 + 1), sample(x0 + 1, y0 + 1));
    let mut out = [0u8; 3];
    for i in 0..3 {
        let v = a[i] * (1.0 - fx) * (1.0 - fy) + b[i] * fx * (1.0 - fy) + c[i] * (1.0 - fx) * fy + d[i] * fx * fy;
        out[i] = v.round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn trim_err<E: fmt::Display>(e: E) -> String {
    let s = e.to_string();
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn striped(w: u32, h: u32) -> Image {
        let mut px = vec![0u8; (w as usize) * (h as usize) * 3];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let v = if (x + y) % 2 == 0 { 20u8 } else { 230u8 };
                let o = (y * (w as usize) + x) * 3;
                px[o..o + 3].copy_from_slice(&[v, v, v]);
            }
        }
        Image::new(w, h, px).unwrap()
    }

    fn text_page(w: u32, h: u32) -> Image {
        // Dark horizontal "text lines" on white with uniform margins, so a
        // straight page profiles flat and only rotation introduces skew.
        let mut img = Image::filled(w, h, [255, 255, 255]).unwrap();
        let wu = w as usize;
        for y in (20..h.saturating_sub(20)).step_by(14) {
            for y2 in y..(y + 5).min(h) {
                for x in 20..wu.saturating_sub(20).min(20 + wu.saturating_sub(60)) {
                    let o = ((y2 as usize) * wu + x) * 3;
                    img.px[o..o + 3].copy_from_slice(&[10, 10, 10]);
                }
            }
        }
        img
    }

    #[test]
    fn rotate_four_times_is_identity() {
        let img = striped(37, 23);
        let back = img.rotate90_cw().rotate90_cw().rotate90_cw().rotate90_cw();
        assert_eq!(back, img);
    }

    #[test]
    fn edit_stack_roundtrip() {
        use scancraft_model::EditOp;
        let img = striped(37, 23);
        let back = img.apply_edits(&[EditOp::RotateCw, EditOp::RotateCcw, EditOp::FlipHorizontal, EditOp::FlipHorizontal]).unwrap();
        assert_eq!(back, img);
        let cropped = img.apply_edits(&[EditOp::Crop { x: 1, y: 1, w: 10, h: 10 }]).unwrap();
        assert_eq!(cropped.dims(), (10, 10));
        assert!(img.apply_edits(&[EditOp::Crop { x: 30, y: 30, w: 20, h: 20 }]).is_err());
    }

    #[test]
    fn crop_rejects_out_of_bounds() {
        let img = striped(40, 40);
        assert!(img.crop(Rect { x: 30, y: 30, w: 20, h: 20 }).is_err());
        let c = img.crop(Rect { x: 5, y: 5, w: 10, h: 10 }).unwrap();
        assert_eq!(c.dims(), (10, 10));
    }

    #[test]
    fn thresholds_are_monotonic() {
        let img = striped(16, 16);
        let lo = img.threshold(10).px_count_dark();
        let hi = img.threshold(240).px_count_dark();
        assert!(lo <= hi);
    }

    #[test]
    fn png_roundtrip() {
        let img = striped(32, 24);
        let bytes = img.encode_png().unwrap();
        let back = Image::decode(&bytes).unwrap();
        assert_eq!(back, img);
    }

    #[test]
    fn jpeg_decodes() {
        let img = striped(32, 24);
        let bytes = img.encode_jpeg(80).unwrap();
        let back = Image::decode(&bytes).unwrap();
        assert_eq!(back.dims(), (32, 24));
    }

    #[test]
    fn blank_detection() {
        let blank = Image::filled(100, 100, [250, 250, 250]).unwrap();
        assert!(blank.is_blank(240, 0.02));
        let text = text_page(200, 200);
        assert!(!text.is_blank(240, 0.02));
    }

    #[test]
    fn deskew_finds_known_angle() {
        let straight = text_page(240, 200);
        assert!(detect_skew(&straight).abs() <= 1.0);
        let skewed = straight.rotate_arbitrary(4.0).unwrap();
        let found = detect_skew(&skewed);
        assert!((found.abs() - 4.0).abs() <= 1.5, "found {found}");
        // The detector returns either the skew or its correction; the sign
        // that straightens the page is the right correction.
        let a = skewed.rotate_arbitrary(-(found as f64)).unwrap();
        let b = skewed.rotate_arbitrary(found as f64).unwrap();
        let fixed = if detect_skew(&a).abs() <= detect_skew(&b).abs() { a } else { b };
        assert!(detect_skew(&fixed).abs() <= 1.5);
    }

    #[test]
    fn autocrop_trims_white_border() {
        let mut img = Image::filled(200, 200, [255, 255, 255]).unwrap();
        for y in 60..140 {
            for x in 60..140 {
                let o = (y * 200 + x) * 3;
                img.px[o..o + 3].copy_from_slice(&[10, 10, 10]);
            }
        }
        let b = img.auto_crop(240, 4).unwrap();
        assert!(b.x <= 60 && b.y <= 60 && b.x + b.w >= 140 && b.y + b.h >= 140);
        assert!(b.w < 200 && b.h < 200);
    }

    #[test]
    fn oversized_rejected() {
        assert_eq!(Image::new(0, 10, vec![]), Err(ImgError::Empty));
        assert_eq!(Image::new(30_000, 10, vec![0u8; 100]), Err(ImgError::TooLarge));
    }

    trait DarkCount {
        fn px_count_dark(&self) -> usize;
    }

    impl DarkCount for Image {
        fn px_count_dark(&self) -> usize {
            self.px.chunks(3).filter(|c| c[0] < 128).count()
        }
    }
}
