//! Windows 窗口管理：剪贴板窗口默认不可聚焦，输入控件编辑期间临时恢复可聚焦。

use std::sync::Mutex;

use tauri::AppHandle;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, IsWindow, IsWindowVisible, SetForegroundWindow, SetWindowPos, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
};

use super::{
    get_window, CLIPBOARD_PINNED_WINDOW_LABEL, CLIPBOARD_SIDE_LEFT_WINDOW_LABEL,
    CLIPBOARD_WINDOW_LABEL,
};
use crate::core::Result;
use crate::{keyboard, mouse};

static PRE_EDIT_FOREGROUND_HWND: Mutex<Option<isize>> = Mutex::new(None);

pub fn show_window(app_handle: &AppHandle, label: &str) -> Result<()> {
    let window = get_window(app_handle, label)?;
    if label == CLIPBOARD_WINDOW_LABEL
        || label == CLIPBOARD_PINNED_WINDOW_LABEL
        || label == CLIPBOARD_SIDE_LEFT_WINDOW_LABEL
    {
        window
            .set_focusable(false)
            .map_err(|e| anyhow::anyhow!(e))?;
        if label == CLIPBOARD_WINDOW_LABEL {
            clear_pre_edit_foreground();
        }
    }

    window.show().map_err(|e| anyhow::anyhow!(e))?;
    window.unminimize().map_err(|e| anyhow::anyhow!(e))?;

    if label == CLIPBOARD_WINDOW_LABEL {
        keyboard::enable_navigation_keys(app_handle);
        mouse::enable_outside_click_hide(app_handle);
    } else if label != CLIPBOARD_PINNED_WINDOW_LABEL && label != CLIPBOARD_SIDE_LEFT_WINDOW_LABEL {
        window.set_focus().map_err(|e| anyhow::anyhow!(e))?;
    }

    Ok(())
}

/// 左右副宿主显示后重新将主窗放回 Windows topmost 栈顶，不激活窗口，不抢粘贴焦点。
/// 旧版只有一个副窗口，新版后显示的副宿主可能盖过或遮住主窗。
pub fn raise_main_clipboard_window(app_handle: &AppHandle) -> Result<()> {
    let main = get_window(app_handle, CLIPBOARD_WINDOW_LABEL)?;
    let raw = main.hwnd().map_err(|e| anyhow::anyhow!(e))?;
    let hwnd = HWND(raw.0 as isize);

    main.set_always_on_top(true)
        .map_err(|e| anyhow::anyhow!(e))?;

    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        )
        .map_err(|e| anyhow::anyhow!(e))?;
    }

    let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
    let position = main.outer_position().map_err(|e| anyhow::anyhow!(e))?;
    let size = main.inner_size().map_err(|e| anyhow::anyhow!(e))?;
    log::info!(
        "clipboard native main raised: hwnd={:?}, visible={visible}, pos=({},{}), inner={}x{}",
        hwnd,
        position.x,
        position.y,
        size.width,
        size.height
    );
    if !visible {
        return Err(anyhow::anyhow!("clipboard main HWND is not visible after topmost promotion").into());
    }

    Ok(())
}

pub fn set_clipboard_window_editing(
    app_handle: &AppHandle,
    label: &str,
    editing: bool,
) -> Result<()> {
    let window = get_window(app_handle, label)?;
    let raw_hwnd = window.hwnd().map_err(|e| anyhow::anyhow!(e))?;
    let hwnd = HWND(raw_hwnd.0 as isize);

    if editing {
        remember_pre_edit_foreground(hwnd);
        keyboard::disable_navigation_keys();
        window.set_focusable(true).map_err(|e| anyhow::anyhow!(e))?;
        window.set_focus().map_err(|e| anyhow::anyhow!(e))?;

        return Ok(());
    }

    let should_restore_foreground = unsafe { GetForegroundWindow() == hwnd };
    window
        .set_focusable(false)
        .map_err(|e| anyhow::anyhow!(e))?;

    if label == CLIPBOARD_WINDOW_LABEL && window.is_visible().unwrap_or(false) {
        keyboard::enable_navigation_keys(app_handle);
        mouse::enable_outside_click_hide(app_handle);
    }

    if should_restore_foreground {
        restore_pre_edit_foreground(hwnd);
    } else {
        clear_pre_edit_foreground();
    }

    Ok(())
}

pub fn hide_window(app_handle: &AppHandle, label: &str) -> Result<()> {
    let window = get_window(app_handle, label)?;
    window.hide().map_err(|e| anyhow::anyhow!(e))?;
    if label == CLIPBOARD_WINDOW_LABEL {
        if let Err(err) = window.set_focusable(false) {
            log::warn!("reset clipboard window focusable on hide failed: {err:?}");
        }
        clear_pre_edit_foreground();
        keyboard::disable_navigation_keys();
        mouse::disable_outside_click_hide();
        crate::menu::context_window::hide(app_handle);
    }

    Ok(())
}

fn remember_pre_edit_foreground(clipboard_hwnd: HWND) {
    let mut guard = PRE_EDIT_FOREGROUND_HWND
        .lock()
        .expect("pre edit foreground hwnd poisoned");
    if guard.is_some() {
        return;
    }

    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0 == 0 || foreground == clipboard_hwnd {
        return;
    }

    *guard = Some(foreground.0);
}

fn restore_pre_edit_foreground(clipboard_hwnd: HWND) {
    let previous = PRE_EDIT_FOREGROUND_HWND
        .lock()
        .expect("pre edit foreground hwnd poisoned")
        .take();
    let Some(previous) = previous else {
        return;
    };

    let previous_hwnd = HWND(previous);
    if previous_hwnd == clipboard_hwnd || !unsafe { IsWindow(previous_hwnd).as_bool() } {
        return;
    }

    if !unsafe { SetForegroundWindow(previous_hwnd).as_bool() } {
        log::debug!("restore pre-edit foreground window was rejected by Windows");
    }
}

fn clear_pre_edit_foreground() {
    PRE_EDIT_FOREGROUND_HWND
        .lock()
        .expect("pre edit foreground hwnd poisoned")
        .take();
}

pub fn show_taskbar_icon(app_handle: &AppHandle, visible: bool) -> Result<()> {
    let window = get_window(app_handle, CLIPBOARD_WINDOW_LABEL)?;
    window
        .set_skip_taskbar(!visible)
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}
