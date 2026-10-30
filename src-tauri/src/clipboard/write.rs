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
#[cfg(not(target_os = "windows"))]
use clipboard_rs::Clipboard;
use clipboard_rs::{ClipboardContent, ClipboardContext, RustImageData};

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
    // Decode image files and prepare text outside the Win32 transaction gate.
    let ctx = ClipboardContext::new().map_err(clip_err)?;

    match item.kind {
        ClipboardKind::Text => write_text(&ctx, guard, item, plain, app)?,
        ClipboardKind::Image => write_image(&ctx, store, guard, item, app)?,
        // files + plain：把路径列表当文本写回，供「粘贴为路径」使用。
        ClipboardKind::Files if plain => write_files_as_text(&ctx, guard, item, app)?,
        ClipboardKind::Files => write_files(&ctx, guard, item, app)?,
    }
    Ok(())
}

/// 编辑器临时纯文本写回：不创建历史记录，复用既有回环抑制。
pub fn write_edited_plain_text(guard: &WritebackGuard, text: &str, app: &tauri::AppHandle) -> Result<()> {
    let ctx = ClipboardContext::new().map_err(clip_err)?;
    guard.suppress(content_hash(ClipboardKind::Text, text));
    set_contents(&ctx, vec![ClipboardContent::Text(text.to_owned())], Some(app))
}

fn write_text(
    ctx: &ClipboardContext,
    guard: &WritebackGuard,
    item: &ClipboardItem,
    plain: bool,
    app: Option<&tauri::AppHandle>,
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
        None if plain => set_contents(ctx, vec![ClipboardContent::Text(content)], app)?,
        // HTML / RTF 必须同时写入纯文本回退：clipboard-rs 的 set_html / set_rich_text
        // 会先 clearContents，单独写时只剩富格式，多数应用读 plain/text 拿不到就拒绝粘贴。
        // 走 set(Vec<ClipboardContent>) 一次写多格式（内部不再相互清空）。
        Some(ClipboardSubKind::Html) => {
            let plain = item.search_text.clone().unwrap_or_else(|| content.clone());
            guard.suppress(content_hash(ClipboardKind::Text, &plain));
            set_contents(ctx, vec![
                ClipboardContent::Text(plain),
                ClipboardContent::Html(content),
            ], app)?;
        }
        Some(ClipboardSubKind::Rtf) => {
            let plain = item.search_text.clone().unwrap_or_else(|| content.clone());
            guard.suppress(content_hash(ClipboardKind::Text, &plain));
            set_contents(ctx, vec![
                ClipboardContent::Text(plain),
                ClipboardContent::Rtf(content),
            ], app)?;
        }
        // url / email / color / path 及无 sub_kind 都走纯文本通道。
        _ => set_contents(ctx, vec![ClipboardContent::Text(content)], app)?,
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
    let _access = super::lock_windows_write()?;
    let _clipboard = open_windows_clipboard(app)?;
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
fn open_windows_clipboard(
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
        return Err(AppError::Clipboard("Windows clipboard write requires an application window".to_owned()));
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
        "open Windows clipboard for write failed: {last_error}"
    )))
}

fn write_files(ctx: &ClipboardContext, guard: &WritebackGuard, item: &ClipboardItem, app: Option<&tauri::AppHandle>) -> Result<()> {
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
    set_contents(ctx, vec![ClipboardContent::Files(paths)], app)?;
    Ok(())
}

/// 把 files 条目的路径列表当文本写回（换行分隔，多文件按行展开）。
/// 与 `write_files` 共用 `content_hash` 抑制——OS 监听不会拿到与原文本完全一致的回环。
fn write_files_as_text(
    ctx: &ClipboardContext,
    guard: &WritebackGuard,
    item: &ClipboardItem,
    app: Option<&tauri::AppHandle>,
) -> Result<()> {
    let text = item.content.clone();

    guard.suppress(content_hash(ClipboardKind::Text, &text));
    set_contents(ctx, vec![ClipboardContent::Text(text)], app)?;
    Ok(())
}

/// Publish every requested format in one checked transaction. Do not call
/// clipboard-rs::set here: it ignores both open and per-format write failures.
fn set_contents(ctx: &ClipboardContext, contents: Vec<ClipboardContent>, app: Option<&tauri::AppHandle>) -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        ctx.set(contents).map_err(clip_err)
    }
    #[cfg(target_os = "windows")]
    {
        use clipboard_win::{options, raw};
        let _ = ctx;
        if contents.iter().any(|value| !matches!(value, ClipboardContent::Text(_) | ClipboardContent::Html(_) | ClipboardContent::Rtf(_) | ClipboardContent::Files(_))) {
            return Err(AppError::Clipboard("unsupported Windows clipboard write format".to_owned()));
        }
        // Do not require registration of HTML/RTF when writing ordinary text.
        let html = if contents.iter().any(|v| matches!(v, ClipboardContent::Html(_))) {
            Some(clipboard_win::formats::Html::new().ok_or_else(|| {
                AppError::Clipboard("register Windows HTML format failed".to_owned())
            })?)
        } else {
            None
        };
        let rtf = if contents.iter().any(|v| matches!(v, ClipboardContent::Rtf(_))) {
            Some(clipboard_win::register_format("Rich Text Format").ok_or_else(|| {
                AppError::Clipboard("register Windows RTF format failed".to_owned())
            })?)
        } else {
            None
        };
        let _access = super::lock_windows_write()?;
        let _clipboard = open_windows_clipboard(app)?;
        raw::empty().map_err(clip_err)?;
        let mut plain_written = false;
        for content in contents {
            let (result, optional_rich): (clipboard_win::SysResult<()>, bool) = match content {
                ClipboardContent::Text(text) => {
                    (raw::set_string_with(&text, options::NoClear), false)
                }
                ClipboardContent::Html(text) => (
                    raw::set_html_with(html.as_ref().expect("HTML format pre-registered").code(), &text, options::NoClear),
                    true,
                ),
                ClipboardContent::Rtf(text) => (
                    raw::set_without_clear(rtf.as_ref().expect("RTF format pre-registered").get(), text.as_bytes()),
                    true,
                ),
                ClipboardContent::Files(paths) => {
                    (raw::set_file_list_with(&paths, options::NoClear), false)
                }
                _ => unreachable!("formats validated before opening clipboard"),
            };
            match result {
                Ok(()) => {
                    if !optional_rich {
                        plain_written = true;
                    }
                }
                Err(err) if optional_rich && plain_written => {
                    // A rich format is optional only when the plain-text fallback
                    // was already successfully published. Never hide text errors.
                    log::warn!("clipboard optional rich format publication failed: {err}");
                }
                Err(err) => {
                    log::warn!("clipboard required format publication failed: {err}");
                    return Err(clip_err(err));
                }
            }
        }
        Ok(())
    }
}

fn clip_err<E: std::fmt::Display>(err: E) -> AppError {
    AppError::Clipboard(err.to_string())
}

