use std::collections::HashMap;

use anyhow::Context;
use blake3::Hasher;
use chrono::Utc;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::sync::Mutex;

use crate::core::{AppError, Result};
use crate::db::models::{
    ClipboardGroupFilter, ClipboardItem, ClipboardItemQuery, ClipboardItemSort, ClipboardKind,
};

const SELECT_ITEM: &str = "SELECT id, kind, sub_kind, group_id, source_app_id, content, \
     content_hash, search_text, summary, file_types, size, width, height, use_count, is_favorite, is_pinned, \
     priority_order, pin_order, is_sensitive, platform, note, created_at, updated_at FROM clipboard_items";

/// 列表/单条刷新场景的精简 SELECT：text 类型条目的 `content` 与 `search_text` 一律置空，
/// 由前端用 `summary` 渲染。HTML/RTF/长纯文本可能很大（用户复制整段文档），
/// 整段过 IPC + 进 DOM 是这条链路最昂贵的一环；image/files 的 content 是
/// 文件名 / 路径列表，保留原值。预览/写回走 [`find_item_by_id`] 拿完整 content。
///
/// LEFT JOIN `clipboard_apps` 顺带把来源应用名 / 图标文件名带回，前端直接渲染，
/// 不再额外发 list_clipboard_apps + get_clipboard_app_icon_path 请求。
const LIST_SELECT_ITEM: &str = "SELECT clipboard_items.id, clipboard_items.kind, \
     clipboard_items.sub_kind, clipboard_items.group_id, clipboard_items.source_app_id, \
     CASE WHEN clipboard_items.kind = 'text' THEN '' ELSE clipboard_items.content END AS content, \
     clipboard_items.content_hash, \
     CASE WHEN clipboard_items.kind = 'text' THEN NULL ELSE clipboard_items.search_text END AS search_text, \
     clipboard_items.summary, clipboard_items.file_types, clipboard_items.size, \
     clipboard_items.width, clipboard_items.height, clipboard_items.use_count, \
     clipboard_items.is_favorite, clipboard_items.is_pinned, \
     clipboard_items.priority_order, clipboard_items.pin_order, \
     clipboard_items.is_sensitive, \
     clipboard_items.platform, clipboard_items.note, \
     clipboard_items.created_at, clipboard_items.updated_at, \
     clipboard_apps.name AS source_app_name, \
     clipboard_apps.icon_file AS source_app_icon_file \
     FROM clipboard_items \
     LEFT JOIN clipboard_apps ON clipboard_apps.id = clipboard_items.source_app_id";

/// 入库去重的结果：`id` 为生效行的主键（命中时是已有行，未命中时是新插入行），
/// `deduplicated` 表示是否命中了已有内容（命中则只 `use_count + 1` 未插入新行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertResult {
    pub id: String,
    pub deduplicated: bool,
}

/// 计算去重指纹：`blake3("<kind>:<content>")`。
/// 加 `kind` 前缀，避免 text 与 files 恰好同串内容被误判为重复。
/// text 直接哈希内容串即可；image/files 的 `content` 是落盘引用/路径，
/// 调用方持有原始字节时可改为对原始内容字节哈希后写入 `content_hash`。
pub fn content_hash(kind: ClipboardKind, content: &str) -> String {
    let mut hasher = Hasher::new();
    hasher.update(kind_tag(kind).as_bytes());
    hasher.update(b":");
    hasher.update(content.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn kind_tag(kind: ClipboardKind) -> &'static str {
    match kind {
        ClipboardKind::Text => "text",
        ClipboardKind::Image => "image",
        ClipboardKind::Files => "files",
    }
}

/// Serialize check + insert: the capture watcher can enqueue multiple async
/// persistence tasks from consecutive copies. Without this gate, two tasks
/// can both SELECT "absent" and INSERT duplicate cards with different IDs.
/// All writes in one EcoPaste process use this shared gate.
static UPSERT_GATE: Mutex<()> = Mutex::const_new(());

/// 入库主入口：图片/文件按原始内容指纹去重；文本额外比较真正展示的完整纯文本。
/// 同样的可见文本可能分别来自 HTML/RTF/plain，原始格式字节不同，
/// 但历史卡片不应因此出现多条。禁止用截断的 summary 进行去重。
pub async fn upsert_item(pool: &SqlitePool, item: &ClipboardItem) -> Result<UpsertResult> {
    let _serial = UPSERT_GATE.lock().await;
    if let Some(existing) = find_item_by_content_hash(pool, &item.content_hash).await? {
        increment_item_use_count(pool, &existing.id).await?;
        return Ok(UpsertResult {
            id: existing.id,
            deduplicated: true,
        });
    }

    if item.kind == ClipboardKind::Text {
        if let Some(plain) = item.search_text.as_deref().filter(|s| !s.is_empty()) {
            // FULL search_text, never short summary: false matches would destroy
            // distinct copied paragraphs with identical first 256 characters.
            let existing_id = sqlx::query_scalar::<_, String>(
                "SELECT id FROM clipboard_items \
                 WHERE kind = 'text' AND search_text = ? \
                 ORDER BY is_pinned DESC, is_favorite DESC, updated_at DESC, id ASC LIMIT 1",
            )
            .bind(plain)
            .fetch_optional(pool)
            .await
            .context("failed to find same visible text")?;
            if let Some(existing_id) = existing_id {
                increment_item_use_count(pool, &existing_id).await?;
                return Ok(UpsertResult {
                    id: existing_id,
                    deduplicated: true,
                });
            }
        }
    }

    insert_item(pool, item).await?;
    Ok(UpsertResult {
        id: item.id.clone(),
        deduplicated: false,
    })
}

/// One-time-safe reconciliation for pre-existing duplicates from the old
/// rich-format/raw-hash policy or concurrent asynchronous writes. Only
/// untouched ordinary history with the same full text AND same group is
/// eligible; favorite, pinned, manually ordered, annotated and sensitive
/// entries are never deleted automatically. Preserve total use_count and
/// the newest representative's full-format content.
pub async fn consolidate_safe_text_duplicates(pool: &SqlitePool) -> Result<u64> {
    let _serial = UPSERT_GATE.lock().await;
    let mut tx = pool.begin().await.context("begin text dedup transaction")?;
    let rows: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT id, search_text, group_id, use_count FROM clipboard_items \
         WHERE kind = 'text' AND search_text IS NOT NULL AND search_text <> '' \
           AND is_favorite = 0 AND is_pinned = 0 \
           AND priority_order IS NULL AND pin_order IS NULL \
           AND note IS NULL AND is_sensitive = 0 AND is_manually_edited = 0 \
         ORDER BY search_text ASC, group_id ASC, updated_at DESC, created_at DESC, id ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .context("load ordinary duplicate candidates")?;

    let mut keepers: HashMap<(String, Option<String>), String> = HashMap::new();
    let mut removed = 0u64;
    for (id, visible_text, group_id, use_count) in rows {
        let key = (visible_text, group_id);
        if let Some(keep_id) = keepers.get(&key) {
            sqlx::query("UPDATE clipboard_items SET use_count = use_count + ? WHERE id = ?")
                .bind(use_count)
                .bind(keep_id)
                .execute(&mut *tx)
                .await
                .context("merge duplicate clipboard use count")?;
            sqlx::query("DELETE FROM clipboard_items WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await
                .context("remove safe ordinary text duplicate")?;
            removed += 1;
        } else {
            keepers.insert(key, id);
        }
    }
    tx.commit().await.context("commit text dedup transaction")?;
    Ok(removed)
}

/// 按 `content_hash` 查最近一条同内容记录（命中 `idx_clipboard_items_content_hash` 索引）。
pub async fn find_item_by_content_hash(
    pool: &SqlitePool,
    hash: &str,
) -> Result<Option<ClipboardItem>> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(SELECT_ITEM);
    qb.push(" WHERE content_hash = ")
        .push_bind(hash.to_owned())
        .push(" ORDER BY created_at DESC LIMIT 1");

    let item = qb
        .build_query_as::<ClipboardItem>()
        .fetch_optional(pool)
        .await
        .context("failed to find clipboard item by content_hash")?;
    Ok(item)
}

/// 插入一条剪贴板记录（不做去重；去重请走 [`upsert_item`]）。
pub async fn insert_item(pool: &SqlitePool, item: &ClipboardItem) -> Result<()> {
    sqlx::query(
        "INSERT INTO clipboard_items \
         (id, kind, sub_kind, group_id, source_app_id, content, content_hash, search_text, \
          summary, file_types, size, width, height, use_count, is_favorite, is_pinned, priority_order, pin_order, is_sensitive, platform, note, \
          created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(item.id.as_str())
    .bind(item.kind)
    .bind(item.sub_kind)
    .bind(item.group_id.as_deref())
    .bind(item.source_app_id.as_deref())
    .bind(item.content.as_str())
    .bind(item.content_hash.as_str())
    .bind(item.search_text.as_deref())
    .bind(item.summary.as_deref())
    .bind(item.file_types.as_deref())
    .bind(item.size)
    .bind(item.width)
    .bind(item.height)
    .bind(item.use_count)
    .bind(item.is_favorite)
    .bind(item.is_pinned)
    .bind(item.priority_order)
    .bind(item.pin_order)
    .bind(item.is_sensitive)
    .bind(item.platform)
    .bind(item.note.as_deref())
    .bind(item.created_at)
    .bind(item.updated_at)
    .execute(pool)
    .await
    .context("failed to insert clipboard item")?;
    Ok(())
}


/// 列表 + 总数一次返回，供命令层组装 [`ClipboardItemPage`]：
/// 一次 IPC 拿到「本页项 / 当前过滤下的总数 / 是否还有下一页」。
/// `keyword` 按字符长度分流：≥3 走 FTS5（trigram 分词），1–2 走 `LIKE '%kw%'`
/// （兜底短词；trigram 索引最短 3 字符，对 1–2 字符词永远 0 命中）。
pub async fn query_items_page(
    pool: &SqlitePool,
    q: &ClipboardItemQuery,
) -> Result<(Vec<ClipboardItem>, i64)> {
    let keyword = KeywordFilter::from_keyword(q.keyword.as_deref());
    let items = fetch_items(pool, q, keyword.clone()).await?;
    let total = fetch_items_count(pool, q, keyword).await?;
    Ok((items, total))
}

/// 按 `id` 查找单条记录，不存在时返回 `None`。
pub async fn find_item_by_id(pool: &SqlitePool, id: &str) -> Result<Option<ClipboardItem>> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(SELECT_ITEM);
    qb.push(" WHERE id = ").push_bind(id.to_owned());

    let item = qb
        .build_query_as::<ClipboardItem>()
        .fetch_optional(pool)
        .await
        .context("failed to find clipboard item by id")?;
    Ok(item)
}

/// 按 `id` 查找单条记录的「列表视图」副本——与 [`fetch_items`] 走同款 [`LIST_SELECT_ITEM`] 裁剪：
/// text 类型条目的 `content` / `search_text` 一律置空，由前端用 `summary` 渲染。
/// 供前端响应 `clipboard://updated` 事件时按 id 拉取使用，避免事件驱动刷新整页 refetch
/// 时回传整段 HTML/RTF。需要完整 `content` 的写回 / 预览路径请走 [`find_item_by_id`]。
pub async fn find_item_for_list_by_id(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<ClipboardItem>> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(LIST_SELECT_ITEM);
    qb.push(" WHERE clipboard_items.id = ")
        .push_bind(id.to_owned());

    let item = qb
        .build_query_as::<ClipboardItem>()
        .fetch_optional(pool)
        .await
        .context("failed to find clipboard item (list view) by id")?;
    Ok(item)
}

/// 翻转 `is_favorite`（收藏 / 取消收藏），返回翻转后的新状态。
pub async fn toggle_item_favorite(pool: &SqlitePool, id: &str) -> Result<bool> {
    let new_value: bool = sqlx::query_scalar(
        "UPDATE clipboard_items SET is_favorite = NOT is_favorite WHERE id = ? RETURNING is_favorite",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .context("failed to toggle clipboard item favorite")?;
    Ok(new_value)
}

/// 幂等地将 `is_favorite` 置为 true（已收藏的无变化）。auto-favorite 场景用。
pub async fn mark_item_favorite(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("UPDATE clipboard_items SET is_favorite = 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .context("failed to mark clipboard item favorite")?;
    Ok(())
}

/// 翻转置顶态并维护独立的置顶顺序。置顶时追加到末尾并清除普通排序；
/// 取消置顶时清除 pin_order，随后压紧剩余置顶顺序。
pub async fn toggle_item_pinned(pool: &SqlitePool, id: &str) -> Result<bool> {
    let mut tx = pool
        .begin()
        .await
        .context("failed to begin pin transaction")?;
    let current: bool = sqlx::query_scalar("SELECT is_pinned FROM clipboard_items WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .context("failed to read clipboard item pinned state")?;

    if current {
        sqlx::query("UPDATE clipboard_items SET is_pinned = 0, pin_order = NULL WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("failed to unpin clipboard item")?;
        compact_order_tx(&mut tx, OrderKind::Pinned).await?;
    } else {
        let next: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(pin_order), 0) + 1 FROM clipboard_items WHERE is_pinned = 1",
        )
        .fetch_one(&mut *tx)
        .await
        .context("failed to calculate next pin order")?;
        sqlx::query(
            "UPDATE clipboard_items SET is_pinned = 1, priority_order = NULL, pin_order = ? WHERE id = ?",
        )
        .bind(next)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("failed to pin clipboard item")?;
        compact_order_tx(&mut tx, OrderKind::Priority).await?;
    }

    tx.commit()
        .await
        .context("failed to commit pin transaction")?;
    Ok(!current)
}

/// 把普通历史加入手动排序末尾。已排序时保持原位置；置顶项不可加入普通排序。
pub async fn add_item_priority(pool: &SqlitePool, id: &str) -> Result<i64> {
    let mut tx = pool
        .begin()
        .await
        .context("failed to begin priority transaction")?;
    let row: (bool, Option<i64>) =
        sqlx::query_as("SELECT is_pinned, priority_order FROM clipboard_items WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .context("failed to read clipboard item priority state")?;
    if row.0 {
        return Err(anyhow::anyhow!("pinned item cannot join normal manual order").into());
    }
    if let Some(order) = row.1 {
        tx.commit()
            .await
            .context("failed to commit priority transaction")?;
        return Ok(order);
    }

    let next: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(priority_order), 0) + 1 FROM clipboard_items WHERE is_pinned = 0",
    )
    .fetch_one(&mut *tx)
    .await
    .context("failed to calculate next priority order")?;
    sqlx::query("UPDATE clipboard_items SET priority_order = ? WHERE id = ?")
        .bind(next)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("failed to add clipboard item priority")?;
    tx.commit()
        .await
        .context("failed to commit priority transaction")?;
    Ok(next)
}

/// 取消普通手动排序，并压紧剩余序号。
pub async fn cancel_item_priority(pool: &SqlitePool, id: &str) -> Result<()> {
    let mut tx = pool
        .begin()
        .await
        .context("failed to begin priority transaction")?;
    sqlx::query("UPDATE clipboard_items SET priority_order = NULL WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("failed to cancel clipboard item priority")?;
    compact_order_tx(&mut tx, OrderKind::Priority).await?;
    tx.commit()
        .await
        .context("failed to commit priority transaction")?;
    Ok(())
}

/// 移动普通手动排序项。position 小于 1 归一到 1，超出范围归一到末尾。
pub async fn move_item_priority(pool: &SqlitePool, id: &str, position: i64) -> Result<i64> {
    move_ordered_item(pool, id, position, OrderKind::Priority).await
}

/// 移动置顶项。position 小于 1 归一到 1，超出范围归一到末尾。
pub async fn move_pinned_item(pool: &SqlitePool, id: &str, position: i64) -> Result<i64> {
    move_ordered_item(pool, id, position, OrderKind::Pinned).await
}

#[derive(Clone, Copy)]
enum OrderKind {
    Priority,
    Pinned,
}

async fn move_ordered_item(
    pool: &SqlitePool,
    id: &str,
    position: i64,
    kind: OrderKind,
) -> Result<i64> {
    let mut tx = pool
        .begin()
        .await
        .context("failed to begin reorder transaction")?;
    let ids = ordered_ids_tx(&mut tx, kind).await?;
    let Some(current_index) = ids.iter().position(|current| current == id) else {
        return Err(anyhow::anyhow!("clipboard item is not in this ordered list").into());
    };

    let mut reordered = ids;
    let item_id = reordered.remove(current_index);
    let target = position.max(1).min((reordered.len() + 1) as i64) as usize - 1;
    reordered.insert(target, item_id);
    write_order_tx(&mut tx, kind, &reordered).await?;
    tx.commit()
        .await
        .context("failed to commit reorder transaction")?;
    Ok((target + 1) as i64)
}

async fn compact_order_tx(tx: &mut sqlx::Transaction<'_, Sqlite>, kind: OrderKind) -> Result<()> {
    let ids = ordered_ids_tx(tx, kind).await?;
    write_order_tx(tx, kind, &ids).await
}

/// 压紧普通手动排序与置顶排序到连续的 1..N。
/// 备份合并等批量写入完成后调用，避免来源序号冲突或跳号。
pub async fn normalize_item_orders(pool: &SqlitePool) -> Result<()> {
    let mut tx = pool
        .begin()
        .await
        .context("failed to begin order normalization")?;
    compact_order_tx(&mut tx, OrderKind::Priority).await?;
    compact_order_tx(&mut tx, OrderKind::Pinned).await?;
    tx.commit()
        .await
        .context("failed to commit order normalization")?;
    Ok(())
}

async fn ordered_ids_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    kind: OrderKind,
) -> Result<Vec<String>> {
    let sql: &'static str = match kind {
        OrderKind::Priority => {
            "SELECT id FROM clipboard_items \
             WHERE is_pinned = 0 AND priority_order IS NOT NULL \
             ORDER BY priority_order ASC, created_at ASC, id ASC"
        }
        OrderKind::Pinned => {
            "SELECT id FROM clipboard_items \
             WHERE is_pinned = 1 \
             ORDER BY pin_order IS NULL, pin_order ASC, created_at DESC, id ASC"
        }
    };
    sqlx::query_scalar(sql)
        .fetch_all(&mut **tx)
        .await
        .context("failed to read ordered clipboard ids")
        .map_err(Into::into)
}

async fn write_order_tx(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    kind: OrderKind,
    ids: &[String],
) -> Result<()> {
    let sql: &'static str = match kind {
        OrderKind::Priority => "UPDATE clipboard_items SET priority_order = ? WHERE id = ?",
        OrderKind::Pinned => "UPDATE clipboard_items SET pin_order = ? WHERE id = ?",
    };
    for (index, id) in ids.iter().enumerate() {
        sqlx::query(sql)
            .bind((index + 1) as i64)
            .bind(id)
            .execute(&mut **tx)
            .await
            .context("failed to write clipboard order")?;
    }
    Ok(())
}

/// 原位保存编辑后的文字。只改变内容字段，保留收藏、置顶、排序、分组和备注。
/// 与监听采集共用 UPSERT_GATE，避免与新内容去重写入竞态；CAS 防止覆盖他人的编辑。
pub async fn replace_item_text(
    pool: &SqlitePool,
    id: &str,
    expected_hash: &str,
    edited: &ClipboardItem,
) -> Result<()> {
    if edited.kind != ClipboardKind::Text {
        return Err(AppError::Clipboard(
            "only text clipboard items can be edited".to_owned(),
        ));
    }
    let _serial = UPSERT_GATE.lock().await;
    let mut tx = pool.begin().await.context("begin text edit transaction")?;

    let old_hash = sqlx::query_scalar::<_, String>(
        "SELECT content_hash FROM clipboard_items WHERE id = ? AND kind = 'text'",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .context("load current clipboard item version")?;
    if old_hash.as_deref() != Some(expected_hash) {
        return Err(AppError::Clipboard(
            "original clipboard text changed or was deleted; reopen the editor".to_owned(),
        ));
    }

    // 用户主动保存允许内容与其他记录相同，不合并/删除任何行。
    // 被手动编辑的 ID 会明确标记，避免应用重启后的旧记录整理误删它。

    let saved = sqlx::query(
        "UPDATE clipboard_items SET content = ?, content_hash = ?, search_text = ?, \
         summary = ?, sub_kind = ?, size = ?, is_sensitive = ?, is_manually_edited = 1, updated_at = ? \
         WHERE id = ? AND kind = 'text' AND content_hash = ?",
    )
    .bind(&edited.content)
    .bind(&edited.content_hash)
    .bind(edited.search_text.as_deref())
    .bind(edited.summary.as_deref())
    .bind(edited.sub_kind)
    .bind(edited.size)
    .bind(edited.is_sensitive)
    .bind(Utc::now())
    .bind(id)
    .bind(expected_hash)
    .execute(&mut *tx)
    .await
    .context("save original clipboard text")?;
    if saved.rows_affected() != 1 {
        return Err(AppError::Clipboard(
            "clipboard text changed during save; reopen the editor".to_owned(),
        ));
    }
    tx.commit().await.context("commit clipboard text edit")?;
    Ok(())
}

/// 更新备注，传 `None` 清空备注。
pub async fn update_item_note(pool: &SqlitePool, id: &str, note: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE clipboard_items SET note = ? WHERE id = ?")
        .bind(note)
        .bind(id)
        .execute(pool)
        .await
        .context("failed to update clipboard item note")?;
    Ok(())
}

/// 更新条目所属分组；不刷新 `updated_at`，避免污染最近使用排序。
pub async fn update_item_group(pool: &SqlitePool, id: &str, group_id: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE clipboard_items SET group_id = ? WHERE id = ?")
        .bind(group_id)
        .bind(id)
        .execute(pool)
        .await
        .context("failed to update clipboard item group")?;
    Ok(())
}

/// `use_count + 1` 并刷新 `updated_at`（命中去重时复用）。
pub async fn increment_item_use_count(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE clipboard_items SET use_count = use_count + 1, updated_at = ? WHERE id = ?",
    )
    .bind(Utc::now())
    .bind(id)
    .execute(pool)
    .await
    .context("failed to increment clipboard item use_count")?;
    Ok(())
}

/// 删除单条记录。若删除的是图片记录，返回其落盘文件名（`<hash>.png`），供调用方删图；
/// 否则返回 `None`。记录不存在时也返回 `None`。
///
/// image 去重指纹源自 PNG 字节、落盘文件名即字节哈希，故库里同图至多一行，
/// 删行后该文件必为孤儿，调用方可直接删，无需引用计数。
pub async fn delete_item(pool: &SqlitePool, id: &str) -> Result<Option<String>> {
    let row = sqlx::query_as::<_, (ClipboardKind, String)>(
        "DELETE FROM clipboard_items WHERE id = ? RETURNING kind, content",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("failed to delete clipboard item")?;

    if row.is_some() {
        normalize_item_orders(pool).await?;
    }

    Ok(row.and_then(|(kind, content)| image_file_name(kind, content)))
}

/// 取被删行里需要连带删除的图片文件名：kind 为 image 时 `content` 即文件名，否则 `None`。
fn image_file_name(kind: ClipboardKind, content: String) -> Option<String> {
    (kind == ClipboardKind::Image).then_some(content)
}

/// 批量删除，返回实际删除行数；`ids` 为空时不发查询。
#[allow(dead_code)]
pub async fn delete_items(pool: &SqlitePool, ids: &[String]) -> Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }

    let mut qb: QueryBuilder<Sqlite> =
        QueryBuilder::new("DELETE FROM clipboard_items WHERE id IN (");
    let mut separated = qb.separated(", ");
    for id in ids {
        separated.push_bind(id);
    }
    qb.push(")");

    let result = qb
        .build()
        .execute(pool)
        .await
        .context("failed to delete clipboard items")?;
    if result.rows_affected() > 0 {
        normalize_item_orders(pool).await?;
    }

    Ok(result.rows_affected())
}

/// 历史清理的结果：删除行数 + 其中图片记录的落盘文件名（供调用方删图）。
#[derive(Debug, Default)]
pub struct CleanupOutcome {
    pub removed: u64,
    pub image_files: Vec<String>,
}

/// 历史清理：按「时间下限」和「最大条数」删除条目；置顶 / 收藏项一律保留。
/// 返回删除行数与被删图片的文件名。`older_than = None` 跳过时长清理；`max_count = None` 或 `Some(0)` 跳过条数清理。
pub async fn cleanup_history(
    pool: &SqlitePool,
    older_than: Option<chrono::DateTime<chrono::Utc>>,
    max_count: Option<u32>,
) -> Result<CleanupOutcome> {
    let mut outcome = CleanupOutcome::default();

    if let Some(cutoff) = older_than {
        let rows = sqlx::query_as::<_, (ClipboardKind, String)>(
            "DELETE FROM clipboard_items \
             WHERE is_pinned = 0 AND is_favorite = 0 AND created_at < ? \
             RETURNING kind, content",
        )
        .bind(cutoff)
        .fetch_all(pool)
        .await
        .context("failed to cleanup clipboard items by retention")?;
        absorb_deleted(&mut outcome, rows);
    }

    if let Some(max) = max_count.filter(|n| *n > 0) {
        // 仅在非置顶 / 非收藏集合内按 created_at DESC 保留前 max 条，多余的删除。
        // SQLite 中 `LIMIT -1 OFFSET n` 表示「跳过前 n 条，剩下全要」。
        let rows = sqlx::query_as::<_, (ClipboardKind, String)>(
            "DELETE FROM clipboard_items WHERE id IN ( \
                 SELECT id FROM clipboard_items \
                 WHERE is_pinned = 0 AND is_favorite = 0 \
                 ORDER BY created_at DESC \
                 LIMIT -1 OFFSET ? \
             ) \
             RETURNING kind, content",
        )
        .bind(max as i64)
        .fetch_all(pool)
        .await
        .context("failed to cleanup clipboard items by max_count")?;
        absorb_deleted(&mut outcome, rows);
    }

    if outcome.removed > 0 {
        normalize_item_orders(pool).await?;
    }

    Ok(outcome)
}

/// 把一批被删行计入 outcome：累加行数，并收集其中的图片文件名。
fn absorb_deleted(outcome: &mut CleanupOutcome, rows: Vec<(ClipboardKind, String)>) {
    outcome.removed += rows.len() as u64;
    outcome.image_files.extend(
        rows.into_iter()
            .filter_map(|(kind, content)| image_file_name(kind, content)),
    );
}

/// 清空记录，返回删除行数与被删图片文件名；未显式删除的收藏 / 置顶项会保留。
pub async fn clear_items(
    pool: &SqlitePool,
    delete_favorites: bool,
    delete_pinned: bool,
) -> Result<CleanupOutcome> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("DELETE FROM clipboard_items");

    let mut has_condition = false;
    if !delete_favorites {
        qb.push(" WHERE is_favorite = 0");
        has_condition = true;
    }
    if !delete_pinned {
        qb.push(if has_condition { " AND " } else { " WHERE " });
        qb.push("is_pinned = 0");
    }
    qb.push(" RETURNING kind, content");

    let rows = qb
        .build_query_as::<(ClipboardKind, String)>()
        .fetch_all(pool)
        .await
        .context("failed to clear clipboard items")?;

    let mut outcome = CleanupOutcome::default();
    absorb_deleted(&mut outcome, rows);
    if outcome.removed > 0 {
        normalize_item_orders(pool).await?;
    }

    Ok(outcome)
}

/// 关键词过滤分流：≥3 字符走 FTS5（trigram），1–2 字符走 LIKE 兜底，空 / 仅空白不过滤。
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeywordFilter {
    None,
    /// `clipboard_items_fts MATCH ?` 的表达式，已含前缀通配。
    Fts(String),
    /// 已转义 `% _ \` 的关键词；下游统一拼成 `%<kw>%` 多列模糊匹配。
    Like(String),
}

impl KeywordFilter {
    /// 按字符数（非字节）判定走 FTS 还是 LIKE。CJK 一个字符也算 1，
    /// 与 trigram 的 3 字符门槛保持一致。
    ///
    /// 注意：门槛按**每个空白分词**判定，而非整串字符数。trigram 索引对 <3 字符的
    /// token 永远 0 命中，而 FTS5 表达式里多 token 之间默认是 AND —— 只要有一个短词，
    /// 整条表达式就被拖成 0 结果（例如 `a b` 会生成 `"a"* "b"*`，二者皆 <3 字符）。
    /// 因此仅当所有分词均 ≥3 字符时才走 FTS，否则降级到 LIKE 兜底。
    fn from_keyword(keyword: Option<&str>) -> Self {
        let Some(trimmed) = keyword.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::None;
        };

        let fts_viable = trimmed
            .split_whitespace()
            .all(|token| token.chars().count() >= 3);

        if fts_viable {
            if let Some(expr) = build_fts_expr(trimmed) {
                return Self::Fts(expr);
            }
        }

        Self::Like(escape_like(trimmed))
    }
}

/// 把用户关键词拆成 FTS5 前缀匹配表达式（如 `foo bar` -> `"foo"* "bar"*`）。
/// 双引号包裹 + 转义，避免关键词中的 FTS5 语法字符被当作运算符。空白关键词返回 `None`。
fn build_fts_expr(keyword: &str) -> Option<String> {
    let expr = keyword
        .split_whitespace()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ");

    (!expr.is_empty()).then_some(expr)
}

/// 转义 LIKE 的特殊字符（`\ % _`），配合 SQL 端 `ESCAPE '\\'`。
fn escape_like(keyword: &str) -> String {
    let mut out = String::with_capacity(keyword.len());
    for ch in keyword.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 拼装查询：过滤 + 常规手动优先 / 最近时间优先 + 稳定分页。
/// recent_first 只供收藏/内容快捷查找页使用，绝不修改持久化的排序字段。
/// 所有 bind 均传入拥有所有权/Copy 的值，避免 `QueryBuilder` 借用 `q` 引发的生命周期问题。
async fn fetch_items(
    pool: &SqlitePool,
    q: &ClipboardItemQuery,
    keyword: KeywordFilter,
) -> Result<Vec<ClipboardItem>> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(LIST_SELECT_ITEM);
    qb.push(" WHERE 1 = 1");
    push_filter_clauses(&mut qb, q, &keyword);

    if q.recent_first {
        // 快捷查找视图跨置顶/手动排序条目统一按最近时间排列。
        qb.push(
            " ORDER BY clipboard_items.updated_at DESC, \
             clipboard_items.created_at DESC, clipboard_items.id DESC",
        );
    } else {
        match q.pinned {
            Some(true) => {
                qb.push(
                    " ORDER BY clipboard_items.pin_order IS NULL, clipboard_items.pin_order ASC, ",
                );
            }
            Some(false) => {
                qb.push(
                    " ORDER BY clipboard_items.priority_order IS NULL, clipboard_items.priority_order ASC, ",
                );
            }
            None => {
                // 原有设计：置顶在前，再按独立手动排序；不可由快捷查找需求破坏。
                qb.push(
                    " ORDER BY clipboard_items.is_pinned DESC, \
                     CASE WHEN clipboard_items.is_pinned = 1 THEN clipboard_items.pin_order IS NULL ELSE 1 END ASC, \
                     CASE WHEN clipboard_items.is_pinned = 1 THEN clipboard_items.pin_order END ASC, \
                     CASE WHEN clipboard_items.is_pinned = 0 THEN clipboard_items.priority_order IS NULL ELSE 1 END ASC, \
                     CASE WHEN clipboard_items.is_pinned = 0 THEN clipboard_items.priority_order END ASC, ",
                );
            }
        }
        match q.sort {
            ClipboardItemSort::CreatedAt => {
                qb.push("clipboard_items.created_at DESC");
            }
            ClipboardItemSort::UpdatedAt => {
                qb.push("clipboard_items.updated_at DESC, clipboard_items.created_at DESC");
            }
            ClipboardItemSort::UseCount => {
                qb.push("clipboard_items.use_count DESC, clipboard_items.created_at DESC");
            }
        }
        // 时间、使用次数和手动排序位置相同时，稳定分页不得随机换序。
        qb.push(", clipboard_items.id DESC");
    }

    qb.push(" LIMIT ").push_bind(q.limit);
    qb.push(" OFFSET ").push_bind(q.offset);

    let items = qb
        .build_query_as::<ClipboardItem>()
        .fetch_all(pool)
        .await
        .context("failed to query clipboard items")?;
    Ok(items)
}

/// 统计满足同样过滤条件的总条数（不参与排序 / 分页），与 [`fetch_items`] 共用 [`push_filter_clauses`]。
async fn fetch_items_count(
    pool: &SqlitePool,
    q: &ClipboardItemQuery,
    keyword: KeywordFilter,
) -> Result<i64> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT COUNT(*) FROM clipboard_items");
    qb.push(" WHERE 1 = 1");
    push_filter_clauses(&mut qb, q, &keyword);

    let row: (i64,) = qb
        .build_query_as()
        .fetch_one(pool)
        .await
        .context("failed to count filtered clipboard items")?;
    Ok(row.0)
}

/// 把当前查询的过滤条件追加到 `qb`（不含 ORDER BY / LIMIT / OFFSET），供列表查询与计数共用。
fn push_filter_clauses(
    qb: &mut QueryBuilder<Sqlite>,
    q: &ClipboardItemQuery,
    keyword: &KeywordFilter,
) {
    match keyword {
        KeywordFilter::None => {}
        KeywordFilter::Fts(expr) => {
            qb.push(
                " AND clipboard_items.rowid IN (SELECT rowid FROM clipboard_items_fts WHERE clipboard_items_fts MATCH ",
            )
            .push_bind(expr.clone())
            .push(")");
        }
        KeywordFilter::Like(kw) => {
            // FTS 索引覆盖 search_text / note 两列，LIKE 兜底也跟齐，
            // 让 1–2 字符短词的命中范围与长词一致。
            let pattern = format!("%{kw}%");
            qb.push(" AND (clipboard_items.search_text LIKE ")
                .push_bind(pattern.clone())
                .push(" ESCAPE '\\' OR clipboard_items.note LIKE ")
                .push_bind(pattern)
                .push(" ESCAPE '\\')");
        }
    }
    // group（UI Tab）覆盖显式 kind / favorite；为 None 时回退到显式字段（单测使用）。
    let (effective_kind, effective_favorite) = match q.group {
        Some(ClipboardGroupFilter::All) => (None, None),
        Some(ClipboardGroupFilter::Text) => (Some(ClipboardKind::Text), None),
        Some(ClipboardGroupFilter::Image) => (Some(ClipboardKind::Image), None),
        Some(ClipboardGroupFilter::Files) => (Some(ClipboardKind::Files), None),
        Some(ClipboardGroupFilter::Favorite) => (None, Some(true)),
        None => (q.kind, q.favorite),
    };
    if let Some(kind) = effective_kind {
        qb.push(" AND clipboard_items.kind = ").push_bind(kind);
    }
    if let Some(group_id) = &q.group_id {
        qb.push(" AND clipboard_items.group_id = ")
            .push_bind(group_id.clone());
    }
    if let Some(favorite) = effective_favorite {
        qb.push(" AND clipboard_items.is_favorite = ")
            .push_bind(favorite);
    }
    if let Some(pinned) = q.pinned {
        qb.push(" AND clipboard_items.is_pinned = ")
            .push_bind(pinned);
    }
}

