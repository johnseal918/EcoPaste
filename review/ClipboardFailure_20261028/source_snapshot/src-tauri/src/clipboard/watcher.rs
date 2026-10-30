//! OS 级剪贴板监听：把 [`clipboard_rs`] 的 watcher 接到「读取 → 去重入库 → emit」闭环。
//!
//! [`clipboard_rs`] 内部已实现 macOS（`NSPasteboard.changeCount` 轮询）/ Windows
//! （`AddClipboardFormatListener` → `WM_CLIPBOARDUPDATE`）的平台监听，这里不重复造。
//!
//! 线程模型：`ClipboardWatcherContext::start_watch()` 是阻塞调用，故整个监听跑在独立
//! `std::thread` 上。`ClipboardContext` 等平台句柄**在该线程内构造**，不跨线程移动，
//! 从而绕开其 `Send` 约束；只有 `Send` 的数据（`AppHandle`、`item`）会被
//! 投递进 Tauri 异步运行时做 sqlx 入库与事件 emit。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
#[cfg(target_os = "macos")]
use clipboard_rs::{ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext};
use serde_json::json;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(target_os = "windows")]
use winapi::um::winuser::GetClipboardSequenceNumber;

use super::app_store::AppIconStore;
use super::apps_registry::AppsRegistry;
use super::guard::WritebackGuard;
use super::ingest::build_item_with_settings;
use super::read::ClipboardReader;
use super::sound;
use super::source::{self, FrontmostApp};
use super::storage::ImageStore;
use crate::db::apps::upsert_app;
use crate::db::items::{upsert_item, UpsertResult};
use crate::db::models::{ClipboardApp, ClipboardItem};
use crate::settings::SettingsStore;

/// 剪贴板更新事件名。前端监听此事件后增量刷新 / 重新拉取列表。
pub const CLIPBOARD_UPDATED_EVENT: &str = "clipboard://updated";

/// macOS 轮询 `changeCount` 的间隔。上游 clipboard-rs 默认 500ms，对复制响应（尤其图片）
/// 偏慢；我们 fork 出 `new_with_interval` 后调到 120ms，跟手且 CPU 开销可忽略。
/// Windows 走事件驱动（`WM_CLIPBOARDUPDATE`），此值被忽略。
#[cfg(target_os = "macos")]
const CLIPBOARD_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(120);

/// Windows 不再依赖消息处理完成速度来决定何时读剪贴板：直接盯 sequence number。
/// 空闲时 8ms 已远快于人工连续 Ctrl+C；检测到一次变化后 2 秒内进入 1ms burst 模式，
/// 把下一次复制覆盖前的读取窗口压到最小。这里只做 GetClipboardSequenceNumber + sleep，
/// 没有 DB / 图标 / WebView 工作。
#[cfg(target_os = "windows")]
const WINDOWS_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(8);
#[cfg(target_os = "windows")]
const WINDOWS_BURST_POLL_INTERVAL: Duration = Duration::from_millis(1);
#[cfg(target_os = "windows")]
const WINDOWS_BURST_WINDOW: Duration = Duration::from_secs(2);

/// Empty formats are retryable for a short settling period. This is separate
/// from sampling frequency: a fast poll does not help if the reader reports a
/// temporary lock failure as permanently empty.
#[cfg(target_os = "windows")]
const WINDOWS_EMPTY_SETTLE_WINDOW: Duration = Duration::from_millis(500);
#[cfg(target_os = "windows")]
const WINDOWS_PENDING_RETRY_INTERVAL: Duration = Duration::from_millis(8);
/// 对永久读取失败的格式建立时间和次数双重上限。
#[cfg(target_os = "windows")]
const WINDOWS_FAILED_SETTLE_WINDOW: Duration = Duration::from_secs(2);
#[cfg(target_os = "windows")]
const WINDOWS_FAILED_MAX_ATTEMPTS: u8 = 16;
/// Do not seize Excel's clipboard while it is publishing a multi-format copy.
/// Other applications retain the existing fast sequence watcher behavior.
#[cfg(target_os = "windows")]
const WINDOWS_EXCEL_FIRST_READ_SETTLE: Duration = Duration::from_millis(120);
#[cfg(target_os = "windows")]
const WINDOWS_EXCEL_FAILED_MAX_ATTEMPTS: u8 = 3;
#[cfg(target_os = "windows")]
const WINDOWS_EXCEL_PENDING_RETRY_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureSnapshot {
    Queued,
    Empty,
    ReadFailed,
}

/// Valid snapshots are acknowledged immediately. Stable empty formats and
/// failed reads are retired after finite settling periods.
#[cfg(target_os = "windows")]
fn should_ack_snapshot(outcome: CaptureSnapshot, pending_age: Duration) -> bool {
    matches!(outcome, CaptureSnapshot::Queued)
        || (matches!(outcome, CaptureSnapshot::Empty) && pending_age >= WINDOWS_EMPTY_SETTLE_WINDOW)
        || (matches!(outcome, CaptureSnapshot::ReadFailed)
            && pending_age >= WINDOWS_FAILED_SETTLE_WINDOW)
}

/// Another clipboard listener can briefly hold the Windows clipboard open. Retry those read
/// failures within a bounded window before dropping the update.
#[cfg(target_os = "macos")]
const CLIPBOARD_READ_RETRY_DELAYS: [Duration; 5] = [
    Duration::from_millis(1),
    Duration::from_millis(2),
    Duration::from_millis(4),
    Duration::from_millis(8),
    Duration::from_millis(16),
];

#[cfg(target_os = "macos")]
fn read_with_retry<T, E>(
    retry_delays: &[Duration],
    mut read: impl FnMut() -> Result<Option<T>, E>,
) -> Result<Option<T>, E> {
    let mut result = read();
    for delay in retry_delays {
        // An apparently empty Windows clipboard can be a transient snapshot
        // while a copy application is filling its formats, not a final outcome.
        // Only a captured payload is final; retry both Err and Ok(None).
        if matches!(result, Ok(Some(_))) {
            return result;
        }
        std::thread::sleep(*delay);
        result = read();
    }
    result
}

/// 监听暂停开关。托盘菜单「停止监听」翻转，handler 早返回跳过整条入库链路。
/// 用 `Arc<AtomicBool>` 跨线程共享；不停 watcher 线程本身，避免反复重建平台句柄。
#[derive(Debug, Default, Clone)]
pub struct WatcherPause(Arc<AtomicBool>);

impl WatcherPause {
    pub fn is_paused(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.0.store(paused, Ordering::Relaxed);
    }
}

/// 把同步抓到的 [`FrontmostApp`] 落 icon 字节 + 拼成可入库的 [`ClipboardApp`]。
/// icon 落盘失败不阻断（仍保留应用名），仅 warn。
///
/// `registry` 命中缓存时优先复用，省掉一次 PNG 字节 sha256/IO；
/// 缓存未命中再走 FrontmostApp.icon_png 路径，
/// 并把结果回写缓存，让首次见到的应用后续直接命中。
pub fn materialize_source(
    store: &AppIconStore,
    registry: Option<&AppsRegistry>,
    src: FrontmostApp,
) -> ClipboardApp {
    if let Some(reg) = registry {
        if let Some(cached) = reg.get(&src.id) {
            return cached;
        }
    }

    let icon_file = src
        .icon_path
        .as_deref()
        .and_then(|path| super::icon::icon_png(path, None))
        .as_deref()
        .and_then(|bytes| match store.store(bytes) {
            Ok(name) => Some(name),
            Err(err) => {
                log::warn!("app icon store failed for {}: {err}", src.id);
                None
            }
        });
    let now = Utc::now();
    let app = ClipboardApp {
        id: src.id,
        name: src.name,
        icon_file,
        platform: src.platform,
        created_at: now,
        updated_at: now,
    };
    if let Some(reg) = registry {
        reg.insert_into_cache(app.clone());
    }
    app
}

/// 去重入库 + emit「剪贴板更新」事件。监听回调与 `read_clipboard` 命令共用，
/// 保证两条路径的入库语义与事件契约一致。失败仅记日志（监听场景无人接收 Result）。
///
/// `source_app` 为 `Some` 时先 upsert apps 表再写 item，满足 FK 约束。
/// 应用 upsert 失败不阻断条目入库——清掉 source_app_id 后继续，避免单次系统调用抽风丢内容。
pub async fn persist_and_notify(
    app: &AppHandle,
    pool: &SqlitePool,
    item: &ClipboardItem,
    source_app: Option<&ClipboardApp>,
) -> crate::core::Result<UpsertResult> {
    let mut item_to_write = item.clone();
    if let Some(src) = source_app {
        match upsert_app(pool, src).await {
            Ok(()) => {}
            Err(err) => {
                log::warn!("clipboard source app upsert failed ({}): {err}", src.id);
                item_to_write.source_app_id = None;
            }
        }
    }
    let result = upsert_item(pool, &item_to_write).await?;
    sound::maybe_play_copy(app);
    if let Err(err) = app.emit(
        CLIPBOARD_UPDATED_EVENT,
        json!({
            "id": result.id,
            "kind": item_to_write.kind,
            "deduplicated": result.deduplicated,
        }),
    ) {
        log::warn!("emit {CLIPBOARD_UPDATED_EVENT} failed: {err}");
    }
    Ok(result)
}

/// 启动监听：注册 [`WritebackGuard`] / [`ImageStore`] / [`AppIconStore`] 到 Tauri `State`
/// （供写回时打标记 / 取图 / 取来源应用图标），并在独立线程上跑 OS 级监听。
/// 应在 `setup` 中、连接池就绪后调用一次。store 创建失败属致命配置错误，直接返回错误。
pub fn init(app: &AppHandle) -> crate::core::Result<()> {
    let guard = Arc::new(WritebackGuard::new());
    app.manage(guard.clone());

    let store = ImageStore::new(app)?;
    app.manage(store.clone());

    let app_icon_store = AppIconStore::new(app)?;
    app.manage(app_icon_store.clone());

    let file_icon_store = super::FileIconStore::new(app)?;
    app.manage(file_icon_store);

    let registry = AppsRegistry::new(app.clone(), app_icon_store.clone());
    app.manage(registry.clone());

    let pause = WatcherPause::default();
    app.manage(pause.clone());

    // 启动期只把已落库的应用读进缓存；运行中应用由偏好页打开/刷新时补齐。
    {
        let registry = registry.clone();
        let excluded_app_ids = app
            .try_state::<SettingsStore>()
            .map(|store| store.snapshot().clipboard.filters.excluded_app_ids)
            .unwrap_or_default();
        tauri::async_runtime::spawn(async move {
            if let Err(err) = registry.load_from_db().await {
                log::warn!("apps registry: initial DB load failed: {err}");
            }
            if let Err(err) =
                super::apps_registry::add_apps_from_ids(registry.clone(), excluded_app_ids).await
            {
                log::warn!("apps registry: initial excluded app materialization failed: {err}");
            }
        });
    }

    super::cleanup::spawn(app.clone());
    spawn_watch_thread(app.clone(), guard, store, app_icon_store, registry, pause);
    Ok(())
}

#[cfg(target_os = "macos")]
fn spawn_watch_thread(
    app: AppHandle,
    guard: Arc<WritebackGuard>,
    store: ImageStore,
    app_icon_store: AppIconStore,
    registry: AppsRegistry,
    pause: WatcherPause,
) {
    std::thread::Builder::new()
        .name("clipboard-watcher".to_owned())
        .spawn(move || {
            let reader = match ClipboardReader::new() {
                Ok(reader) => reader,
                Err(err) => {
                    log::error!("clipboard watcher: failed to create reader: {err}");
                    return;
                }
            };

            let mut watcher =
                match ClipboardWatcherContext::new_with_interval(CLIPBOARD_POLL_INTERVAL) {
                    Ok(watcher) => watcher,
                    Err(err) => {
                        log::error!("clipboard watcher: failed to create watcher: {err}");
                        return;
                    }
                };

            watcher.add_handler(ClipboardChangeHandler {
                reader,
                app,
                guard,
                store,
                app_icon_store,
                registry,
                pause,
            });

            log::info!("clipboard watcher started");
            watcher.start_watch();
        })
        .expect("failed to spawn clipboard watcher thread");
}

#[cfg(target_os = "windows")]
fn spawn_watch_thread(
    app: AppHandle,
    guard: Arc<WritebackGuard>,
    store: ImageStore,
    app_icon_store: AppIconStore,
    registry: AppsRegistry,
    pause: WatcherPause,
) {
    std::thread::Builder::new()
        .name("clipboard-sequence-watcher".to_owned())
        .spawn(move || {
            let reader = match ClipboardReader::new() {
                Ok(reader) => reader,
                Err(err) => {
                    log::error!("clipboard sequence watcher: failed to create reader: {err}");
                    return;
                }
            };

            let mut last_sequence = unsafe { GetClipboardSequenceNumber() };
            // Current clipboard sequence + first-observed instant + next attempt.
            // The sequence MUST NOT be acknowledged before a valid snapshot.
            let mut pending: Option<(u32, std::time::Instant, std::time::Instant, u8)> = None;
            let mut burst_until: Option<std::time::Instant> = None;
            log::info!("clipboard Windows sequence watcher started");

            loop {
                if pause.is_paused() {
                    // Explicit user pause is the only intentional loss of history.
                    last_sequence = unsafe { GetClipboardSequenceNumber() };
                    pending = None;
                    burst_until = None;
                    std::thread::sleep(Duration::from_millis(25));
                    continue;
                }

                let sequence = unsafe { GetClipboardSequenceNumber() };
                let now = std::time::Instant::now();
                if sequence != 0
                    && sequence != last_sequence
                    && pending.as_ref().is_none_or(|entry| entry.0 != sequence)
                {
                    if let Some((unread, first_seen, _, _)) = pending.take() {
                        log::warn!(
                            "clipboard sequence {unread} was replaced by {sequence} before a readable snapshot (pending {}ms)",
                            first_seen.elapsed().as_millis()
                        );
                    }
                    let excel_copy = source::clipboard_may_be_excel();
                    let first_attempt = now
                        + if excel_copy {
                            WINDOWS_EXCEL_FIRST_READ_SETTLE
                        } else {
                            Duration::ZERO
                        };
                    pending = Some((sequence, now, first_attempt, 0));
                    if excel_copy {
                        log::info!("clipboard sequence={sequence} excel_initial_settle_ms=120");
                    }
                    burst_until = Some(now + WINDOWS_BURST_WINDOW);
                }

                if let Some((target_sequence, first_seen, next_attempt, failed_attempts)) = pending {
                    if now >= next_attempt {
                        // A newer clipboard publication invalidates the pending
                        // request without ever opening the stale format.
                        if unsafe { GetClipboardSequenceNumber() } != target_sequence {
                            continue;
                        }
                        let excel_copy = source::clipboard_may_be_excel();
                        let attempt_started = std::time::Instant::now();
                        let outcome = capture_and_enqueue(
                            &reader,
                            &app,
                            &guard,
                            &store,
                            &app_icon_store,
                            &registry,
                        );
                        let read_elapsed = attempt_started.elapsed();
                        if read_elapsed >= Duration::from_millis(250) {
                            log::warn!(
                                "clipboard sequence={target_sequence} owner_pid={:?} read_ms={} outcome={outcome:?}",
                                source::clipboard_owner_pid(),
                                read_elapsed.as_millis()
                            );
                        }
                        let elapsed = first_seen.elapsed();
                        let failures = failed_attempts.saturating_add(u8::from(matches!(
                            outcome,
                            CaptureSnapshot::ReadFailed
                        )));
                        let max_attempts = if excel_copy {
                            WINDOWS_EXCEL_FAILED_MAX_ATTEMPTS
                        } else {
                            WINDOWS_FAILED_MAX_ATTEMPTS
                        };
                        let attempts_exhausted = matches!(outcome, CaptureSnapshot::ReadFailed)
                            && failures >= max_attempts;
                        if should_ack_snapshot(outcome, elapsed) || attempts_exhausted {
                            if matches!(outcome, CaptureSnapshot::Empty) {
                                log::debug!(
                                    "clipboard sequence {target_sequence}: no enabled/available payload after {}ms",
                                    elapsed.as_millis()
                                );
                            } else if matches!(outcome, CaptureSnapshot::ReadFailed) {
                                log::warn!(
                                    "clipboard sequence={target_sequence} read_failed: retiring after {}ms and {} failed attempts",
                                    elapsed.as_millis(),
                                    failures
                                );
                            }
                            last_sequence = target_sequence;
                            pending = None;
                        } else {
                            // A newer sequence preempts this finite retry path.
                            pending = Some((
                                target_sequence,
                                first_seen,
                                std::time::Instant::now()
                                    + if excel_copy {
                                        WINDOWS_EXCEL_PENDING_RETRY_INTERVAL
                                    } else {
                                        WINDOWS_PENDING_RETRY_INTERVAL
                                    },
                                failures,
                            ));
                        }
                    }
                }

                let in_burst = burst_until.is_some_and(|until| std::time::Instant::now() < until);
                std::thread::sleep(if in_burst {
                    WINDOWS_BURST_POLL_INTERVAL
                } else {
                    WINDOWS_IDLE_POLL_INTERVAL
                });
            }
        })
        .expect("failed to spawn clipboard sequence watcher thread");
}

#[cfg(target_os = "macos")]
struct ClipboardChangeHandler {
    reader: ClipboardReader,
    app: AppHandle,
    guard: Arc<WritebackGuard>,
    store: ImageStore,
    app_icon_store: AppIconStore,
    registry: AppsRegistry,
    pause: WatcherPause,
}

#[cfg(target_os = "macos")]
impl ClipboardHandler for ClipboardChangeHandler {
    fn on_clipboard_change(&mut self) {
        if self.pause.is_paused() {
            return;
        }

        capture_and_enqueue(
            &self.reader,
            &self.app,
            &self.guard,
            &self.store,
            &self.app_icon_store,
            &self.registry,
        );
    }
}

fn capture_and_enqueue(
    reader: &ClipboardReader,
    app: &AppHandle,
    guard: &Arc<WritebackGuard>,
    store: &ImageStore,
    app_icon_store: &AppIconStore,
    registry: &AppsRegistry,
) -> CaptureSnapshot {
    let settings = app
        .try_state::<SettingsStore>()
        .map(|s| s.snapshot())
        .unwrap_or_default();

    // 在可能耗时的剪贴板格式渲染前捕获 owner，避免复制后切窗导致来源错判。
    let source_hint = source::capture_clipboard_source_hint();
    // Windows 外层 pending 会处理短暂占用；不再对每次失败在内部重读六次昂贵格式。
    #[cfg(target_os = "windows")]
    let read_result = reader.read_with_capture(&settings.clipboard.capture);
    #[cfg(target_os = "macos")]
    let read_result = read_with_retry(&CLIPBOARD_READ_RETRY_DELAYS, || {
        reader.read_with_capture(&settings.clipboard.capture)
    });
    let payload = match read_result {
        Ok(Some(payload)) => payload,
        Ok(None) => return CaptureSnapshot::Empty,
        Err(err) => {
            let category = match err {
                crate::core::AppError::Clipboard(ref message)
                    if message.contains("text snapshot busy") => "clipboard_busy",
                crate::core::AppError::Clipboard(_) => "format_read_failed",
                crate::core::AppError::Other(_) => "other_reader_error",
            };
            log::debug!("clipboard snapshot outcome=read_failed category={category}");
            return CaptureSnapshot::ReadFailed;
        }
    };
    let captured_at = Utc::now();

    let app = app.clone();
    let guard = guard.clone();
    let store = store.clone();
    let app_icon_store = app_icon_store.clone();
    let registry = registry.clone();
    tauri::async_runtime::spawn(async move {
        let prepared = tauri::async_runtime::spawn_blocking(move || {
            let mut item = match build_item_with_settings(
                &store,
                &payload,
                &settings.clipboard.capture,
                &settings.clipboard.sensitive,
                settings.clipboard.content.copy_plain,
            ) {
                Ok(Some(item)) => item,
                Ok(None) => return None,
                Err(err) => {
                    log::warn!("clipboard watcher: build item failed: {err}");
                    return None;
                }
            };

            item.created_at = captured_at;
            item.updated_at = captured_at;

            if guard.should_skip(&item.content_hash) {
                return None;
            }

            let source = source_hint.and_then(source::resolve_frontmost_hint);
            if let Some(src) = &source {
                if settings
                    .clipboard
                    .filters
                    .excluded_app_ids
                    .iter()
                    .any(|id| id == &src.id)
                {
                    return None;
                }
            }

            let source_app =
                source.map(|src| materialize_source(&app_icon_store, Some(&registry), src));
            if let Some(src) = &source_app {
                item.source_app_id = Some(src.id.clone());
            }

            Some((item, source_app))
        })
        .await;

        let Some((item, source_app)) = (match prepared {
            Ok(prepared) => prepared,
            Err(err) => {
                log::error!("clipboard watcher: background preparation task failed: {err}");
                None
            }
        }) else {
            return;
        };

        let pool = app.state::<crate::db::DatabaseState>().pool().await;
        // A transient SQLite busy/lock error must not silently discard an
        // already frozen clipboard payload. Never repeat the system read here.
        for (attempt, delay) in [0u64, 20, 60, 120].into_iter().enumerate() {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            match persist_and_notify(&app, &pool, &item, source_app.as_ref()).await {
                Ok(_) => return,
                Err(err) if attempt < 3 => {
                    log::warn!(
                        "clipboard watcher: persist attempt {} failed, will retry: {err}",
                        attempt + 1
                    );
                }
                Err(err) => {
                    log::error!("clipboard watcher: persist failed after 4 attempts: {err}");
                    return;
                }
            }
        }
    });
    CaptureSnapshot::Queued
}

