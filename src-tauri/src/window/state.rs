use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::settings::{SettingsStore, WindowPosition};

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
// Version 1 only normalized legacy height. It preserved oversized saved widths
// and stopped compacting a window if a hide/exit had already stored version=1.
// Version 2 migrates both axes to the configured compact defaults exactly once.
const WINDOW_SIZE_VERSION: u8 = 3;
// Preserve v2 user resizes: version 3 adds display-aware preferences only.
const LEGACY_GEOMETRY_VERSION: u8 = 2;
/// Reserve a little space for the taskbar when checking a saved screen rect.
const BOTTOM_SAFE_MARGIN_LOGICAL: f64 = 8.0;

fn restored_width_with_main_floor(label: &str, saved_physical: u32, scale: f64) -> u32 {
    if label != super::CLIPBOARD_WINDOW_LABEL || !scale.is_finite() || scale <= 0.0 {
        return saved_physical;
    }
    let physical_floor = (MIN_CLIPBOARD_WIDTH_LOGICAL * scale).ceil() as u32;
    saved_physical.max(physical_floor.max(1))
}

/// A previous build accepted any persisted width above 360 logical pixels.
/// That old width survives default changes, so the main window can appear
/// much wider than its right companion. During the v2 migration, restore
/// precisely the ORIGINAL main-window default width of 360 logical pixels.
/// Only legacy states are reset. New manual resizes remain persistent.
fn restored_width_by_state_version(
    label: &str,
    saved_physical: u32,
    scale: f64,
    sizing_version: u8,
) -> u32 {
    if label != super::CLIPBOARD_WINDOW_LABEL
        || sizing_version >= LEGACY_GEOMETRY_VERSION
        || !scale.is_finite()
        || scale <= 0.0
    {
        return restored_width_with_main_floor(label, saved_physical, scale);
    }
    let compact_width = (MIN_CLIPBOARD_WIDTH_LOGICAL * scale).ceil() as u32;
    restored_width_with_main_floor(label, saved_physical.min(compact_width), scale)
}

fn restored_height_with_main_floor(label: &str, saved_physical: u32, scale: f64) -> u32 {
    if label != super::CLIPBOARD_WINDOW_LABEL || !scale.is_finite() || scale <= 0.0 {
        return saved_physical;
    }
    let physical_floor = (MIN_CLIPBOARD_HEIGHT_LOGICAL * scale).ceil() as u32;
    saved_physical.max(physical_floor.max(1))
}

/// Previous builds wrote state.height with a 600-DIP floor, and even the
/// initial compact-height migration could be bypassed by an already-versioned
/// saved state. Migrate versions 0 and 1 to at most 500 logical pixels.
/// Once stored as version 2, preserve the user's subsequent explicit resize.
fn restored_height_by_state_version(
    label: &str,
    saved_physical: u32,
    scale: f64,
    sizing_version: u8,
) -> u32 {
    if label != super::CLIPBOARD_WINDOW_LABEL
        || sizing_version >= LEGACY_GEOMETRY_VERSION
        || !scale.is_finite()
        || scale <= 0.0
    {
        return restored_height_with_main_floor(label, saved_physical, scale);
    }
    let compact_height = (COMPACT_CLIPBOARD_HEIGHT_LOGICAL * scale).round() as u32;
    restored_height_with_main_floor(label, saved_physical.min(compact_height), scale)
}

/// Restrict a saved window rect to a monitor. Checking only that its top-left
/// corner is on-screen is insufficient: the bottom can still extend beyond
/// the usable screen area. This is physical-pixel math, not logical CSS math.
/// Coordinate pairs and physical dimensions are deliberately explicit so
/// callers cannot accidentally mix them with logical Tauri dimensions.
#[allow(clippy::too_many_arguments)]
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

/// Per-monitor usable Windows desktop rect (taskbar already excluded).
#[derive(Clone, Copy, Debug)]
struct DisplayWorkArea {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
}

impl DisplayWorkArea {
    fn from_monitor(monitor: &tauri::Monitor) -> Self {
        let work = monitor.work_area();
        let (x, y, width, height) = if work.size.width > 0 && work.size.height > 0 {
            (work.position.x, work.position.y, work.size.width, work.size.height)
        } else {
            (
                monitor.position().x,
                monitor.position().y,
                monitor.size().width,
                monitor.size().height,
            )
        };
        Self { x, y, width, height, scale: monitor.scale_factor() }
    }
}

fn position_on_monitor(m: &tauri::Monitor, x: f64, y: f64) -> bool {
    let left = f64::from(m.position().x);
    let top = f64::from(m.position().y);
    x >= left && x < left + f64::from(m.size().width)
        && y >= top && y < top + f64::from(m.size().height)
}

/// If an old position is offscreen after UU changes desktop resolution,
/// select the monitor containing the cursor instead of skipping clipping.
fn choose_work_area(
    window: &WebviewWindow,
    saved_position: Option<(i32, i32)>,
    prefer_cursor: bool,
) -> Result<Option<DisplayWorkArea>> {
    let monitors = window.available_monitors().map_err(|e| anyhow::anyhow!(e))?;
    let saved = saved_position.and_then(|(x, y)| {
        monitors.iter().find(|m| position_on_monitor(m, f64::from(x), f64::from(y)))
    });
    let cursor = window.cursor_position().ok().and_then(|p| {
        monitors.iter().find(|m| position_on_monitor(m, p.x, p.y))
    });
    let selected = if prefer_cursor { cursor.or(saved) } else { saved.or(cursor) };
    if let Some(monitor) = selected {
        return Ok(Some(DisplayWorkArea::from_monitor(monitor)));
    }
    Ok(window
        .current_monitor()
        .map_err(|e| anyhow::anyhow!(e))?
        .or(window.primary_monitor().map_err(|e| anyhow::anyhow!(e))?)
        .as_ref()
        .map(DisplayWorkArea::from_monitor))
}

fn valid_preference(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x > 0.0 && *x < 20_000.0)
}

/// Physical fit to current usable work area, including HiDPI remote sessions
/// where the configured logical minHeight cannot physically fit the desktop.
fn effective_main_size(
    preferred_width: f64,
    preferred_height: f64,
    area: DisplayWorkArea,
    inset_width: u32,
    inset_height: u32,
) -> (PhysicalSize<u32>, PhysicalSize<u32>) {
    let scale = if area.scale.is_finite() && area.scale > 0.0 { area.scale } else { 1.0 };
    let safe = (BOTTOM_SAFE_MARGIN_LOGICAL * scale).ceil() as u32;
    let available_width = area.width.saturating_sub(inset_width).saturating_sub(safe).max(1);
    let available_height = area.height.saturating_sub(inset_height).saturating_sub(safe).max(1);
    let min_width = ((MIN_CLIPBOARD_WIDTH_LOGICAL * scale).ceil() as u32)
        .min(available_width).max(1);
    let min_height = ((MIN_CLIPBOARD_HEIGHT_LOGICAL * scale).ceil() as u32)
        .min(available_height).max(1);
    let wanted_width = (preferred_width * scale).round() as u32;
    let wanted_height = (preferred_height * scale).round() as u32;
    (
        PhysicalSize::new(
            wanted_width.clamp(min_width, available_width),
            wanted_height.clamp(min_height, available_height),
        ),
        PhysicalSize::new(min_width, min_height),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// 0/1 = old, 2 = compact main window, 3 = display-aware saved geometry.
    #[serde(default)]
    pub sizing_version: u8,
    #[serde(default)]
    pub preferred_width_logical: Option<f64>,
    #[serde(default)]
    pub preferred_height_logical: Option<f64>,
    #[serde(default)]
    pub saved_scale_factor: Option<f64>,
    #[serde(default)]
    pub auto_fitted_width: Option<u32>,
    #[serde(default)]
    pub auto_fitted_height: Option<u32>,
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
    let scale = window.scale_factor().map_err(|e| anyhow::anyhow!(e))?;
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };

    let store = app.state::<WindowStateStore>();
    let old = store.get(label);
    let (preferred_width_logical, preferred_height_logical) =
        if label == super::CLIPBOARD_WINDOW_LABEL {
            // An automatic fit on a small remote desktop must not overwrite the
            // user's logical preference from a large local monitor.
            let is_auto_fit = old.as_ref().is_some_and(|previous| {
                previous.auto_fitted_width == Some(size.width)
                    && previous.auto_fitted_height == Some(size.height)
            });
            if is_auto_fit {
                (
                    old.as_ref().and_then(|previous| valid_preference(previous.preferred_width_logical)),
                    old.as_ref().and_then(|previous| valid_preference(previous.preferred_height_logical)),
                )
            } else {
                (
                    Some(f64::from(size.width) / scale),
                    Some(f64::from(size.height) / scale),
                )
            }
        } else {
            (None, None)
        };

    store.save(
        label,
        WindowState {
            x: pos.x,
            y: pos.y,
            width: size.width,
            height: size.height,
            sizing_version: WINDOW_SIZE_VERSION,
            preferred_width_logical,
            preferred_height_logical,
            saved_scale_factor: Some(scale),
            auto_fitted_width: Some(size.width),
            auto_fitted_height: Some(size.height),
        },
    )
}

/// On EVERY show use the current monitor's work area, even if the saved
/// position is now off-screen because a remote desktop changed resolution.
/// The logical preferred size persists; only effective physical dimensions
/// are clamped to fit the live monitor.
pub fn restore_window_state(app: &AppHandle, label: &str) -> Result<bool> {
    let store = app.state::<WindowStateStore>();
    let old = store.get(label);
    let window = app
        .get_webview_window(label)
        .ok_or_else(|| anyhow::anyhow!("window not found: {label}"))?;

    if label != super::CLIPBOARD_WINDOW_LABEL {
        let Some(state) = old else { return Ok(false); };
        window
            .set_size(PhysicalSize::new(state.width, state.height))
            .map_err(|e| anyhow::anyhow!(e))?;
        let monitors = window.available_monitors().map_err(|e| anyhow::anyhow!(e))?;
        if monitors.iter().any(|m| position_on_monitor(m, f64::from(state.x), f64::from(state.y))) {
            window
                .set_position(PhysicalPosition::new(state.x, state.y))
                .map_err(|e| anyhow::anyhow!(e))?;
        } else {
            super::position::center_on_cursor_monitor(&window)?;
        }
        return Ok(true);
    }

    let follow_cursor = app.try_state::<SettingsStore>().is_none_or(|settings| {
        !matches!(settings.snapshot().clipboard.window.position, WindowPosition::Remember)
    });
    let saved_position = old.as_ref().map(|state| (state.x, state.y));
    let Some(area) = choose_work_area(&window, saved_position, follow_cursor)? else {
        log::warn!("clipboard auto-fit skipped: no active monitor");
        return Ok(false);
    };
    let scale = if area.scale.is_finite() && area.scale > 0.0 { area.scale } else { 1.0 };

    let (preferred_width, preferred_height) = match old.as_ref() {
        Some(saved) => {
            let saved_scale = valid_preference(saved.saved_scale_factor).unwrap_or(scale);
            if saved.sizing_version < LEGACY_GEOMETRY_VERSION {
                (
                    f64::from(restored_width_by_state_version(
                        label, saved.width, scale, saved.sizing_version,
                    )) / scale,
                    f64::from(restored_height_by_state_version(
                        label, saved.height, scale, saved.sizing_version,
                    )) / scale,
                )
            } else {
                (
                    valid_preference(saved.preferred_width_logical)
                        .unwrap_or(f64::from(saved.width) / saved_scale),
                    valid_preference(saved.preferred_height_logical)
                        .unwrap_or(f64::from(saved.height) / saved_scale),
                )
            }
        }
        None => (MIN_CLIPBOARD_WIDTH_LOGICAL, COMPACT_CLIPBOARD_HEIGHT_LOGICAL),
    };

    let outer = window.outer_size().map_err(|e| anyhow::anyhow!(e))?;
    let inner = window.inner_size().map_err(|e| anyhow::anyhow!(e))?;
    let extra_width = outer.width.saturating_sub(inner.width);
    let extra_height = outer.height.saturating_sub(inner.height);
    let (wanted, adaptive_min) =
        effective_main_size(preferred_width, preferred_height, area, extra_width, extra_height);
    // When a 768px remote screen uses very high DPI, the static 420-DIP
    // minimum would itself exceed the work area. Lower the runtime minimum.
    window.set_min_size(Some(adaptive_min)).map_err(|e| anyhow::anyhow!(e))?;
    if window.inner_size().map_err(|e| anyhow::anyhow!(e))? != wanted {
        window.set_size(wanted).map_err(|e| anyhow::anyhow!(e))?;
    }
    let applied_size = window.inner_size().map_err(|e| anyhow::anyhow!(e))?;
    let (saved_x, saved_y) = saved_position.unwrap_or((area.x, area.y));
    let (x, y, _, _) = fit_saved_rect(
        saved_x, saved_y,
        applied_size.width.saturating_add(extra_width),
        applied_size.height.saturating_add(extra_height),
        area.x, area.y, area.width, area.height, scale,
    );
    let target_pos = PhysicalPosition::new(x, y);
    if window.outer_position().map_err(|e| anyhow::anyhow!(e))? != target_pos {
        window.set_position(target_pos).map_err(|e| anyhow::anyhow!(e))?;
    }

    store.save(
        label,
        WindowState {
            x, y,
            width: applied_size.width,
            height: applied_size.height,
            sizing_version: WINDOW_SIZE_VERSION,
            preferred_width_logical: Some(preferred_width),
            preferred_height_logical: Some(preferred_height),
            saved_scale_factor: Some(scale),
            auto_fitted_width: Some(applied_size.width),
            auto_fitted_height: Some(applied_size.height),
        },
    )?;
    let shrunk = (preferred_width * scale).round() as u32 != applied_size.width
        || (preferred_height * scale).round() as u32 != applied_size.height;
    if shrunk {
        log::info!(
            "clipboard auto-fit to {}x{} workarea at ({},{}), dpi={scale}: preferred {:.0}x{:.0} DIP, effective {}x{} physical",
            area.width, area.height, area.x, area.y,
            preferred_width, preferred_height, applied_size.width, applied_size.height
        );
    }
    Ok(old.is_some())
}

/// While visible, correct genuine off-screen window rects after a monitor
/// change. A normal resize that still fits does not reset the user's size.
pub fn refit_visible_main_if_clipped(app: &AppHandle) -> Result<bool> {
    let Some(window) = app.get_webview_window(super::CLIPBOARD_WINDOW_LABEL) else {
        return Ok(false);
    };
    if !window.is_visible().unwrap_or(false) {
        return Ok(false);
    }
    let pos = window.outer_position().map_err(|e| anyhow::anyhow!(e))?;
    let size = window.outer_size().map_err(|e| anyhow::anyhow!(e))?;
    let Some(area) = choose_work_area(&window, Some((pos.x, pos.y)), false)? else {
        return Ok(false);
    };
    let right = i64::from(pos.x) + i64::from(size.width);
    let bottom = i64::from(pos.y) + i64::from(size.height);
    let clipped = pos.x < area.x || pos.y < area.y
        || right > i64::from(area.x) + i64::from(area.width)
        || bottom > i64::from(area.y) + i64::from(area.height);
    if clipped {
        restore_window_state(app, super::CLIPBOARD_WINDOW_LABEL)?;
    }
    Ok(clipped)
}

#[cfg(test)]
mod compact_window_restoration_tests {
    use super::{
        fit_saved_rect, restored_height_by_state_version, restored_width_by_state_version,
        WindowState,
    };

    #[test]
    fn old_forced_height_is_compacted_once_by_dpi() {
        assert_eq!(
            restored_height_by_state_version("clipboard", 600, 1.0, 0),
            500
        );
        assert_eq!(
            restored_height_by_state_version("clipboard", 900, 1.5, 0),
            750
        );
        assert_eq!(
            restored_height_by_state_version("clipboard", 900, 1.5, 1),
            750
        );
        assert_eq!(
            restored_height_by_state_version("clipboard-pinned", 900, 1.5, 0),
            900
        );
        assert_eq!(
            restored_height_by_state_version("clipboard", 900, 1.5, 2),
            900
        );
    }

    #[test]
    fn v2_resets_oversized_saved_main_width_to_original_default_once() {
        // Screenshot: old saved width is larger than the original default.
        assert_eq!(
            restored_width_by_state_version("clipboard", 548, 1.25, 1),
            450
        );
        assert_eq!(
            restored_width_by_state_version("clipboard", 548, 1.20, 1),
            432
        );
        // The original 360-DIP default at 150% is already 540 physical px.
        assert_eq!(
            restored_width_by_state_version("clipboard", 540, 1.5, 0),
            540
        );
        // A user resize made after v2 must persist, including a larger width.
        assert_eq!(
            restored_width_by_state_version("clipboard", 548, 1.25, 2),
            548
        );
        // Side panel sizes follow their own layout code; do not normalize.
        assert_eq!(
            restored_width_by_state_version("clipboard-pinned", 430, 1.25, 0),
            430
        );
    }

    #[test]
    fn v2_resets_oversized_height_even_when_legacy_marked_version_one() {
        assert_eq!(
            restored_height_by_state_version("clipboard", 904, 1.25, 1),
            625
        );
        assert_eq!(
            restored_height_by_state_version("clipboard", 904, 1.20, 1),
            600
        );
        assert_eq!(
            restored_height_by_state_version("clipboard", 904, 1.25, 2),
            904
        );
        // Do not expand a legitimate already-compact legacy height.
        assert_eq!(
            restored_height_by_state_version("clipboard", 550, 1.25, 1),
            550
        );
    }

    #[test]
    fn oversized_saved_position_and_height_stay_on_monitor() {
        // Screenshot-like 2048x1222 at 150%; the taskbar safety area is 72 px.
        assert_eq!(
            fit_saved_rect(1050, 376, 540, 900, 0, 0, 2048, 1222, 1.5),
            (1050, 310, 540, 900),
        );
        assert_eq!(
            fit_saved_rect(1050, 376, 540, 750, 0, 0, 2048, 1222, 1.5),
            (1050, 376, 540, 750),
        );
    }

    #[test]
    fn remote_1366x768_is_bounded_at_all_common_dpi_scales() {
        use super::{effective_main_size, DisplayWorkArea};
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let area = DisplayWorkArea {
                x: 0, y: 0, width: 1366, height: 728, scale,
            };
            let (fitted, min) = effective_main_size(360.0, 900.0, area, 12, 16);
            assert!(fitted.width + 12 <= area.width);
            assert!(fitted.height + 16 <= area.height);
            assert!(min.width <= fitted.width && min.height <= fitted.height);
        }
    }

    #[test]
    fn tiny_hidpi_remote_relaxes_unfit_static_minimum() {
        use super::{effective_main_size, DisplayWorkArea};
        let area = DisplayWorkArea { x: 0, y: 0, width: 640, height: 430, scale: 2.0 };
        let (fit, min) = effective_main_size(360.0, 500.0, area, 10, 10);
        assert_eq!(fit, min);
        assert!(fit.width + 10 <= 640 && fit.height + 10 <= 430);
    }

    #[test]
    fn fit_handles_old_position_outside_small_remote_workarea() {
        let (x, y, w, h) =
            fit_saved_rect(1700, 800, 540, 800, 0, 0, 1366, 728, 1.25);
        assert!(x >= 0 && y >= 0);
        assert!(x as u32 + w <= 1366);
        assert!(y as u32 + h <= 728);
    }

    #[test]
    fn version_two_resizes_are_not_mistaken_for_legacy_forced_geometry() {
        assert_eq!(restored_width_by_state_version("clipboard", 970, 1.5, 2), 970);
        assert_eq!(restored_height_by_state_version("clipboard", 900, 1.5, 2), 900);
        assert_eq!(restored_height_by_state_version("clipboard", 900, 1.5, 3), 900);
    }

    #[test]
    fn legacy_saved_state_deserializes_without_version_field() {
        let old: WindowState =
            serde_json::from_str(r#"{"x":1050,"y":376,"width":540,"height":900}"#).unwrap();
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
