//! Stable command catalogue: every action the UI, CLI and palette can run.
//! Ids are stable (automation + palette matching depend on them).

/// A runnable command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    /// Stable id, e.g. `"scan"`.
    pub id: &'static str,
    /// English label shown in menus and the palette.
    pub label: &'static str,
    /// Where it lives + shortcut, e.g. `"File · Ctrl+S"`.
    pub detail: &'static str,
}

macro_rules! commands {
    ($(($id:literal, $label:literal, $detail:literal)),* $(,)?) => {
        /// All commands, in palette order.
        pub const COMMANDS: &[Command] = &[
            $(Command { id: $id, label: $label, detail: $detail }),*
        ];
    };
}

commands![
    ("scan", "Scan", "Scan · F5"),
    ("undo", "Undo", "Edit · Ctrl+Z"),
    ("redo", "Redo", "Edit · Ctrl+Y"),
    ("new-doc", "New document", "File · Ctrl+N"),
    ("close-doc", "Close document", "File · Ctrl+W"),
    ("next-tab", "Next tab", "View · Ctrl+Tab"),
    ("prev-tab", "Previous tab", "View · Ctrl+Shift+Tab"),
    ("toggle-view", "Toggle grid / single view", "View · F6"),
    ("select-all", "Select all pages", "Edit · Ctrl+A"),
    ("clear-selection", "Clear selection", "Edit · Esc"),
    ("delete-pages", "Delete selected pages", "Edit · Del"),
    ("reverse-order", "Reverse page order", "Pages"),
    ("interleave", "Interleave pages", "Pages"),
    ("deinterleave", "De-interleave pages", "Pages"),
    ("rotate-left", "Rotate selected left", "Image"),
    ("rotate-right", "Rotate selected right", "Image"),
    ("flip-h", "Flip selected horizontally", "Image"),
    ("flip-v", "Flip selected vertically", "Image"),
    ("reset-page", "Reset selected pages", "Image"),
    ("toggle-theme", "Toggle light / dark theme", "View"),
    ("save-pdf", "Save as PDF…", "File · Ctrl+S"),
    ("save-tiff", "Save as TIFF…", "File"),
    ("save-jpeg", "Save as JPEG…", "File"),
    ("save-png", "Save as PNG…", "File"),
    ("batch-scan", "Batch scan…", "Scan"),
    ("edit-profiles", "Edit scan profiles…", "Scan"),
    ("ocr-run", "Make searchable (OCR)…", "OCR"),
    ("open-palette", "Command palette", "Ctrl+K"),
];

/// Look a command up by id.
pub fn command(id: &str) -> Option<Command> {
    COMMANDS.iter().find(|c| c.id == id).copied()
}
