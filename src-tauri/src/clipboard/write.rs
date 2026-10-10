//! 剪贴板写回：把 [`ClipboardItem`] 按类型写回系统剪贴板（text / html / rtf / image / files）。
//!
//! 时序约束：[`ClipboardContext`] 是 `!Send`，调用方需在不跨 await 的同步段内完成调用
//! （命令层照 `read_clipboard` 的写法处理）。
//!
//! 回环抑制：写回前向 [`WritebackGuard`] 登记将写入内容的 `content_hash`，
//! OS 监听重新读到同内容时跳过入库，避免「点击粘贴 → 自动新增一条」回环。
//! 哈希必须与 [`crate::clipboard::ingest::build_item`] 在 watcher 路径上将算出的哈希一致：
//! - text / html / rtf：watcher 拿到的 plain/html/rtf 经 `draft_from_text` 后 `content` 即我们写入的串，
//!   `content_hash(Text, written)` 自然匹配；
//! - files：watcher 把路径列表用 `\n` 连接后哈希，与我们 `item.content` 一致；
//! - image：watcher 把 PNG 字节再 sha256 → 文件名 → 哈希。前提是 OS pasteboard 不改像素，
//!   且 clipboard-rs 的 PNG 重新编码确定。绝大多数复制路径满足，极端情况可能漏抑制一次（最多多入一条新行）。
//!
//! 纯文本模式（`plain = true`）：忽略 `sub_kind`，写 `search_text`（OS 提供的纯文本表示），
//! 缺失时退回 `content`。供「纯文本粘贴」快捷路径使用。

use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContent, ClipboardContext, RustImageData};

use super::guard::WritebackGuard;
use super::storage::ImageStore;
use crate::core::{AppError, Result};
use crate::db::items::content_hash;
use crate::db::models::{ClipboardItem, ClipboardKind, ClipboardSubKind};

/// 把 `item` 写回系统剪贴板；`plain = true` 强制只写纯文本（剥离 HTML/RTF）。
pub fn write_to_clipboard(
    store: &ImageStore,
    guard: &WritebackGuard,
    item: &ClipboardItem,
    plain: bool,
    app: Option<&tauri::AppHandle>,
) -> Result<()> {
    let ctx = ClipboardContext::new().map_err(clip_err)?;

    match item.kind {
        ClipboardKind::Text => write_text(&ctx, guard, item, plain)?,
        ClipboardKind::Image => write_image(&ctx, store, guard, item, app)?,
        // files + plain：把路径列表当文本写回，供「粘贴为路径」使用。
        ClipboardKind::Files if plain => write_files_as_text(&ctx, guard, item)?,
        ClipboardKind::Files => write_files(&ctx, guard, item)?,
    }
    Ok(())
}

fn write_text(
    ctx: &ClipboardContext,
    guard: &WritebackGuard,
    item: &ClipboardItem,
    plain: bool,
) -> Result<()> {
    // 纯文本模式下，OS 提供的 plain 表示优先；缺失时退回 content（plain 文本场景下 content 即纯文本）。
    let (content, sub_kind) = if plain {
        let text = item
            .search_text
            .clone()
            .unwrap_or_else(|| item.content.clone());
        (text, None)
    } else {
        (item.content.clone(), item.sub_kind)
    };

    guard.suppress(content_hash(ClipboardKind::Text, &content));

    match sub_kind {
        // 纯文本模式必须只写 Text flavor，确保清掉剪贴板中可能残留的 HTML/RTF。
        None if plain => ctx
            .set(vec![ClipboardContent::Text(content)])
            .map_err(clip_err)?,
        // HTML / RTF 必须同时写入纯文本回退：clipboard-rs 的 set_html / set_rich_text
        // 会先 clearContents，单独写时只剩富格式，多数应用读 plain/text 拿不到就拒绝粘贴。
        // 走 set(Vec<ClipboardContent>) 一次写多格式（内部不再相互清空）。
        Some(ClipboardSubKind::Html) => {
            let plain = item.search_text.clone().unwrap_or_else(|| content.clone());
            guard.suppress(content_hash(ClipboardKind::Text, &plain));
            ctx.set(vec![
                ClipboardContent::Text(plain),
                ClipboardContent::Html(content),
            ])
            .map_err(clip_err)?;
        }
        Some(ClipboardSubKind::Rtf) => {
            let plain = item.search_text.clone().unwrap_or_else(|| content.clone());
            guard.suppress(content_hash(ClipboardKind::Text, &plain));
            ctx.set(vec![
                ClipboardContent::Text(plain),
                ClipboardContent::Rtf(content),
            ])
            .map_err(clip_err)?;
        }
        // url / email / color / path 及无 sub_kind 都走纯文本通道。
        _ => ctx.set_text(content).map_err(clip_err)?,
    }
    Ok(())
}

fn write_image(
    ctx: &ClipboardContext,
    store: &ImageStore,
    guard: &WritebackGuard,
    item: &ClipboardItem,
    app: Option<&tauri::AppHandle>,
) -> Result<()> {
    let path = store.origin_path(&item.content);
    let bytes = std::fs::read(&path).map_err(|err| {
        log::error!("read image {path:?} failed: {err}");
        AppError::Clipboard(err.to_string())
    })?;
    let image = RustImageData::from_bytes(&bytes).map_err(clip_err)?;

    guard.suppress(item.content_hash.clone());
    #[cfg(target_os = "windows")]
    {
        let _ = ctx;
        write_image_windows(&image, app)?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        ctx.set_image(image).map_err(clip_err)?;
    }
    Ok(())
}

/// Prepare portable PNG, 24-bit CF_DIB and alpha-capable CF_DIBV5 from the same
/// original pixels. PNG + V5 mirror Chromium's native Windows image formats;
/// explicit 24-bit CF_DIB avoids depending on Win32's bitmap synthesis for
/// web editors that inspect the available clipboard MIME types.
#[cfg(target_os = "windows")]
fn encode_windows_image_formats(image: &RustImageData) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let png = image.to_png().map_err(clip_err)?.get_bytes().to_vec();
    let bmp_v4 = image.to_bitmap().map_err(clip_err)?;
    let bmp = bmp_v4.get_bytes();
    const FILE_HEADER: usize = 14;
    const V4_HEADER: usize = 108;
    const V5_HEADER: usize = 124;
    let (width, height) = image.get_size();
    let invalid = || AppError::Clipboard("invalid Windows bitmap payload for clipboard".to_owned());
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(invalid());
    }
    if bmp.len() < FILE_HEADER + V4_HEADER || bmp.get(0..2) != Some(b"BM".as_slice()) {
        return Err(invalid());
    }
    let dib = &bmp[FILE_HEADER..];
    if u32::from_le_bytes(dib[0..4].try_into().map_err(|_| invalid())?) != V4_HEADER as u32
        || u32::from_le_bytes(bmp[10..14].try_into().map_err(|_| invalid())?)
            != (FILE_HEADER + V4_HEADER) as u32
    {
        return Err(invalid());
    }
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(invalid)?;
    let pixel_bytes = pixel_count.checked_mul(4).ok_or_else(invalid)?;
    if dib.len() != V4_HEADER + pixel_bytes {
        return Err(invalid());
    }

    // image-rs' RGBA BMP is a V4 BITFIELDS header (108 bytes) followed by
    // bottom-up BGRA pixels. V5 extends this exact header by 16 bytes for
    // rendering intent, profile offset/length and reserved field.
    let mut dib_v5 = Vec::with_capacity(V5_HEADER + pixel_bytes);
    dib_v5.extend_from_slice(&(V5_HEADER as u32).to_le_bytes());
    dib_v5.extend_from_slice(&dib[4..V4_HEADER]);
    dib_v5.extend_from_slice(&4u32.to_le_bytes()); // LCS_GM_IMAGES
    dib_v5.extend_from_slice(&[0u8; 12]); // No ICC profile, reserved = 0
    dib_v5.extend_from_slice(&dib[V4_HEADER..]);

    // CF_DIB fallback: common, uncompressed 24-bit bottom-up BGR. It has no
    // BITFIELDS masks or alpha; PNG and DIBV5 above retain transparency.
    let row_bytes = (width as usize)
        .checked_mul(3)
        .and_then(|len| len.checked_add(3))
        .map(|len| len & !3)
        .ok_or_else(invalid)?;
    let size = row_bytes.checked_mul(height as usize).ok_or_else(invalid)?;
    if size > u32::MAX as usize {
        return Err(invalid());
    }
    let mut dib_rgb = Vec::with_capacity(40 + size);
    dib_rgb.extend_from_slice(&40u32.to_le_bytes());
    dib_rgb.extend_from_slice(&(width as i32).to_le_bytes());
    dib_rgb.extend_from_slice(&(height as i32).to_le_bytes());
    dib_rgb.extend_from_slice(&1u16.to_le_bytes());
    dib_rgb.extend_from_slice(&24u16.to_le_bytes());
    dib_rgb.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    dib_rgb.extend_from_slice(&(size as u32).to_le_bytes());
    dib_rgb.extend_from_slice(&[0u8; 16]); // ppm x/y, palette count, important colors
    for bgra_row in dib[V4_HEADER..].chunks_exact(width as usize * 4) {
        for pixel in bgra_row.chunks_exact(4) {
            dib_rgb.extend_from_slice(&pixel[..3]);
        }
        dib_rgb.resize(dib_rgb.len() + row_bytes - width as usize * 3, 0);
    }
    Ok((png, dib_rgb, dib_v5))
}

/// Write image formats atomically while the clipboard is opened by our own
/// native Tauri window. OpenClipboard(NULL) + EmptyClipboard may leave
/// clipboard ownership NULL and cause SetClipboardData to fail on Windows.
#[cfg(target_os = "windows")]
fn write_image_windows(image: &RustImageData, app: Option<&tauri::AppHandle>) -> Result<()> {
    use clipboard_win::{formats, raw};
    let (png, dib, dib_v5) = encode_windows_image_formats(image)?;
    let png_format = clipboard_win::register_format("PNG").ok_or_else(|| {
        AppError::Clipboard("register Windows PNG clipboard format failed".to_owned())
    })?;
    let _clipboard = open_windows_image_clipboard(app)?;
    raw::empty().map_err(|err| {
        AppError::Clipboard(format!("clear Windows clipboard for image failed: {err}"))
    })?;

    // Chromium registers PNG and CF_DIBV5. Many paste event handlers also
    // require CF_DIB before advertising image/png in DataTransfer.
    let png_result = raw::set_without_clear(png_format.get(), &png);
    let dib_v5_result = raw::set_without_clear(formats::CF_DIBV5, &dib_v5);
    let dib_result = raw::set_without_clear(formats::CF_DIB, &dib);

    if png_result.is_err() || dib_result.is_err() || dib_v5_result.is_err() {
        log::warn!(
            "image format publish: PNG={png_result:?}, CF_DIBV5={dib_v5_result:?}, CF_DIB={dib_result:?}"
        );
    }
    // PNG-only can look successful to a tolerant consumer but fail in
    // Chromium's image paste path. Require at least one native DIB format.
    if dib_result.is_err() && dib_v5_result.is_err() {
        return Err(AppError::Clipboard(format!(
            "Windows image formats unavailable: CF_DIB={dib_result:?}, CF_DIBV5={dib_v5_result:?}, PNG={png_result:?}"
        )));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn open_windows_image_clipboard(
    app: Option<&tauri::AppHandle>,
) -> Result<clipboard_win::Clipboard> {
    use tauri::Manager;
    let owner: clipboard_win::types::HWND = if let Some(app) = app {
        let window = app
            .get_webview_window(crate::window::CLIPBOARD_WINDOW_LABEL)
            .ok_or_else(|| AppError::Clipboard("main clipboard window not found".to_owned()))?;
        let hwnd = window.hwnd().map_err(clip_err)?;
        if hwnd.0 as isize == 0 {
            return Err(AppError::Clipboard(
                "main clipboard window handle is NULL".to_owned(),
            ));
        }
        hwnd.0 as clipboard_win::types::HWND
    } else {
        // Only legacy desktop-only tests call this path without an AppHandle.
        std::ptr::null_mut()
    };
    let mut last_error = String::new();
    for delay_ms in [0, 10, 30, 70, 140] {
        if delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
        match clipboard_win::Clipboard::new_attempts_for(owner, 10) {
            Ok(clipboard) => return Ok(clipboard),
            Err(err) => last_error = err.to_string(),
        }
    }
    Err(AppError::Clipboard(format!(
        "open Windows clipboard for image failed: {last_error}"
    )))
}

fn write_files(ctx: &ClipboardContext, guard: &WritebackGuard, item: &ClipboardItem) -> Result<()> {
    let paths: Vec<String> = item
        .content
        .split('\n')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if paths.is_empty() {
        return Err(AppError::Clipboard("no files to write".to_owned()));
    }

    guard.suppress(item.content_hash.clone());
    ctx.set_files(paths).map_err(clip_err)?;
    Ok(())
}

/// 把 files 条目的路径列表当文本写回（换行分隔，多文件按行展开）。
/// 与 `write_files` 共用 `content_hash` 抑制——OS 监听不会拿到与原文本完全一致的回环。
fn write_files_as_text(
    ctx: &ClipboardContext,
    guard: &WritebackGuard,
    item: &ClipboardItem,
) -> Result<()> {
    let text = item.content.clone();

    guard.suppress(content_hash(ClipboardKind::Text, &text));
    ctx.set_text(text).map_err(clip_err)?;
    Ok(())
}

fn clip_err<E: std::fmt::Display>(err: E) -> AppError {
    AppError::Clipboard(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::payload::ImagePayload;
    use super::super::read::ClipboardReader;
    use super::*;
    use crate::clipboard::{build_item, ImageStore, WritebackGuard};
    use crate::db::models::Platform;
    use chrono::Utc;

    fn text_item(
        content: &str,
        sub: Option<ClipboardSubKind>,
        search: Option<&str>,
    ) -> ClipboardItem {
        ClipboardItem {
            id: uuid::Uuid::new_v4().to_string(),
            kind: ClipboardKind::Text,
            sub_kind: sub,
            group_id: None,
            source_app_id: None,
            content_hash: content_hash(ClipboardKind::Text, content),
            content: content.to_owned(),
            search_text: search.map(str::to_owned),
            summary: None,
            file_types: None,
            size: None,
            width: None,
            height: None,
            use_count: 1,
            is_favorite: false,
            is_pinned: false,
            priority_order: None,
            pin_order: None,
            is_sensitive: false,
            platform: Platform::Macos,
            note: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            source_app_name: None,
            source_app_icon_file: None,
            source_app_icon_path: None,
            image_thumbnail_path: None,
            file_entries: None,
            files_preview_kind: None,
            available_actions: Vec::new(),
            color_preview: None,
            display_created_at: String::new(),
        }
    }

    fn temp_store() -> (TempDir, ImageStore) {
        let dir = TempDir::new();
        let store = ImageStore::for_test(dir.path().join("resources").join("clipboard-images"));
        (dir, store)
    }

    // 触碰真实剪贴板：写入纯文本 → 读回应为同串，且 guard 已登记本次哈希。
    #[test]
    #[ignore = "touches the real system clipboard; run with --ignored on a desktop session"]
    fn writes_plain_text_and_arms_guard() {
        let _serial = crate::clipboard::test_lock::serial();
        let (_dir, store) = temp_store();
        let guard = WritebackGuard::new();

        let item = text_item("hello write", None, None);
        write_to_clipboard(&store, &guard, &item, false, None).unwrap();

        let reader = ClipboardReader::new().unwrap();
        let payload = reader
            .read_with_capture(&crate::settings::Capture::default())
            .unwrap()
            .expect("should read");
        let read_item = build_item(&store, &payload).unwrap().unwrap();
        assert_eq!(read_item.content, "hello write");
        assert!(guard.should_skip(&read_item.content_hash));
    }

    // 纯文本模式：强制丢弃 HTML，写 search_text。
    #[test]
    #[ignore = "touches the real system clipboard; run with --ignored on a desktop session"]
    fn plain_mode_strips_html() {
        let _serial = crate::clipboard::test_lock::serial();
        let (_dir, store) = temp_store();
        let guard = WritebackGuard::new();

        let item = text_item(
            "<b>Hello</b> World",
            Some(ClipboardSubKind::Html),
            Some("Hello World"),
        );
        write_to_clipboard(&store, &guard, &item, true, None).unwrap();

        let reader = ClipboardReader::new().unwrap();
        let payload = reader
            .read_with_capture(&crate::settings::Capture::default())
            .unwrap()
            .expect("should read");
        let read_item = build_item(&store, &payload).unwrap().unwrap();
        assert_eq!(read_item.kind, ClipboardKind::Text);
        assert_eq!(read_item.sub_kind, None);
        assert_eq!(read_item.content, "Hello World");
    }

    // 图片往返：写盘上的 PNG → 写剪贴板 → 读回 → 落盘的文件名应一致（去重哈希命中）。
    #[test]
    #[ignore = "touches the real system clipboard; run with --ignored on a desktop session"]
    fn round_trip_image_matches_hash() {
        let _serial = crate::clipboard::test_lock::serial();
        let (_dir, store) = temp_store();
        let guard = WritebackGuard::new();

        // 先落盘一张原图（模拟历史记录里的 image item）。
        let png = sample_png(48, 32);
        let stored = store
            .store(&ImagePayload {
                bytes: png,
                width: 48,
                height: 32,
            })
            .unwrap();
        let item = ClipboardItem {
            id: uuid::Uuid::new_v4().to_string(),
            kind: ClipboardKind::Image,
            sub_kind: None,
            group_id: None,
            source_app_id: None,
            content_hash: content_hash(ClipboardKind::Image, &stored.file_name),
            content: stored.file_name.clone(),
            search_text: None,
            summary: None,
            file_types: None,
            size: Some(stored.size),
            width: Some(stored.width),
            height: Some(stored.height),
            use_count: 1,
            is_favorite: false,
            is_pinned: false,
            priority_order: None,
            pin_order: None,
            is_sensitive: false,
            platform: Platform::Macos,
            note: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            source_app_name: None,
            source_app_icon_file: None,
            source_app_icon_path: None,
            image_thumbnail_path: None,
            file_entries: None,
            files_preview_kind: None,
            available_actions: Vec::new(),
            color_preview: None,
            display_created_at: String::new(),
        };

        write_to_clipboard(&store, &guard, &item, false, None).unwrap();

        let reader = ClipboardReader::new().unwrap();
        let payload = reader
            .read_with_capture(&crate::settings::Capture::default())
            .unwrap()
            .expect("should read image");
        let read_item = build_item(&store, &payload).unwrap().unwrap();
        assert_eq!(read_item.kind, ClipboardKind::Image);
        // 往返期望 PNG 字节哈希一致 → 同 content_hash → guard 抑制。
        assert_eq!(read_item.content_hash, item.content_hash);
        assert!(guard.should_skip(&read_item.content_hash));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn image_formats_preserve_png_dib24_and_dibv5_headers() {
        let png = sample_png(3, 2);
        let image = RustImageData::from_bytes(&png).unwrap();
        let (png_result, dib, dib_v5) = encode_windows_image_formats(&image).unwrap();
        assert!(png_result.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 3);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 24);
        assert_eq!(u32::from_le_bytes(dib[16..20].try_into().unwrap()), 0);
        assert_eq!(dib.len(), 40 + 12 * 2); // 3xBGR plus 3 bytes row padding.
        assert_eq!(u32::from_le_bytes(dib_v5[0..4].try_into().unwrap()), 124);
        assert_eq!(u16::from_le_bytes(dib_v5[14..16].try_into().unwrap()), 32);
        assert_eq!(u32::from_le_bytes(dib_v5[16..20].try_into().unwrap()), 3);
        assert_eq!(
            u32::from_le_bytes(dib_v5[52..56].try_into().unwrap()),
            0xff00_0000
        ); // alpha mask
        assert_eq!(
            u32::from_le_bytes(dib_v5[56..60].try_into().unwrap()),
            0x7352_4742
        ); // sRGB
        assert_eq!(dib_v5.len(), 124 + 3 * 2 * 4);
        // Decode both independently of a live desktop clipboard. A valid
        // header alone is insufficient if the pixel data is misaligned.
        for expected_dib in [&dib, &dib_v5] {
            let bitmap_header_len =
                u32::from_le_bytes(expected_dib[0..4].try_into().unwrap()) as usize;
            let total_len = 14 + expected_dib.len();
            let mut file = Vec::with_capacity(total_len);
            file.extend_from_slice(b"BM");
            file.extend_from_slice(&(total_len as u32).to_le_bytes());
            file.extend_from_slice(&[0u8; 4]);
            file.extend_from_slice(&((14 + bitmap_header_len) as u32).to_le_bytes());
            file.extend_from_slice(expected_dib);
            let decoded =
                image::load_from_memory_with_format(&file, image::ImageFormat::Bmp).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (3, 2));
        }
    }

    fn sample_png(w: u32, h: u32) -> Vec<u8> {
        use std::io::Cursor;
        let buf = image::RgbaImage::from_pixel(w, h, image::Rgba([4, 5, 6, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("ecopaste-write-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
}
