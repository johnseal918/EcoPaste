pub mod lifecycle;
pub(super) mod position;
pub mod preview;
mod state;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub use macos::handle_reopen;
pub use state::WindowStateStore;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window};

use crate::core::Result;
use crate::settings::{SettingsStore, SidePanelKind, WindowPosition};

pub const CLIPBOARD_WINDOW_LABEL: &str = "clipboard";
pub const CLIPBOARD_PINNED_WINDOW_LABEL: &str = "clipboard-pinned";
pub const PREFERENCE_WINDOW_LABEL: &str = "preference";
pub const CLIPBOARD_PREVIEW_WINDOW_LABEL: &str = "clipboard-preview";
pub const ONBOARDING_WINDOW_LABEL: &str = "onboarding";
pub const UPDATE_WINDOW_LABEL: &str = "update";

/// 偏好页定位高亮事件。前端收到后切到目标设置项所在分类并滚动高亮。
const PREFERENCE_HIGHLIGHT_EVENT: &str = "preference://highlight-setting";

/// 偏好窗口重建前暂存的高亮目标设置项。
///
/// preference 改为空闲可销毁后，「打开偏好并定位到某设置项」这类一次性投递存在竞态：
/// 窗口已销毁时重建是异步的，直接 `emit` 会丢给尚未挂载的前端（与 backup 接收同源）。
/// 故窗口不存在时先存入此 slot，由前端重建后经 `take_pending_preference_highlight` 主动拉取。
static PENDING_PREFERENCE_HIGHLIGHT: LazyLock<Mutex<Option<String>>> =
    LazyLock::new(|| Mutex::new(None));

/// 当前这一次主剪贴板窗口会话里实际打开的右侧副面板。
/// 主窗口隐藏时清空；下次显示只从 Settings.side_panels.always_show 恢复。
static OPEN_CLIPBOARD_SIDE_PANELS: LazyLock<Mutex<Vec<SidePanelKind>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

const SIDE_PANELS_UPDATED_EVENT: &str = "clipboard-side-panels://updated";

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardSidePanelsState {
    pub open: Vec<SidePanelKind>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PreferenceHighlightPayload {
    setting_id: String,
}

/// 剪贴板窗口「固定」状态：true 时失焦不自动隐藏（点击窗外、切到其它 App 都不会隐藏），
/// 由前端 Pin 按钮 / 快捷键切换；macOS resign_key 与 Windows 外部点击钩子都尊重这个开关。
static CLIPBOARD_WINDOW_PINNED: AtomicBool = AtomicBool::new(false);
/// 剪贴板窗口自动隐藏的临时暂停状态，用于系统文件选择等会短暂转移焦点的原生交互。
static CLIPBOARD_WINDOW_AUTO_HIDE_SUSPENDED: AtomicBool = AtomicBool::new(false);

/// 返回用户是否显式固定剪贴板窗口；复制后隐藏等路径仍需读取这个用户态开关。
pub fn is_clipboard_window_pinned() -> bool {
    CLIPBOARD_WINDOW_PINNED.load(Ordering::Relaxed)
}

/// 判断剪贴板窗口当前是否允许因失焦或外部点击自动隐藏。
pub fn should_auto_hide_clipboard_window() -> bool {
    !CLIPBOARD_WINDOW_PINNED.load(Ordering::Relaxed)
        && !CLIPBOARD_WINDOW_AUTO_HIDE_SUSPENDED.load(Ordering::Relaxed)
}

/// 设置用户控制的剪贴板窗口固定态。
pub fn set_clipboard_window_pinned(pinned: bool) {
    CLIPBOARD_WINDOW_PINNED.store(pinned, Ordering::Relaxed);
}

/// 临时暂停剪贴板窗口自动隐藏，不改变用户控制的固定态。
pub fn set_clipboard_window_auto_hide_suspended(suspended: bool) {
    CLIPBOARD_WINDOW_AUTO_HIDE_SUSPENDED.store(suspended, Ordering::Relaxed);
}

pub fn set_clipboard_window_editing(
    app_handle: &AppHandle,
    label: &str,
    editing: bool,
) -> Result<()> {
    #[cfg(target_os = "windows")]
    return windows::set_clipboard_window_editing(app_handle, label, editing);

    #[cfg(target_os = "macos")]
    {
        let _ = app_handle;
        let _ = label;
        let _ = editing;

        Ok(())
    }
}

/// 剪贴板窗口显隐变化事件。前端用以做默认聚焦 / 自动清空搜索等 UI 副作用。
/// 由 [`show_window`] / [`hide_window`] 在统一入口处发出，平台一致，
/// 不依赖 `tauri://focus` / `tauri://blur`（Windows 剪贴板窗口 `focusable: false` 不可靠）。
const WINDOW_VISIBILITY_EVENT: &str = "window://visibility";

#[derive(Clone, serde::Serialize)]
struct WindowVisibilityPayload<'a> {
    label: &'a str,
    visible: bool,
}

pub(super) fn emit_visibility(app_handle: &AppHandle, label: &str, visible: bool) {
    if let Err(err) = app_handle.emit(
        WINDOW_VISIBILITY_EVENT,
        WindowVisibilityPayload { label, visible },
    ) {
        log::error!("emit window visibility failed: {err:?}");
    }
}

pub(super) fn get_window(app_handle: &AppHandle, label: &str) -> Result<WebviewWindow> {
    app_handle
        .get_webview_window(label)
        .ok_or_else(|| anyhow::anyhow!("window not found: {label}").into())
}

pub fn clipboard_side_panels_state() -> ClipboardSidePanelsState {
    let open = OPEN_CLIPBOARD_SIDE_PANELS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    ClipboardSidePanelsState { open }
}

fn emit_side_panels_state(app_handle: &AppHandle) {
    if let Err(err) = app_handle.emit(SIDE_PANELS_UPDATED_EVENT, clipboard_side_panels_state()) {
        log::warn!("emit side panels state failed: {err}");
    }
}

fn ordered_side_panels(order: &[SidePanelKind], selected: &[SidePanelKind]) -> Vec<SidePanelKind> {
    let mut result = Vec::new();

    for panel in order {
        if selected.contains(panel) && !result.contains(panel) {
            result.push(*panel);
        }
    }
    for panel in SidePanelKind::ALL {
        if selected.contains(&panel) && !result.contains(&panel) {
            result.push(panel);
        }
    }

    result
}

fn set_open_side_panels(open: Vec<SidePanelKind>) {
    *OPEN_CLIPBOARD_SIDE_PANELS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = open;
}

fn restore_always_show_side_panels(app_handle: &AppHandle) {
    let Some(store) = app_handle.try_state::<SettingsStore>() else {
        set_open_side_panels(Vec::new());
        return;
    };
    let snapshot = store.snapshot();
    let open = ordered_side_panels(
        &snapshot.clipboard.side_panels.order,
        &snapshot.clipboard.side_panels.always_show,
    );
    set_open_side_panels(open);
}

pub fn set_clipboard_side_panel_open(
    app_handle: &AppHandle,
    panel: SidePanelKind,
    open: bool,
) -> Result<ClipboardSidePanelsState> {
    let order = app_handle
        .try_state::<SettingsStore>()
        .map(|store| store.snapshot().clipboard.side_panels.order)
        .unwrap_or_else(|| SidePanelKind::ALL.to_vec());

    {
        let mut current = OPEN_CLIPBOARD_SIDE_PANELS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if open {
            if !current.contains(&panel) {
                current.push(panel);
            }
        } else {
            current.retain(|candidate| *candidate != panel);
        }

        *current = ordered_side_panels(&order, current);
    }

    apply_side_panels_visibility(app_handle)?;
    emit_side_panels_state(app_handle);
    Ok(clipboard_side_panels_state())
}

fn apply_side_panels_visibility(app_handle: &AppHandle) -> Result<()> {
    let should_show = !clipboard_side_panels_state().open.is_empty();
    let companion = get_window(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL)?;

    if should_show {
        sync_pinned_panel_layout(app_handle)?;

        #[cfg(target_os = "macos")]
        macos::show_window(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL)?;
        #[cfg(target_os = "windows")]
        windows::show_window(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL)?;

        emit_visibility(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL, true);
        lifecycle::on_shown(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL);
    } else if companion.is_visible().unwrap_or(false) {
        #[cfg(target_os = "macos")]
        macos::hide_window(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL)?;
        #[cfg(target_os = "windows")]
        windows::hide_window(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL)?;

        emit_visibility(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL, false);
        lifecycle::on_hidden(
            app_handle,
            CLIPBOARD_PINNED_WINDOW_LABEL,
            "side-panels-empty",
        );
    }

    Ok(())
}

pub fn show_window(app_handle: &AppHandle, label: &str) -> Result<()> {
    // 销毁后重建：`DestroyWhenIdle` 窗口空闲超时后 WebView 已被销毁，打开时按 descriptor
    // 的 build fn 重新建窗。重建后窗口为 `visible: false`，下方走与既有一致的恢复 + show 流程。
    if app_handle.get_webview_window(label).is_none() {
        if let Some(build) = lifecycle::rebuild_fn(label) {
            build(app_handle)?;
        }
    }

    if label == CLIPBOARD_WINDOW_LABEL {
        restore_always_show_side_panels(app_handle);
        if let Err(err) = apply_clipboard_window_layout(app_handle) {
            log::warn!("apply clipboard window layout failed: {err}");
        }
        if !clipboard_side_panels_state().open.is_empty() {
            if let Err(err) = sync_pinned_panel_layout(app_handle) {
                log::warn!("position clipboard side panels failed: {err}");
            }
        }
    } else if label == ONBOARDING_WINDOW_LABEL {
        if let Err(err) = position_window(app_handle, label, WindowPosition::Center) {
            log::warn!("center onboarding window failed: {err}");
        }
    } else {
        let visible = get_window(app_handle, label)?.is_visible().unwrap_or(false);

        if !visible {
            // 次级窗口（如 preference）：只在从隐藏态打开时恢复位置 + 尺寸。
            // 已可见窗口可能刚被用户移动但尚未落盘，重复恢复会把窗口拉回旧位置。
            if let Err(err) = state::restore_window_state(app_handle, label) {
                log::warn!("restore window state failed for {label}: {err}");
            }
        }
    }

    #[cfg(target_os = "macos")]
    let result = macos::show_window(app_handle, label);
    #[cfg(target_os = "windows")]
    let result = windows::show_window(app_handle, label);
    if result.is_ok() && !delays_clipboard_visibility_event(label) {
        if label == CLIPBOARD_WINDOW_LABEL {
            preview::resume_after_clipboard_show();
        }
        emit_visibility(app_handle, label, true);
        lifecycle::on_shown(app_handle, label);
    }

    if result.is_ok() && label == CLIPBOARD_WINDOW_LABEL {
        if let Err(err) = apply_side_panels_visibility(app_handle) {
            log::warn!("show clipboard side panels failed: {err}");
        }
        emit_side_panels_state(app_handle);
    }

    result
}

/// macOS 剪贴板窗口有延迟 show，visibility 需等 NSPanel 真的显示后再 emit。
fn delays_clipboard_visibility_event(label: &str) -> bool {
    cfg!(target_os = "macos") && label == CLIPBOARD_WINDOW_LABEL
}

pub fn hide_window(app_handle: &AppHandle, label: &str) -> Result<()> {
    // 隐藏前保存任意窗口的实时几何：移动与缩放都在这里落盘，下次显示/启动可恢复。
    if let Err(err) = state::save_window_state(app_handle, label) {
        log::warn!("save window state on hide failed for {label}: {err}");
    }

    if label == CLIPBOARD_WINDOW_LABEL {
        preview::suppress_for_clipboard_hide(app_handle);
        #[cfg(target_os = "macos")]
        let companion_result = macos::hide_window(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL);
        #[cfg(target_os = "windows")]
        let companion_result = windows::hide_window(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL);
        if companion_result.is_ok() {
            emit_visibility(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL, false);
            lifecycle::on_hidden(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL, "companion-hide");
        }
        set_open_side_panels(Vec::new());
        emit_side_panels_state(app_handle);
    }

    #[cfg(target_os = "macos")]
    let result = macos::hide_window(app_handle, label);
    #[cfg(target_os = "windows")]
    let result = windows::hide_window(app_handle, label);
    if result.is_ok() {
        emit_visibility(app_handle, label, false);
        lifecycle::on_hidden(app_handle, label, "hide");
    }
    result
}

pub fn toggle_window(app_handle: &AppHandle, label: &str) -> Result<()> {
    // 已销毁的按需窗口（如空闲超时后的 preference）取不到实例，视为不可见 → 走 show 重建。
    let visible = app_handle
        .get_webview_window(label)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide_window(app_handle, label)
    } else {
        show_window(app_handle, label)
    }
}

pub fn show_taskbar_icon(app_handle: &AppHandle, visible: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    return macos::show_taskbar_icon(app_handle, visible);
    #[cfg(target_os = "windows")]
    return windows::show_taskbar_icon(app_handle, visible);
}

pub fn position_window(app_handle: &AppHandle, label: &str, pos: WindowPosition) -> Result<()> {
    let window = get_window(app_handle, label)?;
    position::position_window(&window, pos)
}

/// 剪贴板窗口显示前按设置应用窗口定位策略。
/// 始终先调用 `restore_window_state` 恢复尺寸与合法位置（含越界 fallback）；
/// 非 Remember 策略再由 `position_window` 覆盖位置。
/// 平台 `show_window` 需要在主线程闭包里调用，避免 set_position 与 show 异步交错产生闪烁。
fn apply_clipboard_window_layout(app_handle: &AppHandle) -> Result<()> {
    let Some(store) = app_handle.try_state::<SettingsStore>() else {
        return Ok(());
    };
    let snap = store.snapshot();
    let position = snap.clipboard.window.position;

    let _ = state::restore_window_state(app_handle, CLIPBOARD_WINDOW_LABEL)?;

    if matches!(position, WindowPosition::Remember) {
        return Ok(());
    }

    let window = get_window(app_handle, CLIPBOARD_WINDOW_LABEL)?;
    position::position_window(&window, position)
}

/// 置顶面板使用固定逻辑宽度，并按两个 WebView 的“可见内区”而不是外窗阴影边界对齐。
/// Windows 无边框透明窗仍可能有 DWM 不可见边距：若按 outer_size/outer_position 拼接，
/// 会留下中间灰缝，且把主窗 outer height 当作右窗 inner height 时会导致右窗越同步越高。
/// 这里显式补偿 inner/outer inset：内容边缘零间隙、顶部/底部严格对齐。
pub fn sync_pinned_panel_layout(app_handle: &AppHandle) -> Result<()> {
    use tauri::{PhysicalPosition, PhysicalSize};

    const SIDE_PANEL_MAX_WIDTH_LOGICAL: f64 = 360.0;
    const SIDE_PANEL_MIN_WIDTH_LOGICAL: f64 = 240.0;

    let main = get_window(app_handle, CLIPBOARD_WINDOW_LABEL)?;
    let pinned = get_window(app_handle, CLIPBOARD_PINNED_WINDOW_LABEL)?;
    let panel_count = clipboard_side_panels_state().open.len().max(1) as u32;

    let main_outer_position = main.outer_position().map_err(|err| anyhow::anyhow!(err))?;
    let main_inner_position = main.inner_position().map_err(|err| anyhow::anyhow!(err))?;
    let main_inner_size = main.inner_size().map_err(|err| anyhow::anyhow!(err))?;
    let main_inset_x = main_inner_position.x - main_outer_position.x;

    let scale = main.scale_factor().map_err(|err| anyhow::anyhow!(err))?;

    // 读取右窗自身的 DWM 内外边距，用它把“可见内容左上角”对齐到主窗内容右上角。
    let pinned_outer_position = pinned
        .outer_position()
        .map_err(|err| anyhow::anyhow!(err))?;
    let pinned_inner_position = pinned
        .inner_position()
        .map_err(|err| anyhow::anyhow!(err))?;
    let pinned_inset_x = pinned_inner_position.x - pinned_outer_position.x;
    let pinned_inset_y = pinned_inner_position.y - pinned_outer_position.y;

    let monitor = main
        .current_monitor()
        .map_err(|err| anyhow::anyhow!(err))?
        .or_else(|| main.primary_monitor().ok().flatten());

    let preferred_main_inner_x = main_inner_position.x;
    let main_inner_y = main_inner_position.y;

    let (main_inner_x, pinned_width) = if let Some(monitor) = monitor {
        let monitor_position = monitor.position();
        let monitor_size = monitor.size();
        let monitor_left = monitor_position.x;
        let monitor_right = monitor_left + monitor_size.width as i32;
        let available_side_width =
            (monitor_size.width as i32 - main_inner_size.width as i32).max(1);
        let per_panel_available = available_side_width / panel_count as i32;
        let min_panel_width = (SIDE_PANEL_MIN_WIDTH_LOGICAL * scale).round().max(1.0) as i32;
        let max_panel_width = (SIDE_PANEL_MAX_WIDTH_LOGICAL * scale).round().max(1.0) as i32;
        let per_panel_width = per_panel_available.clamp(min_panel_width, max_panel_width);
        let pinned_width = (per_panel_width * panel_count as i32).max(1) as u32;
        let pair_inner_width = main_inner_size.width as i32 + pinned_width as i32;
        let main_inner_x = preferred_main_inner_x
            .min(monitor_right - pair_inner_width)
            .max(monitor_left);

        (main_inner_x, pinned_width)
    } else {
        let per_panel_width = (SIDE_PANEL_MAX_WIDTH_LOGICAL * scale).round().max(1.0) as u32;
        (
            preferred_main_inner_x,
            per_panel_width.saturating_mul(panel_count),
        )
    };

    // set_size 接收的是窗口内区尺寸，因此高度必须跟 main.inner_size 对齐。
    pinned
        .set_size(PhysicalSize::new(pinned_width, main_inner_size.height))
        .map_err(|err| anyhow::anyhow!(err))?;

    // 主窗只在横向需要让位时平移；保持其当前可见顶部不变。
    let main_outer_x = main_inner_x - main_inset_x;
    if main_outer_x != main_outer_position.x {
        main.set_position(PhysicalPosition::new(main_outer_x, main_outer_position.y))
            .map_err(|err| anyhow::anyhow!(err))?;
    }

    // 右窗的可见内区左边缘 = 主窗可见内区右边缘；可见顶部完全一致。
    let pinned_outer_x = main_inner_x + main_inner_size.width as i32 - pinned_inset_x;
    let pinned_outer_y = main_inner_y - pinned_inset_y;
    pinned
        .set_position(PhysicalPosition::new(pinned_outer_x, pinned_outer_y))
        .map_err(|err| anyhow::anyhow!(err))?;

    Ok(())
}

/// 保存当前所有窗口的几何信息。供应用退出（`RunEvent::ExitRequested`）时调用，
/// 覆盖「调整大小后不关窗直接退出」这一隐藏/关闭都漏掉的场景。
pub fn save_all_window_states(app_handle: &AppHandle) {
    for label in app_handle.webview_windows().into_keys() {
        if let Err(err) = state::save_window_state(app_handle, &label) {
            log::warn!("save window state on exit failed for {label}: {err}");
        }
    }
}

/// 处理窗口关闭请求，让应用常驻后台（系统托盘）。
/// 返回 `true` 表示已拦截关闭，调用方需 `api.prevent_close()`。
///
/// 引导窗口属于强制流程，关闭请求只拦截不隐藏；其它窗口的关闭按钮统一 hide，不直接销毁。
/// `DestroyWhenIdle` 窗口在 hide 触发的 `on_hidden` 里启动空闲计时器，超时后才由生命周期
/// 管理器 `destroy`，故无需在 close 路径区分销毁分支。
pub fn intercept_close_request(window: &Window) -> bool {
    if window.label() == ONBOARDING_WINDOW_LABEL {
        return true;
    }

    // 关闭按钮不走 `hide_window`，需在此单独保存几何，否则 preference 的移动/缩放会丢失。
    if let Err(err) = state::save_window_state(window.app_handle(), window.label()) {
        log::warn!(
            "save window state on close failed for {}: {err}",
            window.label()
        );
    }

    if let Err(err) = window.hide() {
        log::error!("hide window on close failed: {err:?}");
    } else {
        emit_visibility(window.app_handle(), window.label(), false);
        lifecycle::on_hidden(window.app_handle(), window.label(), "close");
    }
    true
}

/// 按需重建 preference 窗口。preference 不再由 Tauri 配置预创建（改为 `DestroyWhenIdle`），
/// 故所有选项必须在此用 builder 完整复刻原 `tauri.conf.json` 声明，否则重建后行为漂移。
///
/// 建窗后保持 `visible: false`：由 [`show_window`] 统一走恢复几何 + 平台 show 流程，
/// 与其它窗口的显示路径一致。
pub fn build_preference_window(app_handle: &AppHandle) -> Result<()> {
    if app_handle
        .get_webview_window(PREFERENCE_WINDOW_LABEL)
        .is_some()
    {
        return Ok(());
    }

    let builder = WebviewWindowBuilder::new(
        app_handle,
        PREFERENCE_WINDOW_LABEL,
        WebviewUrl::App("index.html/#/preference".into()),
    )
    .title("EcoPaste Preference")
    .inner_size(960.0, 600.0)
    .min_inner_size(960.0, 600.0)
    .center()
    .maximizable(false)
    .skip_taskbar(true)
    .accept_first_mouse(true)
    .disable_drag_drop_handler()
    .visible(false);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    builder
        .build()
        .map_err(|err| anyhow::anyhow!("build preference window: {err}"))?;

    Ok(())
}

/// 按需创建软件更新窗口。更新流程由 Rust updater 命令驱动，窗口只负责渲染状态。
pub fn build_update_window(app_handle: &AppHandle) -> Result<()> {
    if app_handle.get_webview_window(UPDATE_WINDOW_LABEL).is_some() {
        return Ok(());
    }

    let builder = WebviewWindowBuilder::new(
        app_handle,
        UPDATE_WINDOW_LABEL,
        WebviewUrl::App("index.html/#/update".into()),
    )
    .title("EcoPaste Update")
    .inner_size(520.0, 230.0)
    .min_inner_size(520.0, 230.0)
    .center()
    .maximizable(false)
    .resizable(false)
    .skip_taskbar(true)
    .accept_first_mouse(true)
    .disable_drag_drop_handler()
    .decorations(true)
    .transparent(false)
    .visible(false);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    builder
        .build()
        .map_err(|err| anyhow::anyhow!("build update window: {err}"))?;

    Ok(())
}

/// 按需创建首次启动引导窗口。引导窗口始终无边框、深色 UI、打开时居中。
pub fn build_onboarding_window(app_handle: &AppHandle) -> Result<()> {
    if app_handle
        .get_webview_window(ONBOARDING_WINDOW_LABEL)
        .is_some()
    {
        return Ok(());
    }

    WebviewWindowBuilder::new(
        app_handle,
        ONBOARDING_WINDOW_LABEL,
        WebviewUrl::App("index.html/#/onboarding".into()),
    )
    .title("EcoPaste Onboarding")
    .inner_size(900.0, 600.0)
    .center()
    .resizable(false)
    .maximizable(false)
    .decorations(false)
    .transparent(true)
    .accept_first_mouse(true)
    .disable_drag_drop_handler()
    .visible(false)
    .build()
    .map_err(|err| anyhow::anyhow!("build onboarding window: {err}"))?;

    Ok(())
}

/// 创建并显示首次启动引导窗口。
pub fn open_onboarding(app_handle: &AppHandle) -> Result<()> {
    if app_handle
        .get_webview_window(ONBOARDING_WINDOW_LABEL)
        .is_none()
    {
        build_onboarding_window(app_handle)?;
    }

    show_window(app_handle, ONBOARDING_WINDOW_LABEL)
}

/// 打开偏好窗口并定位到指定设置项。
///
/// 偏好窗口存活时直接 emit 高亮事件；已空闲销毁时先把目标存入 pending slot，再 show
/// 触发重建——前端重建后经 [`take_pending_preference_highlight`] 主动拉取，规避
/// 「重建异步、push 丢失」竞态。所有「打开偏好并跳转某设置项」的入口都应走这里，
/// 不要在前端 `show_window` 后直接 `emitTo`。
pub fn open_preference_with_highlight(app_handle: &AppHandle, setting_id: String) -> Result<()> {
    let exists = app_handle
        .get_webview_window(PREFERENCE_WINDOW_LABEL)
        .is_some();

    if !exists {
        set_pending_preference_highlight(setting_id.clone());
    }

    show_window(app_handle, PREFERENCE_WINDOW_LABEL)?;

    if exists {
        app_handle
            .emit_to(
                PREFERENCE_WINDOW_LABEL,
                PREFERENCE_HIGHLIGHT_EVENT,
                PreferenceHighlightPayload { setting_id },
            )
            .map_err(|err| anyhow::anyhow!("emit preference highlight: {err}"))?;
    }

    Ok(())
}

/// 存入待定位的高亮目标，覆盖旧值（仅保留最近一次）。
fn set_pending_preference_highlight(setting_id: String) {
    let mut guard = PENDING_PREFERENCE_HIGHLIGHT
        .lock()
        .unwrap_or_else(|poisoned| {
            log::error!("pending preference highlight mutex poisoned on set, recovering");
            poisoned.into_inner()
        });
    *guard = Some(setting_id);
}

/// 取走并清空待定位的高亮目标，供偏好窗口重建后首屏拉取。
pub fn take_pending_preference_highlight() -> Option<String> {
    let mut guard = PENDING_PREFERENCE_HIGHLIGHT
        .lock()
        .unwrap_or_else(|poisoned| {
            log::error!("pending preference highlight mutex poisoned on take, recovering");
            poisoned.into_inner()
        });
    guard.take()
}
