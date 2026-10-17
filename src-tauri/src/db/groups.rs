use anyhow::Context;
use sqlx::SqlitePool;

use crate::core::Result;
use crate::db::models::ClipboardGroup;

const LIST_GROUPS_SQL: &str =
    "SELECT id, name, icon, is_hidden, sort_order, created_at, updated_at FROM clipboard_groups \
     ORDER BY sort_order ASC, created_at ASC";

/// 列出全部分组，按 `sort_order` 升序（同序时按 `created_at` 兜底，保证顺序稳定）。
pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<ClipboardGroup>> {
    let groups = sqlx::query_as::<_, ClipboardGroup>(LIST_GROUPS_SQL)
        .fetch_all(pool)
        .await
        .context("failed to list clipboard groups")?;
    Ok(groups)
}

/// 读取下一条自定义分组的排序值。
pub async fn next_group_sort_order(pool: &SqlitePool) -> Result<i64> {
    let row: (Option<i64>,) = sqlx::query_as("SELECT MAX(sort_order) FROM clipboard_groups")
        .fetch_one(pool)
        .await
        .context("failed to read max clipboard group sort order")?;

    Ok(row.0.unwrap_or(-1) + 1)
}

/// 新建分组。
pub async fn insert_group(pool: &SqlitePool, group: &ClipboardGroup) -> Result<()> {
    sqlx::query(
        "INSERT INTO clipboard_groups (id, name, icon, is_hidden, sort_order, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(group.id.as_str())
    .bind(group.name.as_str())
    .bind(group.icon.as_str())
    .bind(group.is_hidden)
    .bind(group.sort_order)
    .bind(group.created_at)
    .bind(group.updated_at)
    .execute(pool)
    .await
    .context("failed to insert clipboard group")?;
    Ok(())
}

/// 更新分组基础信息。
pub async fn update_group(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    icon: &str,
    is_hidden: bool,
) -> Result<()> {
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE clipboard_groups SET name = ?, icon = ?, is_hidden = ?, updated_at = ? WHERE id = ?",
    )
        .bind(name)
        .bind(icon)
        .bind(is_hidden)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await
        .context("failed to update clipboard group")?;
    Ok(())
}

/// 批量更新分组排序和隐藏状态。
pub async fn update_group_layout(
    pool: &SqlitePool,
    ordered_ids: &[String],
    visible_ids: &[String],
) -> Result<()> {
    let now = chrono::Utc::now();
    let visible_set: std::collections::HashSet<&str> =
        visible_ids.iter().map(String::as_str).collect();

    for (index, id) in ordered_ids.iter().enumerate() {
        sqlx::query(
            "UPDATE clipboard_groups SET sort_order = ?, is_hidden = ?, updated_at = ? WHERE id = ?",
        )
        .bind(index as i64)
        .bind(!visible_set.contains(id.as_str()))
        .bind(now)
        .bind(id)
        .execute(pool)
        .await
        .context("failed to update clipboard group layout")?;
    }

    Ok(())
}

/// 删除分组；其下记录的 `group_id` 由外键 `ON DELETE SET NULL` 自动置空（已启用 `foreign_keys`）。
pub async fn delete_group(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM clipboard_groups WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .context("failed to delete clipboard group")?;
    Ok(())
}

