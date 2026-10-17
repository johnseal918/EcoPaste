//! 把读取到的 [`ClipboardPayload`] 转换为可入库的 [`ClipboardItem`]：
//! 编排子类型识别（[`super::detect`]）与图片落盘（[`super::storage`]）。
//!
//! 文本存储语义（按用户采集顺序在 HTML / RTF / plain 之间择一）：
//! - `content` 存**源表示**（HTML/RTF 原文、或纯文本），供前端渲染与写回；
//! - `search_text` 存**纯文本**（HTML 去标签 / RTF 用 OS 提供的 plain 文本），供 FTS 检索与纯文本粘贴；
//! - 纯文本无富文本时 `sub_kind` 走 url/email/color/path 识别。
//!
//! 图片：落盘原图 + 缩略图，`content` 存文件名 `<sha256>.png`，`content_hash` 仍走
//! [`content_hash(Image, file_name)`]，而 `file_name` 源自 PNG 字节哈希 → 去重对字节敏感。

use chrono::Utc;

use super::detect::detect_text_sub_kind;
use super::payload::{ClipboardPayload, TextPayload};
use super::secrets::contains_secret;
use super::storage::ImageStore;
use crate::core::Result;
use crate::db::items::content_hash;
use crate::db::models::{ClipboardItem, ClipboardKind, ClipboardSubKind, Platform};
use crate::settings::{Capture, CaptureKind, Sensitive};

/// 列表渲染用摘要的最大字符数（按 Unicode 标量计，不是字节）。
/// 超过此长度的文本会被截断，前端列表只渲染摘要，预览/写回时再读完整 `content`。
pub const SUMMARY_MAX_CHARS: usize = 256;

/// 从纯文本生成列表摘要：trim 后按 [`SUMMARY_MAX_CHARS`] 字符截断。
/// 输入空串返回 `None`。HTML/RTF 也用这个，输入是 OS 同时提供的纯文本，不解析富文本。
fn make_summary(plain: &str) -> Option<String> {
    let trimmed = plain.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(SUMMARY_MAX_CHARS).collect())
}

/// 当前平台标记。仅 macOS / Windows 双平台（见 AGENTS.md），其余 target 不应被编译进来。
fn current_platform() -> Platform {
    #[cfg(target_os = "macos")]
    {
        Platform::Macos
    }
    #[cfg(target_os = "windows")]
    {
        Platform::Windows
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        compile_error!("EcoPaste only supports macOS and Windows")
    }
}

/// 转换中间产物：决定一条记录的核心字段，其余字段由 [`build_item`] 补齐。
struct Draft {
    kind: ClipboardKind,
    sub_kind: Option<ClipboardSubKind>,
    content: String,
    search_text: Option<String>,
    summary: Option<String>,
    file_types: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    size: Option<i64>,
}

/// files 列表入库时的 `content`：换行连接的路径串（与去重哈希、FTS 检索单一来源）。
fn files_to_content(files: &[String]) -> String {
    files.join("\n")
}

/// 文本载荷 → 草稿。优先级来自用户配置：
/// - html：`content` = HTML 源（`get_html()` 原文，前端 DOMPurify 后渲染），`sub_kind` = Html；
/// - rtf：`content` = RTF 源（`get_rich_text()` 原文），`sub_kind` = Rtf；
/// - 两者的 `search_text` 都直接用 OS 同时提供的纯文本（`get_text()`）——
///   复制富文本时剪贴板本就并存纯文本表示，无需自己解析 HTML/RTF；
/// - plain：被顺序选中时，`content` = 纯文本，`sub_kind` = url/email/color/path 识别，
///   `search_text` 与 content 同串（统一由 FTS 索引 search_text）。
///
/// 一律以 trim 后的纯文本作为「是否有可展示内容」的判据：纯文本为空就直接 `None`，
/// 不管 HTML/RTF 源是否存在（只有样式/空白节点的源对用户没意义，列表也渲染不出来）。
fn draft_from_text(text: &TextPayload, capture: &Capture, plain_only: bool) -> Option<Draft> {
    if !capture.text && !capture.html && !capture.rtf {
        return None;
    }

    let plain = text.text.trim();
    if plain.is_empty() {
        return None;
    }

    let plain_search = Some(plain.to_owned());
    let summary = make_summary(plain);

    let html = non_empty(&text.html);
    let rtf = non_empty(&text.rtf);

    if !plain_only {
        for kind in capture.ordered_kinds() {
            if !capture.is_enabled(kind) {
                continue;
            }

            match kind {
                CaptureKind::Html => {
                    if let Some(html) = html {
                        return Some(Draft {
                            kind: ClipboardKind::Text,
                            sub_kind: Some(ClipboardSubKind::Html),
                            content: html.clone(),
                            search_text: plain_search.clone(),
                            summary: summary.clone(),
                            file_types: None,
                            width: None,
                            height: None,
                            size: Some(count_text_bytes(html)),
                        });
                    }
                }
                CaptureKind::Rtf => {
                    if let Some(rtf) = rtf {
                        return Some(Draft {
                            kind: ClipboardKind::Text,
                            sub_kind: Some(ClipboardSubKind::Rtf),
                            content: rtf.clone(),
                            search_text: plain_search.clone(),
                            summary: summary.clone(),
                            file_types: None,
                            width: None,
                            height: None,
                            size: Some(count_text_bytes(rtf)),
                        });
                    }
                }
                CaptureKind::Text => {
                    return Some(draft_plain_text(plain, plain_search, summary));
                }
                CaptureKind::Files | CaptureKind::Image => {}
            }
        }

        return None;
    }

    if !capture.text {
        return None;
    }

    Some(draft_plain_text(plain, plain_search, summary))
}

/// 根据纯文本表示生成文本草稿，并执行 URL / 邮箱 / 色值 / 路径子类型识别。
fn draft_plain_text(plain: &str, plain_search: Option<String>, summary: Option<String>) -> Draft {
    Draft {
        kind: ClipboardKind::Text,
        sub_kind: detect_text_sub_kind(plain),
        content: plain.to_owned(),
        search_text: plain_search,
        summary,
        file_types: None,
        width: None,
        height: None,
        size: Some(count_text_bytes(plain)),
    }
}

fn non_empty(value: &Option<String>) -> Option<&String> {
    value.as_ref().filter(|s| !s.trim().is_empty())
}

/// 文本 size 统一按最终入库内容的 UTF-8 字节数计算。
fn count_text_bytes(text: &str) -> i64 {
    text.len() as i64
}

/// 判断字节数是否超过 MB 设置换算出的限制；`None` 表示不限。
fn exceeds_limit(size: usize, limit: Option<u64>) -> bool {
    limit.is_some_and(|limit| size as u64 > limit)
}


/// 把载荷转换为待入库记录，同时应用内容类型与隐私过滤设置。
pub fn build_item_with_settings(
    store: &ImageStore,
    payload: &ClipboardPayload,
    capture: &Capture,
    sensitive: &Sensitive,
    plain_only: bool,
) -> Result<Option<ClipboardItem>> {
    let mut is_sensitive = false;
    let draft = match payload {
        ClipboardPayload::Text(text) => {
            if contains_secret(&text.text) {
                if !sensitive.collect_secrets {
                    return Ok(None);
                }

                is_sensitive = true;
            }

            draft_from_text(text, capture, plain_only)
        }
        ClipboardPayload::Files(files) => {
            if !capture.files {
                return Ok(None);
            }

            let content = files_to_content(files);
            if content.trim().is_empty() {
                None
            } else {
                // 记录每个路径的类型：d=directory, f=file
                let file_types: Vec<&str> = files
                    .iter()
                    .map(|p| {
                        if std::path::Path::new(p).is_dir() {
                            "d"
                        } else {
                            "f"
                        }
                    })
                    .collect();
                let file_types_str = file_types.join(",");

                // 文件名写入 search_text 供 FTS 命中；列表名由命令层从 content 路径现算，summary 保持为空。
                let basenames = files
                    .iter()
                    .map(|p| {
                        std::path::Path::new(p)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(p.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                Some(Draft {
                    kind: ClipboardKind::Files,
                    sub_kind: None,
                    content,
                    search_text: Some(basenames),
                    summary: None,
                    file_types: Some(file_types_str),
                    width: None,
                    height: None,
                    size: None,
                })
            }
        }
        ClipboardPayload::Image(image) => {
            if !capture.image {
                return Ok(None);
            }
            if exceeds_limit(image.bytes.len(), capture.max_image_bytes()) {
                log::info!(
                    "clipboard image skipped because size {} exceeds limit {:?}",
                    image.bytes.len(),
                    capture.max_image_bytes()
                );
                return Ok(None);
            }

            let stored = store.store(image)?;
            Some(Draft {
                kind: ClipboardKind::Image,
                sub_kind: None,
                content: stored.file_name,
                search_text: None,
                summary: None,
                file_types: None,
                width: Some(stored.width),
                height: Some(stored.height),
                size: Some(stored.size),
            })
        }
    };

    let Some(draft) = draft else {
        return Ok(None);
    };
    if draft.kind == ClipboardKind::Text
        && exceeds_limit(draft.content.len(), capture.max_text_bytes())
    {
        log::info!(
            "clipboard text skipped because size {} exceeds limit {:?}",
            draft.content.len(),
            capture.max_text_bytes()
        );
        return Ok(None);
    }

    let now = Utc::now();
    Ok(Some(ClipboardItem {
        id: uuid::Uuid::new_v4().to_string(),
        content_hash: content_hash(draft.kind, &draft.content),
        kind: draft.kind,
        sub_kind: draft.sub_kind,
        group_id: None,
        source_app_id: None,
        content: draft.content,
        search_text: draft.search_text,
        summary: draft.summary,
        file_types: draft.file_types,
        size: draft.size,
        width: draft.width,
        height: draft.height,
        use_count: 1,
        is_favorite: false,
        is_pinned: false,
        priority_order: None,
        pin_order: None,
        is_sensitive,
        platform: current_platform(),
        note: None,
        created_at: now,
        updated_at: now,
        source_app_name: None,
        source_app_icon_file: None,
        source_app_icon_path: None,
        image_thumbnail_path: None,
        file_entries: None,
        files_preview_kind: None,
        available_actions: Vec::new(),
        color_preview: None,
        display_created_at: String::new(),
    }))
}

