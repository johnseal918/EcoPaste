//! 「这次剪贴板变更来自哪个应用」的探测：在剪贴板事件回调里调用，
//! 返回稳定 id（macOS bundle id / Windows exe 绝对路径）、显示名、可选的 icon PNG 字节。
//!
//! 必须**同步**在监听回调一发生时立即抓——延后到 await 之后再问，前台应用很可能已经切走。
//! 探测失败（无前台应用 / 自身复制 / 平台 API 错误）一律返回 `None`，不阻断入库。
//!
//! 平台 API：macOS 走 `NSWorkspace.frontmostApplication`，Windows 走 `GetForegroundWindow`
//! + `QueryFullProcessImageNameW`。图标统一交给 `crate::clipboard::icon` 跨平台抽取。

use std::path::PathBuf;

use crate::db::models::Platform;

#[derive(Debug, Clone)]
pub enum FrontmostHint {
    #[cfg(target_os = "macos")]
    Mac(FrontmostApp),
    #[cfg(target_os = "windows")]
    WindowsPid(u32),
}

#[derive(Debug, Clone)]
pub struct FrontmostApp {
    /// 稳定主键。macOS = bundle id（如 `com.apple.Safari`），Windows = exe 绝对路径。
    pub id: String,
    /// 显示名（localizedName / FileDescription / exe stem 的优先回落）。
    pub name: String,
    pub platform: Platform,
    /// 用于后台提取应用图标的路径。监听回调只抓元数据，不做图标 IO/编码。
    pub icon_path: Option<PathBuf>,
}

/// 在剪贴板事件刚到达时抓一个尽可能轻量的来源提示。
/// Windows 只取前台窗口 PID，不做 OpenProcess/路径查询；昂贵解析放到后台。
pub fn capture_frontmost_hint() -> Option<FrontmostHint> {
    #[cfg(target_os = "macos")]
    {
        macos::detect().map(FrontmostHint::Mac)
    }
    #[cfg(target_os = "windows")]
    {
        windows::capture_pid().map(FrontmostHint::WindowsPid)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// 剪贴板来源优先采用 Win32 真正的 owner，而不是读取完成后的前台应用。
/// 当 owner 不可用才回退到及时抓取的前台窗口，不读取用户数据。
pub fn capture_clipboard_source_hint() -> Option<FrontmostHint> {
    #[cfg(target_os = "windows")]
    {
        windows::capture_clipboard_owner_pid()
            .map(FrontmostHint::WindowsPid)
            .or_else(capture_frontmost_hint)
    }
    #[cfg(not(target_os = "windows"))]
    {
        capture_frontmost_hint()
    }
}

/// 判断剪贴板所有者是否为 Excel，与当前前台窗口无关。
#[cfg(target_os = "windows")]
pub fn clipboard_owner_is_excel() -> bool {
    windows::capture_clipboard_owner_pid().is_some_and(windows::is_excel_pid)
}

/// 获取仅用于诊断的剪贴板 owner PID，不读取或记录正文。
#[cfg(target_os = "windows")]
pub fn clipboard_owner_pid() -> Option<u32> {
    windows::capture_clipboard_owner_pid()
}

/// 把轻量来源提示解析成稳定应用信息。这个函数允许放到后台线程执行。
pub fn resolve_frontmost_hint(hint: FrontmostHint) -> Option<FrontmostApp> {
    match hint {
        #[cfg(target_os = "macos")]
        FrontmostHint::Mac(app) => Some(app),
        #[cfg(target_os = "windows")]
        FrontmostHint::WindowsPid(pid) => windows::from_pid(pid),
    }
}

/// 兼容其它调用点的一步式探测；监听热路径应优先使用
/// capture_frontmost_hint + resolve_frontmost_hint 两阶段方式。
pub fn detect_frontmost() -> Option<FrontmostApp> {
    capture_frontmost_hint().and_then(resolve_frontmost_hint)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{FrontmostApp, Platform};
    use std::path::PathBuf;

    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2_app_kit::{NSRunningApplication, NSWorkspace};
    use objc2_foundation::{NSString, NSURL};

    pub(super) fn detect() -> Option<FrontmostApp> {
        autoreleasepool(|_| {
            let workspace = NSWorkspace::sharedWorkspace();
            let app = workspace.frontmostApplication()?;

            // 没 bundle id 的进程（命令行子进程等）不入表，避免主键不稳定。
            let id = app.bundleIdentifier().map(|s| s.to_string())?;
            let name = app
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_else(|| id.clone());

            let icon_path = unsafe { bundle_path(&app) };

            Some(FrontmostApp {
                id,
                name,
                platform: Platform::Macos,
                icon_path,
            })
        })
    }

    /// 通过 NSRunningApplication.bundleURL 拿到 .app 路径。objc2-app-kit 当前 feature 没生成
    /// 该 getter，只能 msg_send!；返回 NSURL 后用 path 取 NSString → Rust String。
    unsafe fn bundle_path(app: &NSRunningApplication) -> Option<PathBuf> {
        let url: Option<Retained<NSURL>> = msg_send![app, bundleURL];
        let url = url?;
        let path: Option<Retained<NSString>> = msg_send![&*url, path];
        Some(PathBuf::from(path?.to_string()))
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::{FrontmostApp, Platform};
    use std::path::{Path, PathBuf};

    use winapi::shared::minwindef::{DWORD, FALSE};
    use winapi::shared::windef::HWND;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::processthreadsapi::OpenProcess;
    use winapi::um::winbase::QueryFullProcessImageNameW;
    use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;
    use winapi::um::winuser::{GetClipboardOwner, GetForegroundWindow, GetWindowThreadProcessId};

    /// 获取实际剪贴板所有者，而不是可能已被切换的前台窗口。
    pub(super) fn capture_clipboard_owner_pid() -> Option<DWORD> {
        let hwnd = unsafe { GetClipboardOwner() };
        if hwnd.is_null() {
            return None;
        }
        let mut pid: DWORD = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        (pid != 0).then_some(pid)
    }

    pub(super) fn is_excel_pid(pid: DWORD) -> bool {
        let Some(path) = (unsafe { process_exe_path(pid) }) else {
            return false;
        };
        Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("EXCEL.EXE"))
    }

    pub(super) fn capture_pid() -> Option<DWORD> {
        let hwnd: HWND = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return None;
        }

        let mut pid: DWORD = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        (pid != 0).then_some(pid)
    }

    pub(super) fn from_pid(pid: DWORD) -> Option<FrontmostApp> {
        let exe_path = unsafe { process_exe_path(pid) }?;
        // 自身写回事件依赖 WritebackGuard 的 content_hash 判定，这里不过滤自身。
        let name = Path::new(&exe_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&exe_path)
            .to_owned();
        let icon_path = Some(PathBuf::from(&exe_path));

        Some(FrontmostApp {
            id: exe_path,
            name,
            platform: Platform::Windows,
            icon_path,
        })
    }

    unsafe fn process_exe_path(pid: DWORD) -> Option<String> {
        // PROCESS_QUERY_LIMITED_INFORMATION 足够 QueryFullProcessImageNameW，且不需要管理员权限。
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if handle.is_null() {
            return None;
        }

        let mut buf = [0u16; 1024];
        let mut size: DWORD = buf.len() as DWORD;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);

        if ok == 0 || size == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}
