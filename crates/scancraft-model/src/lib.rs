//! Pure data model for ScanCraft. No I/O, no OS calls, no UI.
//!
//! Vocabulary mirrors NAPS2's option names (facts, clean-room) so profiles
//! and docs translate 1:1. All input-derived values validate via `Result`.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;

/// Bit depth of a scanned or imported page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitDepth {
    Color,
    Grayscale,
    BlackWhite,
}

/// Where the sheet comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperSource {
    Flatbed,
    Feeder,
    Duplex,
}

/// Front or back side of a duplex sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSide {
    Front,
    Back,
}

/// Standard page sizes plus custom (millimetres).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageSize {
    A4,
    Letter,
    Legal,
    A5,
    A3,
    B4,
    B5,
    Custom { width_mm: f64, height_mm: f64 },
}

impl PageSize {
    /// Dimensions in millimetres. Returns an error for non-positive custom sizes.
    pub fn dims_mm(self) -> Result<(f64, f64), ModelError> {
        match self {
            PageSize::A4 => Ok((210.0, 297.0)),
            PageSize::Letter => Ok((215.9, 279.4)),
            PageSize::Legal => Ok((215.9, 355.6)),
            PageSize::A5 => Ok((148.0, 210.0)),
            PageSize::A3 => Ok((297.0, 420.0)),
            PageSize::B4 => Ok((250.0, 353.0)),
            PageSize::B5 => Ok((176.0, 250.0)),
            PageSize::Custom { width_mm, height_mm } => {
                if !width_mm.is_finite() || !height_mm.is_finite() || width_mm <= 0.0 || height_mm <= 0.0 {
                    return Err(ModelError::InvalidPageSize);
                }
                if width_mm > 2000.0 || height_mm > 2000.0 {
                    return Err(ModelError::InvalidPageSize);
                }
                Ok((width_mm, height_mm))
            }
        }
    }
}

/// One non-destructive edit step stored per page (rendered on demand, baked at export).
#[derive(Debug, Clone, PartialEq)]
pub enum EditOp {
    RotateCw,
    RotateCcw,
    FlipHorizontal,
    FlipVertical,
    /// Fixed clockwise rotation in degrees (applied after deskew, like NAPS2).
    RotateDegrees(f64),
    Crop {
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
    BrightnessContrast {
        brightness: i32,
        contrast: i32,
    },
    Reset,
}

/// A single page in a document.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub id: u64,
    pub width_px: u32,
    pub height_px: u32,
    pub dpi: u32,
    pub bit_depth: BitDepth,
    pub side: PageSide,
    /// Storage key into the session cache (files, never inline bitmaps).
    pub storage_key: String,
    pub edits: Vec<EditOp>,
}

impl Page {
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.width_px == 0 || self.height_px == 0 {
            return Err(ModelError::InvalidDimensions);
        }
        if self.dpi == 0 || self.dpi > 2400 {
            return Err(ModelError::InvalidDpi(self.dpi));
        }
        if self.storage_key.is_empty() {
            return Err(ModelError::InvalidStorageKey);
        }
        Ok(())
    }
}

/// A named scan profile. Field vocabulary follows NAPS2 `ScanOptions`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanProfile {
    pub name: String,
    pub driver: String,
    pub device_id: String,
    pub prompt_for_device: bool,
    pub paper_source: PaperSource,
    pub page_size: PageSize,
    pub dpi: u32,
    pub bit_depth: BitDepth,
    /// Shrink factor 1, 2, 4 or 8.
    pub scale_ratio: u32,
    pub brightness: i32,
    pub contrast: i32,
    pub auto_deskew: bool,
    pub auto_crop: bool,
    pub exclude_blank_pages: bool,
    pub blank_white_threshold: u8,
    pub blank_coverage_threshold: u8,
    pub flip_duplexed_pages: bool,
    pub rotate_degrees: f64,
    pub max_quality: bool,
    pub jpeg_quality: u8,
}

impl Default for ScanProfile {
    fn default() -> Self {
        Self {
            name: "Default".to_owned(),
            driver: "wia".to_owned(),
            device_id: String::new(),
            prompt_for_device: false,
            paper_source: PaperSource::Flatbed,
            page_size: PageSize::A4,
            dpi: 300,
            bit_depth: BitDepth::Color,
            scale_ratio: 1,
            brightness: 0,
            contrast: 0,
            auto_deskew: false,
            auto_crop: false,
            exclude_blank_pages: false,
            blank_white_threshold: 70,
            blank_coverage_threshold: 15,
            flip_duplexed_pages: false,
            rotate_degrees: 0.0,
            max_quality: false,
            jpeg_quality: 75,
        }
    }
}

impl ScanProfile {
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.name.trim().is_empty() {
            return Err(ModelError::InvalidProfile("empty name".to_owned()));
        }
        if !(50..=1200).contains(&self.dpi) {
            return Err(ModelError::InvalidDpi(self.dpi));
        }
        if !matches!(self.scale_ratio, 1 | 2 | 4 | 8) {
            return Err(ModelError::InvalidProfile("scale_ratio must be 1, 2, 4 or 8".to_owned()));
        }
        if !(-1000..=1000).contains(&self.brightness) || !(-1000..=1000).contains(&self.contrast) {
            return Err(ModelError::InvalidProfile("brightness/contrast must be -1000..=1000".to_owned()));
        }
        if self.jpeg_quality > 100 {
            return Err(ModelError::InvalidProfile("jpeg_quality must be 0..=100".to_owned()));
        }
        if !self.rotate_degrees.is_finite() {
            return Err(ModelError::InvalidProfile("rotate_degrees must be finite".to_owned()));
        }
        self.page_size.dims_mm().map(|_| ())
    }
}

/// An open scan document: ordered pages plus a title.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Project {
    pub title: String,
    pub pages: Vec<Page>,
}

/// App settings (persisted; versioned + migrated in M4).
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub theme: Theme,
    pub language: String,
    pub thumbnail_size: u32,
    pub default_save_folder: String,
    pub default_filename_pattern: String,
    pub confirm_on_delete: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            language: "auto".to_owned(),
            thumbnail_size: 160,
            default_save_folder: String::new(),
            default_filename_pattern: "$(YYYY)-$(MM)-$(DD) $(n)".to_owned(),
            confirm_on_delete: true,
        }
    }
}

/// Light/dark/system theme preference (mirrors PdfCraft's ThemePreference).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// Closed error type for model validation. No panics, no I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    InvalidDimensions,
    InvalidDpi(u32),
    InvalidPageSize,
    InvalidStorageKey,
    InvalidProfile(String),
    PageNotFound(u64),
    EmptyDocument,
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::InvalidDimensions => write!(f, "page dimensions must be non-zero"),
            ModelError::InvalidDpi(dpi) => write!(f, "unsupported DPI: {dpi}"),
            ModelError::InvalidPageSize => write!(f, "invalid page size"),
            ModelError::InvalidStorageKey => write!(f, "page storage key must not be empty"),
            ModelError::InvalidProfile(msg) => write!(f, "invalid scan profile: {msg}"),
            ModelError::PageNotFound(id) => write!(f, "page not found: {id}"),
            ModelError::EmptyDocument => write!(f, "document has no pages"),
        }
    }
}

impl std::error::Error for ModelError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_validates() {
        assert!(ScanProfile::default().validate().is_ok());
    }

    #[test]
    fn bad_dpi_rejected() {
        let p = ScanProfile { dpi: 0, ..ScanProfile::default() };
        assert!(matches!(p.validate(), Err(ModelError::InvalidDpi(0))));
        let p = ScanProfile { dpi: 5000, ..ScanProfile::default() };
        assert!(p.validate().is_err());
    }

    #[test]
    fn page_size_dims() {
        assert_eq!(PageSize::A4.dims_mm().unwrap(), (210.0, 297.0));
        assert!(PageSize::Custom { width_mm: 0.0, height_mm: 10.0 }.dims_mm().is_err());
        assert!(PageSize::Custom { width_mm: 100.0, height_mm: 100.0 }.dims_mm().is_ok());
    }

    #[test]
    fn page_validation() {
        let good = Page {
            id: 1,
            width_px: 2480,
            height_px: 3508,
            dpi: 300,
            bit_depth: BitDepth::Color,
            side: PageSide::Front,
            storage_key: "p0001".to_owned(),
            edits: Vec::new(),
        };
        assert!(good.validate().is_ok());
        let mut bad = good.clone();
        bad.width_px = 0;
        assert_eq!(bad.validate(), Err(ModelError::InvalidDimensions));
    }
}
