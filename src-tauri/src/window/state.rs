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
/// 与 tauri.conf.json 的 minHeight=600（逻辑像素）一致。
/// 保存过的物理像素高度不能在 Windows DPI 提升后缩成更小的逻辑高度。
const MIN_CLIPBOARD_HEIGHT_LOGICAL: f64 = 600.0;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
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
    let width = restored_width_with_main_floor(label, state.width, scale);
    if width != state.width {
        log::warn!(
            "normalize saved clipboard window width from {} to {} physical px at DPI scale {}",
            state.width,
            width,
            scale
        );
    }
    let height = restored_height_with_main_floor(label, state.height, scale);
    if height != state.height {
        log::warn!(
            "normalize saved clipboard window height from {} to {} physical px at DPI scale {}",
            state.height,
            height,
            scale
        );
    }
    window
        .set_size(PhysicalSize::new(width, height))
        .map_err(|e| anyhow::anyhow!(e))?;

    let monitors = window
        .available_monitors()
        .map_err(|e| anyhow::anyhow!(e))?;
    let on_screen = monitors.iter().any(|m| {
        let mx = m.position().x;
        let my = m.position().y;
        let mw = m.size().width as i32;
        let mh = m.size().height as i32;
        state.x >= mx && state.x < mx + mw && state.y >= my && state.y < my + mh
    });

    if on_screen {
        window
            .set_position(PhysicalPosition::new(state.x, state.y))
            .map_err(|e| anyhow::anyhow!(e))?;
    } else {
        super::position::center_on_cursor_monitor(&window)?;
    }

    Ok(true)
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
        assert_eq!(restored_height_with_main_floor("clipboard", 600, 1.5), 900);
        assert_eq!(
            restored_height_with_main_floor("clipboard", 600, 2.25),
            1350
        );
        assert_eq!(restored_height_with_main_floor("clipboard", 400, 1.0), 600);
        assert_eq!(
            restored_height_with_main_floor("clipboard", 1150, 1.5),
            1150
        );
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
