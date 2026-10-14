// Copyright (C) 2026 ScanCraft contributors (GPL-2.0-or-later).
// TWAIN backend skeleton. Full driver + 32-bit worker arrive in M7.
// Structure ported from NAPS2 NAPS2.Sdk/Scan (GPL-2.0-or-later).

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use scancraft_scan_core::{CancelToken, Capabilities, DeviceId, DeviceInfo, PageSink, ScanError, ScanRequest, ScannerBackend};

/// TWAIN driver options (ported field set from NAPS2 `TwainOptions`).
#[derive(Debug, Clone, Default)]
pub struct TwainOptions {
    /// Show the vendor's native dialog instead of silent transfer.
    pub use_native_ui: bool,
    /// Parent window handle for the native dialog (0 = none).
    pub dialog_parent: usize,
}

/// TWAIN backend (M7). Always reports itself unavailable until the
/// 32-bit worker + IPC channel land.
#[derive(Debug, Clone, Default)]
pub struct TwainScanner;

impl ScannerBackend for TwainScanner {
    fn id(&self) -> &'static str {
        "twain"
    }

    fn list_devices(&self, _cancel: &CancelToken) -> Result<Vec<DeviceInfo>, ScanError> {
        Ok(Vec::new())
    }

    fn capabilities(&self, _device: &DeviceId) -> Result<Capabilities, ScanError> {
        Err(ScanError::Unsupported("TWAIN arrives in M7 (32-bit worker)".to_owned()))
    }

    fn scan(&self, _request: &ScanRequest, _sink: &mut dyn PageSink, _cancel: &CancelToken) -> Result<u32, ScanError> {
        Err(ScanError::Unsupported("TWAIN arrives in M7 (32-bit worker)".to_owned()))
    }
}
