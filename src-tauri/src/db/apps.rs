//! 来源应用仓储：以 macOS bundle id / Windows exe 路径作主键去重，
//! 同 id 二次入库走「保留 created_at、刷新 name/icon_file/updated_at」语义——
//! 应用改名或换图标时无需重建条目，引用方（`clipboard_items.source_app_id`）天然跟着更新。

use anyhow::Context;
use chrono::Utc;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::core::Result;
use crate::db::models::ClipboardApp;

const SELECT_APP: &str = "SELECT id, name, icon_file, platform, created_at, updated_at \
     FROM clipboard_apps";

/// 按 id 查单条记录。
#[allow(dead_code)]
pub async fn find_app_by_id(pool: &SqlitePool, id: &str) -> Result<Option<ClipboardApp>> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(SELECT_APP);
    qb.push(" WHERE id = ").push_bind(id.to_owned());
    let row = qb
        .build_query_as::<ClipboardApp>()
        .fetch_optional(pool)
        .await
        .context("failed to find clipboard app by id")?;
    Ok(row)
}

/// 按 id 列表批量取——给前端渲染卡片时一次性补齐 icon/name 用。
/// 空列表直接返回空结果，避免拼出 `IN ()` 这种非法 SQL。
pub async fn list_apps_by_ids(pool: &SqlitePool, ids: &[String]) -> Result<Vec<ClipboardApp>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(SELECT_APP);
    qb.push(" WHERE id IN (");
    let mut sep = qb.separated(", ");
    for id in ids {
        sep.push_bind(id.clone());
    }
    qb.push(")");
    let rows = qb
        .build_query_as::<ClipboardApp>()
        .fetch_all(pool)
        .await
        .context("failed to list clipboard apps by ids")?;
    Ok(rows)
}

/// 列出全部已知应用（监听过程中捕获、手动添加或默认忽略物化的应用）。
/// 名称按大小写不敏感升序，给前端过滤选择 UI 一个稳定顺序。
pub async fn list_all_apps(pool: &SqlitePool) -> Result<Vec<ClipboardApp>> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(SELECT_APP);
    qb.push(" ORDER BY name COLLATE NOCASE ASC, id ASC");
    let rows = qb
        .build_query_as::<ClipboardApp>()
        .fetch_all(pool)
        .await
        .context("failed to list all clipboard apps")?;
    Ok(rows)
}

/// 删除未被历史记录引用的来源应用，返回实际删除的应用 id。
pub async fn delete_unreferenced_apps(pool: &SqlitePool, ids: &[String]) -> Result<Vec<String>> {
    let mut deleted = Vec::new();

    for id in ids {
        let result = sqlx::query(
            "DELETE FROM clipboard_apps \
             WHERE id = ? \
               AND NOT EXISTS (SELECT 1 FROM clipboard_items WHERE source_app_id = ? LIMIT 1)",
        )
        .bind(id)
        .bind(id)
        .execute(pool)
        .await
        .context("failed to delete unreferenced clipboard app")?;

        if result.rows_affected() > 0 {
            deleted.push(id.clone());
        }
    }

    Ok(deleted)
}

/// upsert：id 已存在则只刷新 name / icon_file / updated_at；不存在则全量插入。
/// 显式分两路而非依赖 `INSERT OR REPLACE`：后者会重置 created_at 与（潜在的）外键级联。
pub async fn upsert_app(pool: &SqlitePool, app: &ClipboardApp) -> Result<()> {
    let now = Utc::now();
    let updated = sqlx::query(
        "UPDATE clipboard_apps SET name = ?, icon_file = ?, updated_at = ? WHERE id = ?",
    )
    .bind(app.name.as_str())
    .bind(app.icon_file.as_deref())
    .bind(now)
    .bind(app.id.as_str())
    .execute(pool)
    .await
    .context("failed to update clipboard app")?;

    if updated.rows_affected() > 0 {
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO clipboard_apps (id, name, icon_file, platform, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(app.id.as_str())
    .bind(app.name.as_str())
    .bind(app.icon_file.as_deref())
    .bind(app.platform)
    .bind(app.created_at)
    .bind(app.updated_at)
    .execute(pool)
    .await
    .context("failed to insert clipboard app")?;
    Ok(())
}

