//! 剪贴板读取：基于 [`clipboard_rs`] 的薄封装，产出带类型标记的 [`ClipboardPayload`]。
//!
//! [`clipboard_rs`] 已内置 macOS（`NSPasteboard`）/ Windows 的平台读取，
//! 这里只负责按用户设置的优先级归类，并过滤掉空内容。
//!
//! 图片读取走「PNG 直通优先」：源本身是 PNG 时直取原始字节、从头部解析尺寸（零解码/编码）；
//! 仅当源是 TIFF/DIB 等非 PNG 时才回退到库的解码 + 重编码 PNG。

use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, ContentFormat};
#[cfg(target_os = "windows")]
use winapi::um::winuser::{IsClipboardFormatAvailable, RegisterClipboardFormatW};
#[cfg(target_os = "windows")]
use clipboard_win::{raw as clipboard_raw, Clipboard as WindowsClipboard};
use std::time::Duration;
#[cfg(target_os = "windows")]
use std::time::Instant;
#[cfg(target_os = "windows")]
use std::sync::Mutex;
#[cfg(target_os = "windows")]
use super::worker::WorkerSession;

use super::payload::{ClipboardPayload, ImagePayload, TextPayload};
#[cfg(target_os = "windows")]
use super::emf;
#[cfg(target_os = "windows")]
use super::source;
use crate::core::{AppError, Result};
use crate::settings::{Capture, CaptureKind};

/// Metadata-only request stage. Start is logged BEFORE Win32 clipboard calls;
/// the corresponding end is logged after all following RAII clipboard locks
/// have been released. A blocked delayed render still leaves the begin record.
#[cfg(target_os = "windows")]
struct NativeClipboardStage {
    format_group: &'static str,
    started: Instant,
    sequence: u32,
}

#[cfg(target_os = "windows")]
impl NativeClipboardStage {
    fn begin(format_group: &'static str) -> Self {
        let sequence = unsafe { winapi::um::winuser::GetClipboardSequenceNumber() };
        log::info!(
            "clipboard native_read begin group={format_group} sequence={sequence} owner_pid={:?}",
            source::clipboard_owner_pid()
        );
        Self { format_group, started: Instant::now(), sequence }
    }
}

#[cfg(target_os = "windows")]
impl Drop for NativeClipboardStage {
    fn drop(&mut self) {
        log::info!(
            "clipboard native_read end group={} sequence={} elapsed_ms={}",
            self.format_group,
            self.sequence,
            self.started.elapsed().as_millis()
        );
    }
}

/// 持有一个 [`ClipboardContext`] 的读取器。轻量，可按需创建；
/// 监听线程（2.2）会持有一个长生命周期实例复用。
pub struct ClipboardReader {
    ctx: ClipboardContext,
    #[cfg(target_os = "windows")]
    worker: Mutex<WorkerSession>,
}

impl ClipboardReader {
    pub fn new() -> Result<Self> {
        let ctx = ClipboardContext::new().map_err(clip_err)?;
        Ok(Self {
            ctx,
            #[cfg(target_os = "windows")]
            worker: Mutex::new(WorkerSession::default()),
        })
    }

    /// 读取当前剪贴板，按用户配置的内容类型顺序归类为单一载荷。
    ///
    /// 同一次复制可能同时存在文件、图片、HTML、RTF、纯文本表示；这里只返回第一个启用且可读取的类型。
    pub fn read_with_capture(&self, capture: &Capture) -> Result<Option<ClipboardPayload>> {
        self.read_with_capture_age(capture, std::time::Duration::MAX)
    }

    /// The watcher supplies the age of the still-current sequence. A manual
    /// read needs no provisional file/image deferral and uses MAX.
    pub fn read_with_capture_age(
        &self,
        capture: &Capture,
        clipboard_age: std::time::Duration,
    ) -> Result<Option<ClipboardPayload>> {
        // A Win32 delayed-render call cannot be preempted in a Rust thread.
        // The persistent helper can be retired without blocking a user paste.
        #[cfg(target_os = "windows")]
        {
            return self.worker
                .lock()
                .map_err(|_| AppError::Clipboard("clipboard worker mutex poisoned".to_owned()))?
                .capture(capture, clipboard_age);
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.read_native_with_capture_age(capture, clipboard_age)
        }
    }

    /// Native path executed by the separate Windows helper process. macOS
    /// uses it directly; no second watcher or alternative database flow.
    pub(super) fn read_native_with_capture_age(
        &self,
        capture: &Capture,
        clipboard_age: Duration,
    ) -> Result<Option<ClipboardPayload>> {
        #[cfg(not(target_os = "windows"))]
        let _ = clipboard_age;
        // During Excel copy the delayed bitmap can precede BIFF publication.
        // Prefer available Unicode text without triggering the delayed picture
        // even when the BIFF metadata is temporarily absent. Picture-only copies
        // without Unicode text continue through the normal image branch.
        #[cfg(target_os = "windows")]
        if source::clipboard_owner_is_excel() {
            let has_cells = is_excel_cell_clipboard();
            let has_text = self.ctx.has(ContentFormat::Text);
            if capture.text && has_text {
                log::info!(
                    "clipboard excel selected=plain_text biff_available={has_cells} owner_pid={:?}",
                    source::clipboard_owner_pid()
                );
                return self
                    .read_excel_plain_text()
                    .map(|text| text.map(ClipboardPayload::Text));
            }
            if has_cells && capture.text {
                // Cell text is not yet offered; do not request the large image.
                return Ok(None);
            }
            // Explicit Excel Copy as Picture (Picture, not Bitmap) can provide
            // only WMF/EMF. Never request it when cell or text formats exist:
            // ordinary column/whole-sheet copies may advertise delayed EMF.
            if capture.image && !has_text && !has_cells && emf::is_advertised() {
                log::info!("clipboard excel image=vector_metafile selected=emf_fallback");
                return emf::capture_png().map(|image| image.map(ClipboardPayload::Image));
            }
        }

        // WeChat may advertise a file drop before its picture becomes usable.
        // Defer the provisional files record briefly, but keep real file copy
        // support when no image formats arrive. Never treat CF_HDROP as proof
        // of an image, and do not replace the user's global order.
        #[cfg(target_os = "windows")]
        if capture.image && source::clipboard_owner_is_wechat() {
            let picture_advertised = self.ctx.has(ContentFormat::Image)
                || self.windows_registered_png_available();
            if picture_advertised {
                log::debug!("clipboard wechat picture_advertised=true");
                // A temporarily unreadable image is a retryable error, not a
                // reason to commit an unrelated temporary file as the picture.
                return self.read_image().map(|image| image.map(ClipboardPayload::Image));
            }
            if capture.files
                && self.ctx.has(ContentFormat::Files)
                && clipboard_age < std::time::Duration::from_millis(420)
            {
                return Ok(None);
            }
        }

        let mut text_payload: Option<Option<TextPayload>> = None;

        for kind in capture.ordered_kinds() {
            if !capture.is_enabled(kind) {
                continue;
            }

            match kind {
                CaptureKind::Files => {
                    if let Some(files) = self.read_files()? {
                        return Ok(Some(ClipboardPayload::Files(files)));
                    }
                }
                CaptureKind::Image => {
                    if self.ctx.has(ContentFormat::Image) {
                        if let Some(image) = self.read_image()? {
                            return Ok(Some(ClipboardPayload::Image(image)));
                        }
                    }
                }
                CaptureKind::Html | CaptureKind::Rtf | CaptureKind::Text => {
                    if text_payload.is_none() {
                        text_payload = Some(self.read_text_payload(capture)?);
                    }

                    let Some(text) = text_payload.as_ref().and_then(|payload| payload.as_ref())
                    else {
                        continue;
                    };

                    if text_contains_kind(text, kind) {
                        return Ok(Some(ClipboardPayload::Text(text.clone())));
                    }
                }
            }
        }

        Ok(None)
    }

    /// 读取剪贴板文件路径列表；空列表视为无可用文件内容。
    fn read_files(&self) -> Result<Option<Vec<String>>> {
        if !self.ctx.has(ContentFormat::Files) {
            return Ok(None);
        }

        let files: Vec<String> = {
            #[cfg(target_os = "windows")]
            let _stage = NativeClipboardStage::begin("CF_HDROP");
            #[cfg(target_os = "windows")]
            let _access = super::lock_windows_capture()?;
            self.ctx
                .get_files()
                .map_err(clip_err)?
                .into_iter()
                .filter(|path| !path.is_empty())
                .collect()
        };
        if files.is_empty() {
            return Ok(None);
        }

        Ok(Some(files))
    }

    /// Windows Excel 单元格仅读取原始 Unicode 字节，并在释放剪贴板后转码。
    /// 这条路径不调用 clipboard-rs::get，避免持锁期间再次 OpenClipboard。
    /// GetClipboardData 本身仍可能触发来源程序不可中断的延迟渲染。
    #[cfg(target_os = "windows")]
    fn read_excel_plain_text(&self) -> Result<Option<TextPayload>> {
        let sequence_before = unsafe { winapi::um::winuser::GetClipboardSequenceNumber() };
        let started = Instant::now();
        log::info!(
            "clipboard excel unicode_read=start sequence={sequence_before} owner_pid={:?}",
            source::clipboard_owner_pid()
        );
        let mut raw = Vec::new();
        let locked_for = {
            let _access = super::lock_windows_capture()?;
            // A single open attempt. The watcher handles bounded retries
            // instead of retrying while Excel is still publishing formats.
            let _clip = WindowsClipboard::new().map_err(|err| {
                AppError::Clipboard(format!("Windows clipboard text snapshot busy: {err}"))
            })?;
            let opened = Instant::now();
            clipboard_raw::get_vec(clipboard_win::formats::CF_UNICODETEXT, &mut raw)
                .map_err(|err| {
                    AppError::Clipboard(format!("Excel Unicode clipboard read failed: {err}"))
                })?;
            opened.elapsed()
        }; // CloseClipboard before decoding or logging.
        let sequence_after = unsafe { winapi::um::winuser::GetClipboardSequenceNumber() };
        log::info!(
            "clipboard excel unicode_read=closed seq_before={sequence_before} seq_after={sequence_after} lock_ms={} total_ms={} bytes={}",
            locked_for.as_millis(),
            started.elapsed().as_millis(),
            raw.len()
        );
        if sequence_before != 0 && sequence_after != 0 && sequence_before != sequence_after {
            return Err(AppError::Clipboard(
                "Excel clipboard sequence changed during Unicode snapshot".to_owned(),
            ));
        }
        if raw.len() % 2 != 0 {
            return Err(AppError::Clipboard(
                "Excel Unicode clipboard has an odd byte length".to_owned(),
            ));
        }
        let mut utf16 = Vec::with_capacity(raw.len() / 2);
        for chunk in raw.chunks_exact(2) {
            let unit = u16::from_le_bytes([chunk[0], chunk[1]]);
            if unit == 0 {
                break;
            }
            utf16.push(unit);
        }
        if utf16.is_empty() {
            return Ok(None);
        }
        Ok(Some(TextPayload {
            text: String::from_utf16_lossy(&utf16),
            html: None,
            rtf: None,
        }))
    }

    /// 读取剪贴板中的文本族表示，包含纯文本、HTML 和 RTF。
    ///
    /// Open once and use raw getters; clipboard-rs::get nests OpenClipboard,
    /// ignores acquisition/read errors and can turn a failed read into Ok([]).
    #[cfg(target_os = "windows")]
    fn read_text_payload(&self, capture: &Capture) -> Result<Option<TextPayload>> {
        use clipboard_win::{formats, get};
        let _stage = NativeClipboardStage::begin("text_formats");
        let _access = super::lock_windows_capture()?;
        let _clipboard_lock = clipboard_win::Clipboard::new_attempts(10).map_err(|err| {
            AppError::Clipboard(format!("Windows clipboard text snapshot busy: {err}"))
        })?;

        let mut text = String::new();
        if clipboard_win::is_format_avail(formats::CF_UNICODETEXT) {
            log::info!("clipboard native_read request=CF_UNICODETEXT");
            text = get(formats::Unicode).map_err(clip_err)?;
        }
        // The current ingest/selection semantics require a non-empty plain
        // fallback even for HTML and RTF. Do not request delayed rich formats
        // when they cannot produce any selectable history item.
        if text.trim().is_empty() {
            return Ok(None);
        }

        let mut html = None;
        let mut rtf = None;
        for kind in capture.ordered_kinds() {
            if !capture.is_enabled(kind) {
                continue;
            }
            if super::windows_write_pending() {
                return if capture.text {
                    Ok(Some(TextPayload { text, html: None, rtf: None }))
                } else {
                    Err(AppError::Clipboard(
                        "Windows clipboard busy: EcoPaste write pending".to_owned(),
                    ))
                };
            }
            match kind {
                CaptureKind::Text => break,
                CaptureKind::Html => {
                    let format = formats::Html::new().ok_or_else(|| {
                        AppError::Clipboard("register Windows HTML format failed".to_owned())
                    })?;
                    if clipboard_win::is_format_avail(format.code()) {
                        log::info!("clipboard native_read request=HTML");
                        match get::<String, _>(format) {
                            Ok(value) => html = non_empty_string(value),
                            Err(err) => log::warn!("read optional clipboard HTML failed: {err}"),
                        }
                    }
                    // HTML is the highest usable format; RTF would add no
                    // value to the selected payload and could delay-render.
                    if html.as_ref().is_some_and(|value| !value.trim().is_empty()) {
                        break;
                    }
                }
                CaptureKind::Rtf => {
                    let format = clipboard_win::register_format("Rich Text Format").ok_or_else(|| {
                        AppError::Clipboard("register Windows RTF format failed".to_owned())
                    })?;
                    if clipboard_win::is_format_avail(format.get()) {
                        log::info!("clipboard native_read request=RTF");
                        match get::<Vec<u8>, _>(formats::RawData(format.get())) {
                            Ok(bytes) => {
                                rtf = non_empty_string(
                                    String::from_utf8_lossy(&bytes)
                                        .trim_end_matches('\0')
                                        .to_owned(),
                                );
                            }
                            Err(err) => log::warn!("read optional clipboard RTF failed: {err}"),
                        }
                    }
                    if rtf.as_ref().is_some_and(|value| !value.trim().is_empty()) {
                        break;
                    }
                }
                CaptureKind::Files | CaptureKind::Image => {}
            }
        }
        Ok(Some(TextPayload { text, html, rtf }))
    }

    #[cfg(not(target_os = "windows"))]
    fn read_text_payload(&self, capture: &Capture) -> Result<Option<TextPayload>> {
        let has_text = self.ctx.has(ContentFormat::Text);
        let has_html = capture.html && self.ctx.has(ContentFormat::Html);
        let has_rtf = capture.rtf && self.ctx.has(ContentFormat::Rtf);
        if !has_text && !has_html && !has_rtf {
            return Ok(None);
        }

        let text = if has_text {
            self.ctx.get_text().map_err(clip_err)?
        } else {
            String::new()
        };
        let html = read_optional(has_html, || self.ctx.get_html());
        let rtf = read_optional(has_rtf, || self.ctx.get_rich_text());

        if text.is_empty() && html.is_none() && rtf.is_none() {
            return Ok(None);
        }

        Ok(Some(TextPayload { text, html, rtf }))
    }

    /// 读取图片为 PNG 载荷。
    ///
    /// 快路径：剪贴板原生就带 PNG（浏览器复制图片等）时，直接取原始 PNG 字节，
    /// 宽高从 PNG 头（IHDR）解析——全程零图像解码/编码。
    ///
    /// 慢路径回退：源是 TIFF（macOS 截图/多数 App）/ DIB（Windows）等非 PNG 时，
    /// 交给 [`clipboard_rs`] 解码再编码为 PNG（存储格式恒为 PNG，这次转码无法避免）。
    fn read_image(&self) -> Result<Option<ImagePayload>> {
        #[cfg(target_os = "windows")]
        {
            let png_error = match self.read_windows_png() {
                Ok(Some(bytes)) => {
                    if let Some((width, height)) = png_dimensions(&bytes) {
                        return Ok(Some(ImagePayload { bytes, width, height }));
                    }
                    Some(AppError::Clipboard("Windows PNG header invalid".to_owned()))
                }
                Ok(None) => None,
                Err(error) => {
                    // A temporarily unreadable PNG must not hide a valid DIB.
                    log::info!("clipboard PNG unreadable; checking DIB fallback");
                    Some(error)
                }
            };
            // Only copy raw DIB bytes while the system clipboard is open.
            // Bitmap decoding and PNG encoding run after releasing both locks.
            let Some(bytes) = self.read_windows_dib()? else {
                return match png_error {
                    Some(err) => Err(err),
                    None => Ok(None),
                };
            };
            let decoder = image::codecs::bmp::BmpDecoder::new_without_file_header(
                std::io::Cursor::new(bytes.as_slice()),
            ).map_err(clip_err)?;
            let dynamic_image = image::DynamicImage::from_decoder(decoder).map_err(clip_err)?;
            let image = clipboard_rs::RustImageData::from_dynamic_image(dynamic_image);
            return image_payload_from_rust(image);
        }
        #[cfg(not(target_os = "windows"))]
        {
            if let Ok(bytes) = self.ctx.get_buffer(PNG_FORMAT) {
                if let Some((width, height)) = png_dimensions(&bytes) {
                    return Ok(Some(ImagePayload { bytes, width, height }));
                }
            }
            let image = self.ctx.get_image().map_err(clip_err)?;
            image_payload_from_rust(image)
        }
    }

    /// Registered PNG formats used by Windows and WeChat/Chromium.
    #[cfg(target_os = "windows")]
    fn windows_registered_png_available(&self) -> bool {
        ["PNG", "image/png"].iter().any(|name| {
            clipboard_win::register_format(name).is_some_and(|format| {
                clipboard_win::is_format_avail(format.get())
            })
        })
    }

    #[cfg(target_os = "windows")]
    fn read_windows_png(&self) -> Result<Option<Vec<u8>>> {
        let formats: Vec<u32> = ["PNG", "image/png"]
            .into_iter()
            .filter_map(clipboard_win::register_format)
            .map(|format| format.get())
            .filter(|format| clipboard_win::is_format_avail(*format))
            .collect();
        if formats.is_empty() {
            return Ok(None);
        }
        let _stage = NativeClipboardStage::begin("PNG_formats");
        let _access = super::lock_windows_capture()?;
        let _clip = WindowsClipboard::new().map_err(|err| {
            AppError::Clipboard(format!("Windows clipboard busy reading PNG: {err}"))
        })?;
        let mut last_error = None;
        for format in formats {
            if super::windows_write_pending() {
                return Err(AppError::Clipboard("Windows clipboard busy: EcoPaste write pending".to_owned()));
            }
            log::info!("clipboard native_read request=PNG format_id={format}");
            let mut data = Vec::new();
            match clipboard_raw::get_vec(format, &mut data) {
                Ok(_) if !data.is_empty() && png_dimensions(&data).is_some() => {
                    return Ok(Some(data));
                }
                Ok(_) if !data.is_empty() => {
                    last_error = Some(AppError::Clipboard(
                        "Windows PNG clipboard data has an invalid header".to_owned(),
                    ));
                }
                Ok(_) => {}
                Err(err) => {
                    last_error = Some(AppError::Clipboard(format!(
                        "Windows PNG clipboard data temporarily unavailable: {err}"
                    )));
                }
            }
        }
        if let Some(error) = last_error {
            return Err(error);
        }
        Ok(None)
    }

    #[cfg(target_os = "windows")]
    fn read_windows_dib(&self) -> Result<Option<Vec<u8>>> {
        use clipboard_win::formats::{CF_DIB, CF_DIBV5};
        let formats: Vec<u32> = [CF_DIBV5, CF_DIB]
            .into_iter()
            .filter(|format| clipboard_win::is_format_avail(*format))
            .collect();
        if formats.is_empty() {
            return Ok(None);
        }
        let _stage = NativeClipboardStage::begin("DIB_formats");
        let _access = super::lock_windows_capture()?;
        let _clip = WindowsClipboard::new().map_err(|err| {
            AppError::Clipboard(format!("Windows clipboard busy reading DIB: {err}"))
        })?;
        let mut last_error = None;
        for format in formats {
            if super::windows_write_pending() {
                return Err(AppError::Clipboard("Windows clipboard busy: EcoPaste write pending".to_owned()));
            }
            log::info!("clipboard native_read request=DIB format_id={format}");
            let mut data = Vec::new();
            match clipboard_raw::get_vec(format, &mut data) {
                Ok(_) if !data.is_empty() => return Ok(Some(data)),
                Ok(_) => {}
                Err(err) => {
                    last_error = Some(AppError::Clipboard(format!(
                        "Windows DIB clipboard data temporarily unavailable: {err}"
                    )));
                }
            }
        }
        if let Some(error) = last_error {
            return Err(error);
        }
        Ok(None)
    }
}

fn image_payload_from_rust(image: clipboard_rs::RustImageData) -> Result<Option<ImagePayload>> {
    let (width, height) = image.get_size();
    if width == 0 || height == 0 {
        return Ok(None);
    }
    let bytes = image.to_png().map_err(clip_err)?.get_bytes().to_vec();
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(ImagePayload { bytes, width, height }))
}

/// 仅检查已公布的 Excel 单元格格式，不发出数据读取请求。
/// BIFF 用于区分未就绪的单元格；实际 Excel owner 的 Unicode 优先路径不以其为门槛。
#[cfg(target_os = "windows")]
fn is_excel_cell_clipboard() -> bool {
    ["Biff8", "Biff12", "Biff5", "XML Spreadsheet"]
        .iter()
        .any(|name| {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let format = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
            format != 0 && unsafe { IsClipboardFormatAvailable(format) != 0 }
        })
}

/// 平台剪贴板里 PNG 的原始格式标识符，用于 `get_buffer` 直取原始字节。
#[cfg(target_os = "macos")]
const PNG_FORMAT: &str = "public.png";
#[cfg(target_os = "windows")]
const PNG_FORMAT: &str = "PNG";

/// 从 PNG 字节解析宽高（不解码像素）。
///
/// PNG 布局固定：8 字节签名 + 4 字节 IHDR 长度 + 4 字节 "IHDR" + 宽(大端 u32) + 高(大端 u32)。
/// 宽高位于偏移 16..24。校验签名与 IHDR 标记，任一不符返回 `None`（交回退路径处理）。
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

/// 仅当 `available` 时读取，读取失败或空串都归并为 `None`，
/// 让「格式存在但内容为空」与「格式不存在」对下游表现一致。
#[cfg(target_os = "windows")]
fn non_empty_string(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[cfg(not(target_os = "windows"))]
fn read_optional(
    available: bool,
    read: impl FnOnce() -> clipboard_rs::common::Result<String>,
) -> Option<String> {
    if !available {
        return None;
    }
    read().ok().filter(|value| !value.is_empty())
}

/// 判断文本载荷中是否实际带有指定文本族表示。
fn text_contains_kind(text: &TextPayload, kind: CaptureKind) -> bool {
    let has_plain = !text.text.trim().is_empty();

    match kind {
        CaptureKind::Text => has_plain,
        CaptureKind::Html => {
            has_plain
                && text
                    .html
                    .as_ref()
                    .is_some_and(|html| !html.trim().is_empty())
        }
        CaptureKind::Rtf => {
            has_plain && text.rtf.as_ref().is_some_and(|rtf| !rtf.trim().is_empty())
        }
        CaptureKind::Files | CaptureKind::Image => false,
    }
}

fn clip_err<E: std::fmt::Display>(err: E) -> AppError {
    AppError::Clipboard(err.to_string())
}

