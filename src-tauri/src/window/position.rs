use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::core::Result;
use crate::settings::WindowPosition;

/// Cursor-following and centering operate within the current monitor WORK
/// AREA, not the full resolution (which includes Windows taskbars).
struct MonitorInfo {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
}

fn is_on_monitor(m: &tauri::Monitor, x: f64, y: f64) -> bool {
    let origin = m.position();
    x >= f64::from(origin.x)
        && x < f64::from(origin.x) + f64::from(m.size().width)
        && y >= f64::from(origin.y)
        && y < f64::from(origin.y) + f64::from(m.size().height)
}

/// Shared monitor lookup for the original positioning flow and the one-time
/// fit performed before showing a window. Coordinates are physical pixels.
pub(super) fn select_monitor(
    window: &WebviewWindow,
    position: WindowPosition,
    saved: Option<PhysicalPosition<i32>>,
) -> Result<Option<tauri::Monitor>> {
    let monitors = window.available_monitors().map_err(|e| anyhow::anyhow!(e))?;
    let cursor = window.cursor_position().ok();
    let at_cursor = cursor.and_then(|c| {
        monitors.iter().find(|m| is_on_monitor(m, c.x, c.y))
    });
    let at_saved = saved.and_then(|p| {
        monitors.iter().find(|m| is_on_monitor(m, f64::from(p.x), f64::from(p.y)))
    });
    let selected = if matches!(position, WindowPosition::Remember) {
        at_saved.or(at_cursor)
    } else {
        at_cursor.or(at_saved)
    };
    if let Some(monitor) = selected {
        return Ok(Some(monitor.clone()));
    }
    Ok(window.current_monitor().map_err(|e| anyhow::anyhow!(e))?
        .or(window.primary_monitor().map_err(|e| anyhow::anyhow!(e))?))
}

fn monitor_from_cursor(
    window: &WebviewWindow,
) -> Result<Option<(MonitorInfo, PhysicalPosition<f64>)>> {
    let cursor = window.cursor_position().map_err(|e| anyhow::anyhow!(e))?;
    let Some(monitor) = select_monitor(window, WindowPosition::FollowCursor, None)? else {
        return Ok(None);
    };
    let work = monitor.work_area();
    let (position, size) = if work.size.width > 0 && work.size.height > 0 {
        (work.position, work.size)
    } else {
        (*monitor.position(), *monitor.size())
    };
    Ok(Some((MonitorInfo { position, size }, cursor)))
}

pub fn position_window(window: &WebviewWindow, position: WindowPosition) -> Result<()> {
    let Some((monitor, cursor)) = monitor_from_cursor(window)? else {
        return Ok(());
    };

    match position {
        WindowPosition::Remember => {}
        WindowPosition::FollowCursor => apply_follow(window, &monitor, &cursor)?,
        WindowPosition::Center => apply_center(window, &monitor)?,
    }
    Ok(())
}

fn bounded_origin(preferred: f64, low: f64, span: f64, window_span: f64) -> f64 {
    preferred.max(low).min((low + span - window_span).max(low))
}

fn apply_follow(
    window: &WebviewWindow,
    monitor: &MonitorInfo,
    cursor: &PhysicalPosition<f64>,
) -> Result<()> {
    // Position the whole outer window, not only the webview's inner content.
    let win_size = window.outer_size().map_err(|e| anyhow::anyhow!(e))?;
    let mon_x = f64::from(monitor.position.x);
    let mon_y = f64::from(monitor.position.y);
    let x = bounded_origin(
        cursor.x,
        mon_x,
        f64::from(monitor.size.width),
        f64::from(win_size.width),
    );
    let y = bounded_origin(
        cursor.y,
        mon_y,
        f64::from(monitor.size.height),
        f64::from(win_size.height),
    );
    window
        .set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}

/// Center the clipboard on the cursor monitor work area, including after
/// unplugging a monitor or resizing the remote desktop.
pub(super) fn center_on_cursor_monitor(window: &WebviewWindow) -> Result<()> {
    let Some((monitor, _)) = monitor_from_cursor(window)? else {
        return Ok(());
    };
    apply_center(window, &monitor)
}

fn apply_center(window: &WebviewWindow, monitor: &MonitorInfo) -> Result<()> {
    // Position the whole outer window, not only the webview's inner content.
    let win_size = window.outer_size().map_err(|e| anyhow::anyhow!(e))?;
    let mon_x = f64::from(monitor.position.x);
    let mon_y = f64::from(monitor.position.y);
    let x = bounded_origin(
        mon_x + (f64::from(monitor.size.width) - f64::from(win_size.width)) / 2.0,
        mon_x,
        f64::from(monitor.size.width),
        f64::from(win_size.width),
    );
    let y = bounded_origin(
        mon_y + (f64::from(monitor.size.height) - f64::from(win_size.height)) / 2.0,
        mon_y,
        f64::from(monitor.size.height),
        f64::from(win_size.height),
    );
    window
        .set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::bounded_origin;

    #[test]
    fn cursor_following_remains_inside_remote_workarea() {
        assert_eq!(bounded_origin(1300.0, 0.0, 1366.0, 540.0), 826.0);
        assert_eq!(bounded_origin(760.0, 0.0, 728.0, 625.0), 103.0);
        assert_eq!(bounded_origin(-300.0, 0.0, 1366.0, 540.0), 0.0);
    }

    #[test]
    fn negative_desktop_coordinates_are_supported() {
        assert_eq!(bounded_origin(-900.0, -1920.0, 1920.0, 800.0), -900.0);
        assert_eq!(bounded_origin(10.0, -1920.0, 1920.0, 800.0), -800.0);
    }
}
