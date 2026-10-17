use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::ConnectOptions;
use sqlx::SqlitePool;
use tauri::AppHandle;

use crate::core::Result;
use crate::db::db_path;

pub async fn init(app: &AppHandle) -> Result<SqlitePool> {
    let path = db_path(app)?;

    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .disable_statement_logging();

    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .with_context(|| format!("failed to open sqlite database at {path:?}"))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("failed to run sqlite migrations")?;

    // Repair only safe, unprotected legacy duplicate text rows. The former
    // raw HTML/RTF hash policy could save identical visible text more than
    // once; never silently remove favorites, pins, notes or manual orders.
    let consolidated = crate::db::items::consolidate_safe_text_duplicates(&pool)
        .await
        .context("failed to reconcile ordinary text duplicates")?;
    if consolidated > 0 {
        log::info!("consolidated {consolidated} ordinary duplicate clipboard texts");
    }

    log::info!("sqlite pool ready at {path:?}");
    Ok(pool)
}
