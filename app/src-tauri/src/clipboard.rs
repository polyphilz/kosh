use std::sync::mpsc;

use tauri::AppHandle;

use crate::database::{commands::CommandError, DatabaseError};

const MAX_CLIPBOARD_TEXT_BYTES: usize = 16 * 1024;

#[tauri::command]
pub(crate) async fn copy_text<R: tauri::Runtime>(
    app: AppHandle<R>,
    text: String,
) -> Result<(), CommandError> {
    validate_clipboard_text(&text).map_err(CommandError::from)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = sender.send(write_text_on_main_thread(&text));
    })
    .map_err(|error| {
        CommandError::from(DatabaseError::InvalidInput(format!(
            "could not access the clipboard: {error}"
        )))
    })?;
    tauri::async_runtime::spawn_blocking(move || receiver.recv())
        .await
        .map_err(|error| CommandError::worker(error.to_string()))?
        .map_err(|_| CommandError::from(DatabaseError::WriterUnavailable))?
        .map_err(CommandError::from)
}

fn validate_clipboard_text(text: &str) -> Result<(), DatabaseError> {
    if text.is_empty() {
        return Err(DatabaseError::InvalidInput(
            "clipboard text cannot be empty".into(),
        ));
    }
    if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
        return Err(DatabaseError::InvalidInput(format!(
            "clipboard text is larger than {MAX_CLIPBOARD_TEXT_BYTES} bytes"
        )));
    }
    if text.contains('\0') {
        return Err(DatabaseError::InvalidInput(
            "clipboard text cannot contain a null byte".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn write_text_on_main_thread(text: &str) -> Result<(), DatabaseError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;

    MainThreadMarker::new().ok_or_else(|| {
        DatabaseError::InvalidInput("the clipboard was not written on the macOS main thread".into())
    })?;
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    if !pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString }) {
        return Err(DatabaseError::InvalidInput(
            "macOS rejected the clipboard write".into(),
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn write_text_on_main_thread(_text: &str) -> Result<(), DatabaseError> {
    Err(DatabaseError::InvalidInput(
        "clipboard text writing is available only on macOS".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{validate_clipboard_text, MAX_CLIPBOARD_TEXT_BYTES};

    #[test]
    fn clipboard_text_validation_is_bounded_and_null_safe() {
        assert!(validate_clipboard_text("http://tauri.localhost/#/notes/1").is_ok());
        assert!(validate_clipboard_text("").is_err());
        assert!(validate_clipboard_text("bad\0url").is_err());
        assert!(validate_clipboard_text(&"a".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1)).is_err());
    }
}
