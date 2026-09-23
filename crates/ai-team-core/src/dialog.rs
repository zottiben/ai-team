//! A dialog, for a process that has nowhere to print.
//!
//! The desktop app failing before its window exists, and `ait` started by Finder rather
//! than a shell, are the same situation: stderr goes nowhere anybody reads, so something
//! that has to be said has to be put on screen.

/// Show `message` in a dialog and wait for it to be dismissed.
///
/// Tauri's dialog plugin is not loaded when the app fails to start and is not linked into
/// `ait` at all, so this uses the platform's own facility and does nothing where there is
/// not one - the caller prints to stderr as well.
pub fn alert(message: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display dialog {} with title \"ai-team\" buttons {{\"OK\"}} with icon caution",
            applescript_string(message)
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .status();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = message;
    }
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
