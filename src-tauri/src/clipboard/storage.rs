//! 图片落盘：按内容哈希分片存储原图与缩略图。
//!
//! 目录布局（`content` 字段存文件名 `<hash>.png`，分片目录由此函数推导，不入库）：
//! ```text
//! <app_local_data>/resources/clipboard-images/
//!   origin/<hash[..2]>/<hash>.png       原图（PNG），复制时落盘
//!   thumbnails/<hash[..2]>/<hash>.png   缩略图（PNG，最长边 <= THUMBNAIL_MAX），首次预览时按需生成
//! ```
//! 文件名取「PNG 字节的 blake3」：同一张图重复复制 → 同字节 → 同文件名，落盘幂等，
//! 且与去重指纹同源（image 的 `content_hash` 即对 PNG 字节哈希）。
//! 按 hash 前 2 位 hex 分 256 个子目录，避免重度使用下单目录文件爆量。
//!
//! 缩略图的解码/缩放/编码不在复制热路径上——`store` 只写原图，缩略图由
//! [`ImageStore::ensure_thumbnail`] 在前端首次取图时懒生成并缓存。

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Context;
use blake3::Hasher;
use clipboard_rs::common::{RustImage, RustImageData};
use tauri::AppHandle;

use super::payload::ImagePayload;
use crate::core::{AppError, Result};

/// 缩略图最长边像素。仅用于列表预览，够清晰即可。
const THUMBNAIL_MAX: u32 = 300;

/// 剪贴板图片目录名，挂在 `core::paths::resources_dir` 下（与 `app-icons` 并列）。
const IMAGES_DIR: &str = "clipboard-images";
const ORIGIN_DIR: &str = "origin";
const THUMBNAILS_DIR: &str = "thumbnails";

/// 一次图片落盘的结果，交给 ingest 写入 `ClipboardItem`。
pub struct StoredImage {
    /// 入库 `content`：图片文件名 `<hash>.png`（不含分片目录）。
    pub file_name: String,
    /// 去重指纹来源：PNG 字节的 blake3（十六进制）。
    #[allow(dead_code)]
    pub content_digest: String,
    pub width: i64,
    pub height: i64,
    /// 原图字节数。
    pub size: i64,
}

/// 图片存储器：持有 app data 下的 `resources/clipboard-images` 根目录。
/// 放入 Tauri `State`，监听线程与命令共用。
#[derive(Clone)]
pub struct ImageStore {
    images_root: Arc<RwLock<PathBuf>>,
}

impl ImageStore {
    /// 从 `AppHandle` 解析 `<app_local_data>/resources/clipboard-images` 作为根。
    pub fn new(app: &AppHandle) -> Result<Self> {
        let images_root = crate::core::paths::resources_dir(app)?.join(IMAGES_DIR);
        Ok(Self {
            images_root: Arc::new(RwLock::new(images_root)),
        })
    }


    /// 重新绑定到当前真实数据根；数据目录热迁移后由存储命令调用。
    pub fn rebase(&self, app: &AppHandle) -> Result<()> {
        let next = crate::core::paths::resources_dir(app)?.join(IMAGES_DIR);
        *self
            .images_root
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
        Ok(())
    }

    /// 落盘原图，返回 [`StoredImage`]。已存在的文件跳过写入（幂等）。
    ///
    /// 缩略图**不在此生成**——它已移出复制热路径，改由 [`Self::ensure_thumbnail`] 在
    /// 前端首次预览时按需生成并缓存。复制路径只做「哈希 + 写原图」，避免大图编解码卡顿。
    pub fn store(&self, image: &ImagePayload) -> Result<StoredImage> {
        let content_digest = blake3_hex(&image.bytes);
        let file_name = format!("{content_digest}.png");

        let origin_path = self.shard_path(ORIGIN_DIR, &content_digest, &file_name);
        write_if_absent(&origin_path, &image.bytes)?;

        Ok(StoredImage {
            file_name,
            content_digest,
            width: i64::from(image.width),
            height: i64::from(image.height),
            size: image.bytes.len() as i64,
        })
    }

    /// 确保缩略图存在并返回其绝对路径：已存在直接返回；否则读原图 → 解码 → 缩放 → 编码 PNG → 落盘。
    ///
    /// 供 `get_clipboard_image_path(thumbnail=true)` 调用。把生成放在「读」而非「写」侧，
    /// 既将解码/编码移出复制热路径，又因「返回前文件已确保存在」天然避免前端加载到半成品文件。
    pub fn ensure_thumbnail(&self, file_name: &str) -> Result<PathBuf> {
        let thumb_path = self.thumbnail_path(file_name);
        if thumb_path.exists() {
            return Ok(thumb_path);
        }

        let origin_path = self.origin_path(file_name);
        let origin_bytes = std::fs::read(&origin_path)
            .with_context(|| format!("failed to read origin image {origin_path:?}"))?;
        let thumb_bytes = encode_thumbnail(&origin_bytes)?;
        write_if_absent(&thumb_path, &thumb_bytes)?;
        Ok(thumb_path)
    }

    /// 删除一张图片的原图与缩略图。缩略图懒生成、可能不存在，缺失文件视作成功（幂等）。
    /// 删后顺手清理变空的分片目录（`origin/<ab>`、`thumbnails/<ab>`）。
    ///
    /// 调用前提：库里该图至多一行（image 去重指纹源自 PNG 字节，落盘文件名即字节哈希），
    /// 故删行后该文件必为孤儿，可直接删，无需引用计数。其余 IO 错误上抛由调用方记日志。
    pub fn remove(&self, file_name: &str) -> Result<()> {
        let origin = self.origin_path(file_name);
        let thumb = self.thumbnail_path(file_name);
        remove_if_present(&origin)?;
        remove_if_present(&thumb)?;
        // 分片目录可能被同前缀的其他图共享，非空时保留——remove_dir 只删空目录，
        // 非空 / 不存在都返回 Err，一并忽略；目录清理是尽力而为，不影响删图结果。
        remove_dir_if_empty(origin.parent());
        remove_dir_if_empty(thumb.parent());
        Ok(())
    }

    /// 由文件名解析原图绝对路径（分片目录从文件名前 2 位推导）。供写回/粘贴使用。
    pub fn origin_path(&self, file_name: &str) -> PathBuf {
        self.shard_path(ORIGIN_DIR, shard_key(file_name), file_name)
    }

    /// 由文件名解析缩略图绝对路径。供前端预览取图。
    pub fn thumbnail_path(&self, file_name: &str) -> PathBuf {
        self.shard_path(THUMBNAILS_DIR, shard_key(file_name), file_name)
    }

    fn shard_path(&self, kind_dir: &str, shard_src: &str, file_name: &str) -> PathBuf {
        self.images_root()
            .join(kind_dir)
            .join(shard_dir(shard_src))
            .join(file_name)
    }

    fn images_root(&self) -> PathBuf {
        self.images_root
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// 分片子目录名：取来源串前 2 个字符（hash 恒为 hex，必有 2 位）；
/// 异常短串兜底为 `00`，保证始终有一层分片。
fn shard_dir(src: &str) -> &str {
    if src.len() >= 2 {
        &src[..2]
    } else {
        "00"
    }
}

/// 从文件名 `<hash>.png` 取分片来源（即 hash 本身）。
fn shard_key(file_name: &str) -> &str {
    file_name.split('.').next().unwrap_or(file_name)
}

fn blake3_hex(bytes: &[u8]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(bytes);
    hasher.finalize().to_hex().to_string()
}

/// 写文件（自动建分片目录）；目标已存在则跳过，保证幂等且不重复 IO。
fn write_if_absent(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create image dir {parent:?}"))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("failed to write image {path:?}"))?;
    Ok(())
}

/// 删文件；文件不存在时静默成功（幂等），其余 IO 错误上抛。
fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::from(
            anyhow::Error::new(err).context(format!("failed to remove image {path:?}")),
        )),
    }
}

/// 尽力删除空目录：`remove_dir` 仅在目录为空时成功，非空（仍有同分片的其他图）/
/// 不存在都返回 `Err`，一律忽略——目录清理不影响删图结果，无需上抛。
fn remove_dir_if_empty(dir: Option<&Path>) {
    if let Some(dir) = dir {
        let _ = std::fs::remove_dir(dir);
    }
}

/// 把原图 PNG 字节解码 → 生成缩略图（最长边 <= [`THUMBNAIL_MAX`]，保持比例）→ 重新编码 PNG。
fn encode_thumbnail(png_bytes: &[u8]) -> Result<Vec<u8>> {
    let image = RustImageData::from_bytes(png_bytes).map_err(clip_err)?;
    let thumb = image
        .thumbnail(THUMBNAIL_MAX, THUMBNAIL_MAX)
        .map_err(clip_err)?;
    Ok(thumb.to_png().map_err(clip_err)?.get_bytes().to_vec())
}

fn clip_err<E: std::fmt::Display>(err: E) -> AppError {
    AppError::Clipboard(err.to_string())
}

