mod app_store;
mod apps_registry;
mod cleanup;
mod detect;
#[cfg(target_os = "windows")]
mod emf;
mod file_icon_store;
mod guard;
mod icon;
mod ingest;
mod payload;
mod read;
mod secrets;
mod sound;
mod source;
mod storage;
mod watcher;
mod write;

// OpenClipboard(NULL) is not a mutex between threads in the same process:
// either thread's CloseClipboard invalidates the other's open transaction.
#[cfg(target_os = "windows")]
static WINDOWS_ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_os = "windows")]
fn lock_windows_write() -> crate::core::Result<std::sync::MutexGuard<'static, ()>> {
    // ponytail: one process-wide lock; bound waiting for delayed source rendering.
    for attempt in 0..=25 {
        match WINDOWS_ACCESS.try_lock() {
            Ok(access) => return Ok(access),
            Err(std::sync::TryLockError::Poisoned(err)) => return Ok(err.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) if attempt < 25 => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
    Err(crate::core::AppError::Clipboard(
        "EcoPaste is still reading the clipboard; please retry shortly".to_owned(),
    ))
}

#[cfg(all(test, target_os = "windows"))]
mod windows_access_regression {
    #[test]
    fn active_transaction_survives_competing_capture_and_write() {
        let _access = super::WINDOWS_ACCESS.lock().unwrap();
        let _clipboard = clipboard_win::Clipboard::new_attempts(10).unwrap();
        let (capture_busy, write_busy) = std::thread::spawn(|| {
            let reader = super::ClipboardReader::new().unwrap();
            (
                reader.read_with_capture(&crate::settings::Capture::default()).is_err(),
                super::lock_windows_write().is_err(),
            )
        }).join().unwrap();
        assert!(capture_busy && write_busy);
        unsafe {
            windows::Win32::Foundation::SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
            windows::Win32::System::DataExchange::EnumClipboardFormats(0);
            windows::Win32::Foundation::GetLastError()
                .expect("competing operations must not close the active clipboard transaction");
        }
    }
}

pub use app_store::AppIconStore;
pub use apps_registry::{
    add_app_from_path, delete_unreferenced_apps, refresh_running_apps, AppsRegistry,
};
pub use detect::sanitize_css_color;
pub use file_icon_store::FileIconStore;
pub use guard::WritebackGuard;
pub use icon::{get_icon_cache_key, icon_png, DIR_CACHE_KEY};
pub use ingest::build_item_with_settings;
pub use payload::{ClipboardPayload, ImagePayload, TextPayload};
pub use read::ClipboardReader;
pub use sound::play_copy_sound;
pub use source::detect_frontmost;
pub use storage::ImageStore;
pub use watcher::{init, materialize_source, persist_and_notify, WatcherPause};
pub use write::{write_edited_plain_text, write_to_clipboard};

