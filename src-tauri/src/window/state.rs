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

// Preserve the original x/y and physical width/height persistence contract.
// Only two logical preferences and the last saved DPI are needed when moving
// between a local monitor and a smaller remote desktop. The old v0-v3 JSON
// records remain readable; obsolete V3 fields are ignored by serde.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub sizing_version: u8,
    #[serde(default)]
    pub preferred_width_logical: Option<f64>,
    #[serde(default)]
    pub preferred_height_logical: Option<f64>,
    #[serde(default)]
    pub saved_scale_factor: Option<f64>,
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

// Use original Tauri window geometry APIs. The only added policy is to
// keep the existing preferred logical size but fit its effective physical
// dimensions to the CURRENT monitor's usable work area when shown.
const DEFAULT_WIDTH_LOGICAL: f64 = 360.0;
const DEFAULT_HEIGHT_LOGICAL: f64 = 500.0;
const MIN_HEIGHT_LOGICAL: f64 = 420.0;
const FIT_MARGIN_LOGICAL: f64 = 6.0;

#[derive(Debug, Clone, Copy)]
struct WorkArea {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
}

impl WorkArea {
    fn from_monitor(monitor: &tauri::Monitor) -> Self {
        let area = monitor.work_area();
        let (x, y, width, height) = if area.size.width > 0 && area.size.height > 0 {
            (area.position.x, area.position.y, area.size.width, area.size.height)
        } else {
            (
                monitor.position().x,
                monitor.position().y,
                monitor.size().width,
                monitor.size().height,
            )
        };
        let scale = valid_scale(monitor.scale_factor());
        Self { x, y, width, height, scale }
    }
}

fn valid_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 { scale } else { 1.0 }
}

fn valid_preference(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v > 0.0 && *v < 20_000.0)
}

/// The first two compact-size migrations were incomplete. For legacy states
/// without a preferred logical size, normalize to the current defaults ONCE;
/// newer records keep actual manual sizes across monitor and DPI changes.
fn preferred_size(saved: Option<&WindowState>, scale: f64) -> (f64, f64) {
    let Some(state) = saved else {
        return (DEFAULT_WIDTH_LOGICAL, DEFAULT_HEIGHT_LOGICAL);
    };
    let previous_scale = valid_scale(state.saved_scale_factor.unwrap_or(scale));
    let width = valid_preference(state.preferred_width_logical).unwrap_or_else(|| {
        if state.sizing_version < 3 {
            DEFAULT_WIDTH_LOGICAL
        } else {
            f64::from(state.width) / previous_scale
        }
    });
    let height = valid_preference(state.preferred_height_logical).unwrap_or_else(|| {
        if state.sizing_version < 3 {
            (f64::from(state.height) / previous_scale)
                .clamp(MIN_HEIGHT_LOGICAL, DEFAULT_HEIGHT_LOGICAL)
        } else {
            f64::from(state.height) / previous_scale
        }
    });
    (width, height)
}

/// These calculations are physical-pixel only. Temporarily relax the Tauri
/// minimum in a very small high-DPI remote desktop instead of overflowing it.
fn fitted_size(
    work: WorkArea,
    preferred: (f64, f64),
    frame: PhysicalSize<u32>,
) -> (PhysicalSize<u32>, PhysicalSize<u32>) {
    let margin = (FIT_MARGIN_LOGICAL * work.scale).ceil() as u32;
    let available_w = work.width.saturating_sub(frame.width).saturating_sub(margin).max(1);
    let available_h = work.height.saturating_sub(frame.height).saturating_sub(margin).max(1);
    let min_w = ((DEFAULT_WIDTH_LOGICAL * work.scale).ceil() as u32)
        .min(available_w);
    let min_h = ((MIN_HEIGHT_LOGICAL * work.scale).ceil() as u32)
        .min(available_h);
    let width = ((preferred.0 * work.scale).round() as u32).clamp(min_w, available_w);
    let height = ((preferred.1 * work.scale).round() as u32).clamp(min_h, available_h);
    (
        PhysicalSize::new(width, height),
        PhysicalSize::new(min_w, min_h),
    )
}

fn clamp_outer_position(
    preferred: PhysicalPosition<i32>,
    outer: PhysicalSize<u32>,
    work: WorkArea,
) -> PhysicalPosition<i32> {
    let limit_x = i64::from(work.x) + i64::from(work.width.saturating_sub(outer.width));
    let limit_y = i64::from(work.y) + i64::from(work.height.saturating_sub(outer.height));
    PhysicalPosition::new(
        i64::from(preferred.x).clamp(i64::from(work.x), limit_x) as i32,
        i64::from(preferred.y).clamp(i64::from(work.y), limit_y) as i32,
    )
}

fn frame_size(window: &WebviewWindow) -> Result<PhysicalSize<u32>> {
    let outer = window.outer_size().map_err(|e| anyhow::anyhow!(e))?;
    let inner = window.inner_size().map_err(|e| anyhow::anyhow!(e))?;
    Ok(PhysicalSize::new(
        outer.width.saturating_sub(inner.width),
        outer.height.saturating_sub(inner.height),
    ))
}

fn restore_other_window(window: &WebviewWindow, state: &WindowState) -> Result<()> {
    window
        .set_size(PhysicalSize::new(state.width, state.height))
        .map_err(|e| anyhow::anyhow!(e))?;
    let monitors = window.available_monitors().map_err(|e| anyhow::anyhow!(e))?;
    let on_screen = monitors.iter().any(|m| {
        let x = i64::from(state.x);
        let y = i64::from(state.y);
        let mx = i64::from(m.position().x);
        let my = i64::from(m.position().y);
        x >= mx && x < mx + i64::from(m.size().width)
            && y >= my && y < my + i64::from(m.size().height)
    });
    if on_screen {
        window.set_position(PhysicalPosition::new(state.x, state.y))
            .map_err(|e| anyhow::anyhow!(e))?;
    } else {
        super::position::center_on_cursor_monitor(window)?;
    }
    Ok(())
}

/// Original lifecycle: save geometry on hide/close/exit. Detect manual resize
/// by comparing the last applied physical dimensions, and do not let an
/// automatic small-monitor fit overwrite the preferred logical size.
pub fn save_window_state(app: &AppHandle, label: &str) -> Result<()> {
    let window = app.get_webview_window(label)
        .ok_or_else(|| anyhow::anyhow!("window not found: {label}"))?;
    let pos = window.outer_position().map_err(|e| anyhow::anyhow!(e))?;
    let size = window.inner_size().map_err(|e| anyhow::anyhow!(e))?;
    let scale = valid_scale(window.scale_factor().map_err(|e| anyhow::anyhow!(e))?);
    let store = app.state::<WindowStateStore>();
    let old = store.get(label);
    let is_main = cfg!(target_os = "windows") && label == super::CLIPBOARD_WINDOW_LABEL;
    let pref = if is_main {
        let prev = old.as_ref();
        let old_scale = prev
            .and_then(|p| p.saved_scale_factor)
            .map(valid_scale)
            .unwrap_or(scale);
        let dpi_unchanged = (old_scale - scale).abs() < 0.02;
        // Updating only the axis the user actually resized also preserves a
        // wider preferred size when a small remote window is fitted.
        Some((
            if dpi_unchanged && prev.is_some_and(|p| p.width == size.width) {
                prev.and_then(|p| valid_preference(p.preferred_width_logical))
                    .unwrap_or(f64::from(size.width) / scale)
            } else if !dpi_unchanged {
                prev.and_then(|p| valid_preference(p.preferred_width_logical))
                    .unwrap_or(f64::from(size.width) / scale)
            } else {
                f64::from(size.width) / scale
            },
            if dpi_unchanged && prev.is_some_and(|p| p.height == size.height) {
                prev.and_then(|p| valid_preference(p.preferred_height_logical))
                    .unwrap_or(f64::from(size.height) / scale)
            } else if !dpi_unchanged {
                prev.and_then(|p| valid_preference(p.preferred_height_logical))
                    .unwrap_or(f64::from(size.height) / scale)
            } else {
                f64::from(size.height) / scale
            },
        ))
    } else {
        None
    };
    store.save(
        label,
        WindowState {
            x: pos.x, y: pos.y, width: size.width, height: size.height,
            sizing_version: if is_main { 3 } else { 0 },
            preferred_width_logical: pref.map(|p| p.0),
            preferred_height_logical: pref.map(|p| p.1),
            saved_scale_factor: Some(scale),
        },
    )
}

/// Single geometry restore entry. The original subwindow path is unchanged.
/// Windows main window: fit on every show to the CURRENT monitor work area.
/// Saved logical preferences survive temporary UU/RDP resolution changes.
pub fn restore_window_state(app: &AppHandle, label: &str) -> Result<bool> {
    let store = app.state::<WindowStateStore>();
    let saved = store.get(label);
    let window = app.get_webview_window(label)
        .ok_or_else(|| anyhow::anyhow!("window not found: {label}"))?;

    if label != super::CLIPBOARD_WINDOW_LABEL || !cfg!(target_os = "windows") {
        let Some(state) = saved else { return Ok(false); };
        restore_other_window(&window, &state)?;
        return Ok(true);
    }

    let policy = app.try_state::<SettingsStore>()
        .map(|s| s.snapshot().clipboard.window.position)
        .unwrap_or(WindowPosition::FollowCursor);
    let saved_position = saved.as_ref().map(|s| PhysicalPosition::new(s.x, s.y));
    let Some(monitor) = super::position::select_monitor(&window, policy, saved_position)? else {
        log::warn!("clipboard fit skipped: no monitor detected");
        return Ok(saved.is_some());
    };
    let work = WorkArea::from_monitor(&monitor);
    let preferred = preferred_size(saved.as_ref(), work.scale);
    let (wanted, minimum) = fitted_size(work, preferred, frame_size(&window)?);

    window.set_min_size(Some(minimum)).map_err(|e| anyhow::anyhow!(e))?;
    window.set_size(wanted).map_err(|e| anyhow::anyhow!(e))?;
    let size = window.inner_size().map_err(|e| anyhow::anyhow!(e))?;
    let desired_origin = saved_position
        .unwrap_or(PhysicalPosition::new(work.x, work.y));
    let origin = clamp_outer_position(
        desired_origin,
        window.outer_size().map_err(|e| anyhow::anyhow!(e))?,
        work,
    );
    window.set_position(origin).map_err(|e| anyhow::anyhow!(e))?;
    store.save(
        label,
        WindowState {
            x: origin.x, y: origin.y, width: size.width, height: size.height,
            sizing_version: 3,
            preferred_width_logical: Some(preferred.0),
            preferred_height_logical: Some(preferred.1),
            saved_scale_factor: Some(work.scale),
        },
    )?;
    if size != wanted {
        log::warn!("clipboard size constrained by window system: expected {wanted:?}, got {size:?}");
    }
    Ok(saved.is_some())
}

/// The existing window-event handler uses this only when the main window
/// actually extends outside the current work area. All size calculations
/// still go through restore_window_state rather than a second layout engine.
pub fn refit_visible_main_if_clipped(app: &AppHandle) -> Result<bool> {
    let Some(window) = app.get_webview_window(super::CLIPBOARD_WINDOW_LABEL) else {
        return Ok(false);
    };
    if !window.is_visible().unwrap_or(false) {
        return Ok(false);
    }
    let pos = window.outer_position().map_err(|e| anyhow::anyhow!(e))?;
    let policy = app.try_state::<SettingsStore>()
        .map(|s| s.snapshot().clipboard.window.position)
        .unwrap_or(WindowPosition::FollowCursor);
    let Some(monitor) = super::position::select_monitor(&window, policy, Some(pos))? else {
        return Ok(false);
    };
    let work = WorkArea::from_monitor(&monitor);
    let outer = window.outer_size().map_err(|e| anyhow::anyhow!(e))?;
    let clipped = clamp_outer_position(pos, outer, work) != pos
        || outer.width > work.width || outer.height > work.height;
    if clipped {
        restore_window_state(app, super::CLIPBOARD_WINDOW_LABEL)?;
    }
    Ok(clipped)
}

#[cfg(test)]
mod fit_tests {
    use super::{clamp_outer_position, fitted_size, preferred_size, WorkArea, WindowState};
    use tauri::{PhysicalPosition, PhysicalSize};

    fn area(w: u32, h: u32, dpi: f64) -> WorkArea {
        WorkArea { x: 0, y: 0, width: w, height: h, scale: dpi }
    }

    #[test]
    fn small_remote_fits_without_hidden_bottom_at_multiple_dpi() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            let work = area(1366, 728, dpi);
            let (fit, min) = fitted_size(work, (360.0, 900.0), PhysicalSize::new(8, 16));
            assert!(fit.width + 8 <= work.width);
            assert!(fit.height + 16 <= work.height);
            assert!(min.width <= fit.width && min.height <= fit.height);
        }
    }

    #[test]
    fn min_height_relaxes_only_when_monitor_is_too_small() {
        let work = area(640, 430, 2.0);
        let (fit, min) = fitted_size(work, (360.0, 500.0), PhysicalSize::new(8, 16));
        assert_eq!(fit, min);
        assert!(fit.height + 16 <= work.height);
        assert!(fit.width + 8 <= work.width);
    }

    #[test]
    fn saved_old_position_is_clamped_to_visible_area() {
        let work = area(1366, 728, 1.25);
        let pos = clamp_outer_position(
            PhysicalPosition::new(1800, 800),
            PhysicalSize::new(450, 625), work,
        );
        assert_eq!(pos, PhysicalPosition::new(916, 103));
    }

    #[test]
    fn version3_pref_survives_auto_fit_and_legacy_state_is_readable() {
        let modern: WindowState = serde_json::from_str(
            r#"{"x":12,"y":25,"width":450,"height":625,"sizing_version":3,
                "preferred_width_logical":440.0,"preferred_height_logical":850.0,
                "saved_scale_factor":1.25}"#,
        ).unwrap();
        assert_eq!(preferred_size(Some(&modern), 2.0), (440.0, 850.0));
        let legacy: WindowState = serde_json::from_str(
            r#"{"x":0,"y":0,"width":900,"height":900}"#,
        ).unwrap();
        assert_eq!(preferred_size(Some(&legacy), 1.5), (360.0, 500.0));
    }

    #[test]
    fn manual_v3_sizes_persist_even_when_above_default() {
        let modern: WindowState = serde_json::from_str(
            r#"{"x":0,"y":0,"width":800,"height":800,
                "sizing_version":3,"preferred_width_logical":640.0,
                "preferred_height_logical":640.0,"saved_scale_factor":1.25}"#,
        ).unwrap();
        assert_eq!(preferred_size(Some(&modern), 1.5), (640.0, 640.0));
    }
}
