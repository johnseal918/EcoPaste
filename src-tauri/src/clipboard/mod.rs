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

// Serialize only the actual Windows clipboard transactions, not image
// decoding/encoding. A delayed GetClipboardData can still block in Win32.
#[cfg(target_os = "windows")]
static WINDOWS_ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(target_os = "windows")]
static WINDOWS_WRITE_WAITING: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

#[cfg(target_os = "windows")]
fn lock_windows_capture() -> crate::core::Result<std::sync::MutexGuard<'static, ()>> {
    use std::sync::atomic::Ordering;
    if WINDOWS_WRITE_WAITING.load(Ordering::Acquire) > 0 {
        return Err(crate::core::AppError::Clipboard(
            "Windows clipboard busy: EcoPaste write pending".to_owned(),
        ));
    }
    match WINDOWS_ACCESS.try_lock() {
        Ok(guard) => {
            if WINDOWS_WRITE_WAITING.load(Ordering::Acquire) > 0 {
                return Err(crate::core::AppError::Clipboard(
                    "Windows clipboard busy: EcoPaste write pending".to_owned(),
                ));
            }
            Ok(guard)
        }
        Err(std::sync::TryLockError::Poisoned(err)) => Ok(err.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => Err(
            crate::core::AppError::Clipboard(
                "Windows clipboard busy: EcoPaste operation in progress".to_owned(),
            ),
        ),
    }
}

// A capture that already owns the gate can check again between optional
// clipboard formats and yield promptly to the next explicit user write.
#[cfg(target_os = "windows")]
fn windows_write_pending() -> bool {
    use std::sync::atomic::Ordering;
    WINDOWS_WRITE_WAITING.load(Ordering::Acquire) > 0
}

// Writers stop new captures while waiting, so a stream of clipboard events
// cannot indefinitely starve a user's explicit paste request.
#[cfg(target_os = "windows")]
fn lock_windows_write() -> crate::core::Result<std::sync::MutexGuard<'static, ()>> {
    use std::sync::atomic::Ordering;
    struct PendingWrite;
    impl Drop for PendingWrite {
        fn drop(&mut self) {
            WINDOWS_WRITE_WAITING.fetch_sub(1, Ordering::Release);
        }
    }
    WINDOWS_WRITE_WAITING.fetch_add(1, Ordering::AcqRel);
    let _pending = PendingWrite;
    let started = std::time::Instant::now();
    loop {
        let guard = match WINDOWS_ACCESS.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::Poisoned(err)) => Some(err.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
        if let Some(guard) = guard {
            log::info!("clipboard write gate acquired wait_ms={}", started.elapsed().as_millis());
            return Ok(guard);
        }
        if started.elapsed() >= std::time::Duration::from_secs(2) {
            log::warn!("clipboard write gate busy wait_ms={}", started.elapsed().as_millis());
            return Err(crate::core::AppError::Clipboard(
                "Windows clipboard busy: in-flight EcoPaste capture did not finish".to_owned(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
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

