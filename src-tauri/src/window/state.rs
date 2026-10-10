use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize};

use crate::core::Result;

const STATE_FILENAME: &str = "window-state.json";

/// 与 tauri.conf.json 中主剪贴板窗口的 minWidth=360（逻辑像素）一致。
/// 旧窗口状态按物理像素保存，跨 DPI / 版本恢复时不能压缩主窗口。
const MIN_CLIPBOARD_WIDTH_LOGICAL: f64 = 360.0;
/// 主窗允许比旧版强制的 600 DIP 更紧凑，保留用户自行调整尺寸的能力。
const MIN_CLIPBOARD_HEIGHT_LOGICAL: f64 = 420.0;
/// Old persisted heights were forced to at least 600 DIP. Normalize them
/// *once* to a compact 500 DIP default; don't shrink intentional resizes on
/// later launches.
const COMPACT_CLIPBOARD_HEIGHT_LOGICAL: f64 = 500.0;
const WINDOW_SIZE_VERSION: u8 = 1;
/// Reserve a little space for the taskbar when checking a saved screen rect.
const BOTTOM_SAFE_MARGIN_LOGICAL: f64 = 48.0;

fn restored_width_with_main_floor(label: &str, saved_physical: u32, scale: f64) -> u32 {
    if label != super::CLIPBOARD_WINDOW_LABEL || !scale.is_finite() || scale <= 0.0 {
        return saved_physical;
    }
    let physical_floor = (MIN_CLIPBOARD_WIDTH_LOGICAL * scale).ceil() as u32;
    saved_physical.max(physical_floor.max(1))
}

fn restored_height_with_main_floor(label: &str, saved_physical: u32, scale: f64) -> u32 {
    if label != super::CLIPBOARD_WINDOW_LABEL || !scale.is_finite() || scale <= 0.0 {
        return saved_physical;
    }
    let physical_floor = (MIN_CLIPBOARD_HEIGHT_LOGICAL * scale).ceil() as u32;
    saved_physical.max(physical_floor.max(1))
}

/// Previous builds wrote state.height with a 600-DIP floor, producing 900
/// physical pixels at 150% DPI. When that saved geometry is first opened by
/// the compact build, it should not force an oversized or clipped main window.
/// After the first save, version=1 and the user's actual resize is respected.
fn restored_height_by_state_version(
    label: &str,
    saved_physical: u32,
    scale: f64,
    sizing_version: u8,
) -> u32 {
    if label != super::CLIPBOARD_WINDOW_LABEL || sizing_version >= WINDOW_SIZE_VERSION
        || !scale.is_finite() || scale <= 0.0
    {
        return restored_height_with_main_floor(label, saved_physical, scale);
    }
    let compact_height = (COMPACT_CLIPBOARD_HEIGHT_LOGICAL * scale).round() as u32;
    restored_height_with_main_floor(label, saved_physical.min(compact_height), scale)
}

/// Restrict a saved window rect to a monitor. Checking only that its top-left
/// corner is on-screen is insufficient: the bottom can still extend beyond
/// the usable screen area. This is physical-pixel math, not logical CSS math.
fn fit_saved_rect(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    monitor_x: i32,
    monitor_y: i32,
    monitor_width: u32,
    monitor_height: u32,
    scale: f64,
) -> (i32, i32, u32, u32) {
    let margin = if scale.is_finite() && scale > 0.0 {
        (BOTTOM_SAFE_MARGIN_LOGICAL * scale).ceil() as u32
    } else {
        0
    };
    let usable_h = monitor_height.saturating_sub(margin).max(1);
    let actual_h = height.min(usable_h);
    let actual_w = width.min(monitor_width.max(1));
    let max_x = (i64::from(monitor_x) + i64::from(monitor_width) - i64::from(actual_w))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let max_y = (i64::from(monitor_y) + i64::from(usable_h) - i64::from(actual_h))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    (
        x.clamp(monitor_x, max_x.max(monitor_x)),
        y.clamp(monitor_y, max_y.max(monitor_y)),
        actual_w,
        actual_h,
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// 0 means a legacy state saved with the 600-DIP forced floor.
    #[serde(default)]
    pub sizing_version: u8,
}

pub struct WindowStateStore {
    path: RwLock<PathBuf>,
    states: Mutex<HashMap<String, WindowState>>,
}

impl WindowStateStore {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let dir = crate::core::paths::state_dir(app)?;

        fs::create_dir_all(&dir).with_context(|| format!("failed to create dir at {dir:?}"))?;

        let path = dir.join(STATE_FILENAME);

        let states = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                    log::warn!("failed to parse window state at {path:?}, using defaults: {e}");
                    HashMap::new()
                }),
                Err(e) => {
                    log::warn!("failed to read window state at {path:?}, using defaults: {e}");
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        log::info!("window state store ready at {path:?}");
        Ok(Self {
            path: RwLock::new(path),
            states: Mutex::new(states),
        })
    }

    pub fn save(&self, label: &str, state: WindowState) -> Result<()> {
        let mut states = self.states.lock().unwrap_or_else(|poisoned| {
            log::error!("window state mutex poisoned on save, recovering");
            poisoned.into_inner()
        });
        states.insert(label.to_owned(), state);
        let json =
            serde_json::to_string_pretty(&*states).context("failed to serialize window states")?;
        let path = self.path();
        fs::write(&path, json)
            .with_context(|| format!("failed to write window state to {:?}", path))?;
        Ok(())
    }

    pub fn get(&self, label: &str) -> Option<WindowState> {
        let states = self.states.lock().unwrap_or_else(|poisoned| {
            log::error!("window state mutex poisoned on get, recovering");
            poisoned.into_inner()
        });
        states.get(label).cloned()
    }

    /// 数据目录热切换后重新绑定窗口状态文件，并重新读取新目录里的状态。
    pub fn rebase(&self, app: &AppHandle) -> Result<()> {
        let dir = crate::core::paths::state_dir(app)?;
        fs::create_dir_all(&dir).with_context(|| format!("failed to create dir at {dir:?}"))?;
        let path = dir.join(STATE_FILENAME);
        let next_states = load_states(&path);

        *self.path.write().expect("window state path poisoned") = path;
        *self.states.lock().unwrap_or_else(|poisoned| {
            log::error!("window state mutex poisoned on rebase, recovering");
            poisoned.into_inner()
        }) = next_states;
        Ok(())
    }

    fn path(&self) -> PathBuf {
        self.path
            .read()
            .expect("window state path poisoned")
            .clone()
    }
}

fn load_states(path: &PathBuf) -> HashMap<String, WindowState> {
    if !path.exists() {
        return HashMap::new();
    }

    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            log::warn!("failed to parse window state at {path:?}, using defaults: {e}");
            HashMap::new()
        }),
        Err(e) => {
            log::warn!("failed to read window state at {path:?}, using defaults: {e}");
            HashMap::new()
        }
    }
}

/// 读取窗口当前的实时几何（`outer_position` + `inner_size`）并落盘。
/// 在隐藏 / 关闭 / 退出等可靠生命周期点调用即可捕获用户的移动与缩放。
pub fn save_window_state(app: &AppHandle, label: &str) -> Result<()> {
    let window = app
        .get_webview_window(label)
        .ok_or_else(|| anyhow::anyhow!("window not found: {label}"))?;

    let pos = window.outer_position().map_err(|e| anyhow::anyhow!(e))?;
    let size = window.inner_size().map_err(|e| anyhow::anyhow!(e))?;

    let store = app.state::<WindowStateStore>();
    store.save(
        label,
        WindowState {
            x: pos.x,
            y: pos.y,
            width: size.width,
            height: size.height,
            sizing_version: WINDOW_SIZE_VERSION,
        },
    )
}

/// 恢复窗口的尺寸 + 位置。无存档返回 `Ok(false)`。
///
/// 始终恢复存档尺寸；位置在恢复前校验是否仍位于可用显示器范围内：
/// 若上次所在显示器已被拔出，则 fallback 到当前光标所在屏幕的中心，
/// 避免窗口出现在不可见的虚拟坐标区域。
pub fn restore_window_state(app: &AppHandle, label: &str) -> Result<bool> {
    let store = app.state::<WindowStateStore>();
    let Some(state) = store.get(label) else {
        return Ok(false);
    };

    let window = app
        .get_webview_window(label)
        .ok_or_else(|| anyhow::anyhow!("window not found: {label}"))?;

    let scale = window.scale_factor().map_err(|e| anyhow::anyhow!(e))?;
    let mut width = restored_width_with_main_floor(label, state.width, scale);
    let mut height =
        restored_height_by_state_version(label, state.height, scale, state.sizing_version);
    let monitors = window
        .available_monitors()
        .map_err(|e| anyhow::anyhow!(e))?;
    let saved_monitor = monitors.iter().find(|m| {
        let mx = i64::from(m.position().x);
        let my = i64::from(m.position().y);
        let x = i64::from(state.x);
        let y = i64::from(state.y);
        x >= mx
            && x < mx + i64::from(m.size().width)
            && y >= my
            && y < my + i64::from(m.size().height)
    });
    let mut actual_x = state.x;
    let mut actual_y = state.y;
    if let Some(monitor) = saved_monitor {
        if label == super::CLIPBOARD_WINDOW_LABEL {
            (actual_x, actual_y, width, height) = fit_saved_rect(
                state.x,
                state.y,
                width,
                height,
                monitor.position().x,
                monitor.position().y,
                monitor.size().width,
                monitor.size().height,
                scale,
            );
        }
    }
    if width != state.width || height != state.height {
        log::info!(
            "restore clipboard window geometry: {}x{} -> {}x{} physical px, DPI scale {}, state version {}",
            state.width,
            state.height,
            width,
            height,
            scale,
            state.sizing_version
        );
    }
    window
        .set_size(PhysicalSize::new(width, height))
        .map_err(|e| anyhow::anyhow!(e))?;

    if saved_monitor.is_some() {
        window
            .set_position(PhysicalPosition::new(actual_x, actual_y))
            .map_err(|e| anyhow::anyhow!(e))?;
    } else {
        super::position::center_on_cursor_monitor(&window)?;
        let actual = window
            .outer_position()
            .map_err(|e| anyhow::anyhow!(e))?;
        actual_x = actual.x;
        actual_y = actual.y;
    }

    // Persist the one-time compact migration immediately. Subsequent user
    // resizes get version=1 and must not be auto-shrunk again.
    if label == super::CLIPBOARD_WINDOW_LABEL
        && (state.sizing_version < WINDOW_SIZE_VERSION
            || width != state.width
            || height != state.height
            || actual_x != state.x
            || actual_y != state.y)
    {
        let actual_size = window.inner_size().map_err(|e| anyhow::anyhow!(e))?;
        store.save(
            label,
            WindowState {
                x: actual_x,
                y: actual_y,
                width: actual_size.width,
                height: actual_size.height,
                sizing_version: WINDOW_SIZE_VERSION,
            },
        )?;
    }

    Ok(true)
}

#[cfg(test)]
mod compact_window_restoration_tests {
    use super::{fit_saved_rect, restored_height_by_state_version, WindowState};

    #[test]
    fn old_forced_height_is_compacted_once_by_dpi() {
        assert_eq!(restored_height_by_state_version("clipboard", 600, 1.0, 0), 500);
        assert_eq!(restored_height_by_state_version("clipboard", 900, 1.5, 0), 750);
        assert_eq!(restored_height_by_state_version("clipboard", 900, 1.5, 1), 900);
        assert_eq!(restored_height_by_state_version("clipboard-pinned", 900, 1.5, 0), 900);
    }

    #[test]
    fn oversized_saved_position_and_height_stay_on_monitor() {
        // Screenshot-like 2048x1222 at 150%; the taskbar safety area is 72 px.
        assert_eq!(
            fit_saved_rect(1050, 376, 540, 900, 0, 0, 2048, 1222, 1.5),
            (1050, 250, 540, 900),
        );
        assert_eq!(
            fit_saved_rect(1050, 376, 540, 750, 0, 0, 2048, 1222, 1.5),
            (1050, 376, 540, 750),
        );
    }

    #[test]
    fn legacy_saved_state_deserializes_without_version_field() {
        let old: WindowState = serde_json::from_str(
            r#"{"x":1050,"y":376,"width":540,"height":900}"#,
        )
        .unwrap();
        assert_eq!(old.sizing_version, 0);
    }
}

#[cfg(test)]
mod saved_clipboard_width_tests {
    use super::{restored_height_with_main_floor, restored_width_with_main_floor};

    #[test]
    fn old_physical_width_cannot_shrink_clipboard_below_original_logical_minimum() {
        assert_eq!(restored_width_with_main_floor("clipboard", 360, 1.0), 360);
        assert_eq!(restored_width_with_main_floor("clipboard", 360, 1.5), 540);
        assert_eq!(restored_width_with_main_floor("clipboard", 360, 2.25), 810);
        assert_eq!(restored_width_with_main_floor("clipboard", 970, 2.25), 970);
    }

    #[test]
    fn restore_clipboard_height_in_logical_pixels_across_dpi_scales() {
        assert_eq!(restored_height_with_main_floor("clipboard", 600, 1.0), 600);
        assert_eq!(restored_height_with_main_floor("clipboard", 600, 1.5), 630);
        assert_eq!(restored_height_with_main_floor("clipboard", 600, 2.25), 945);
        assert_eq!(restored_height_with_main_floor("clipboard", 400, 1.0), 420);
        assert_eq!(restored_height_with_main_floor("clipboard", 1150, 1.5), 1150);
    }

    #[test]
    fn other_windows_preserve_saved_height_and_invalid_scale_does_not_crash() {
        assert_eq!(
            restored_height_with_main_floor("clipboard-pinned", 400, 1.5),
            400
        );
        assert_eq!(
            restored_height_with_main_floor("clipboard-side-left", 400, 1.5),
            400
        );
        assert_eq!(
            restored_height_with_main_floor("clipboard", 400, f64::NAN),
            400
        );
        assert_eq!(restored_height_with_main_floor("clipboard", 400, 0.0), 400);
    }

    #[test]
    fn other_windows_preserve_saved_width_and_invalid_scale_does_not_crash() {
        assert_eq!(
            restored_width_with_main_floor("clipboard-pinned", 240, 2.25),
            240
        );
        assert_eq!(
            restored_width_with_main_floor("clipboard", 360, f64::NAN),
            360
        );
        assert_eq!(restored_width_with_main_floor("clipboard", 360, 0.0), 360);
    }
}
