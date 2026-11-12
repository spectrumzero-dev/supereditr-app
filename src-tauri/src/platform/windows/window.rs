//! Windows and processes: the window in front, which program owns it, whether it runs at a higher
//! integrity level (as administrator), monitors and their scale.
//!
//! Positions on Windows are physical pixels on the virtual desktop, the same as UI Automation's.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromRect,
};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    TokenIntegrityLevel,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOTOWNER, GUITHREADINFO, GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo,
    GetWindowThreadProcessId, IsWindow,
};
use windows::core::PWSTR;

/// A top-level window, by its handle. Handles are reused only after a window is destroyed, so a
/// check also compares the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRef {
    pub hwnd: isize,
    pub pid: u32,
}

pub fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut _)
}

/// The window in front, or None when there's none (the desktop is switching).
pub fn foreground() -> Option<WindowRef> {
    let handle = unsafe { GetForegroundWindow() };
    if handle.0.is_null() {
        return None;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(handle, Some(&mut pid)) };
    Some(WindowRef { hwnd: handle.0 as isize, pid })
}

/// Whether `window` still exists and still belongs to the same process.
pub fn alive(window: WindowRef) -> bool {
    let handle = hwnd(window.hwnd);
    if !unsafe { IsWindow(Some(handle)) }.as_bool() {
        return false;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(handle, Some(&mut pid)) };
    pid == window.pid
}

/// The window's class name, such as `Chrome_WidgetWin_1` or `ConsoleWindowClass`.
pub fn class_name(handle: isize) -> String {
    let mut buffer = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd(handle), &mut buffer) };
    String::from_utf16_lossy(&buffer[..n.max(0) as usize])
}

/// The top-level owner of a window, for comparing a dialog with its app.
#[allow(dead_code)]
pub fn root_owner(handle: isize) -> isize {
    unsafe { GetAncestor(hwnd(handle), GA_ROOTOWNER) }.0 as isize
}

/// The process's program file, such as `C:\Windows\System32\notepad.exe`.
pub fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

/// The program's file name, such as `notepad.exe`: how settings name apps on Windows.
pub fn image_name(pid: u32) -> Option<String> {
    image_path(pid).map(|path| file_name(&path).to_string())
}

pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// A process's mandatory integrity level (the last part of its integrity SID): 0x2000 medium (an
/// ordinary app), 0x3000 high (run as administrator), 0x4000 system. None when its token can't be
/// read, which for an ordinary user means the process runs above them.
fn integrity_of(process: HANDLE) -> Option<u32> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut buffer = [0u8; 256];
        let mut len = 0u32;
        let read = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr() as *mut _),
            buffer.len() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        read.ok()?;
        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        Some(*GetSidSubAuthority(label.Label.Sid, u32::from(count).checked_sub(1)?))
    }
}

pub fn own_integrity() -> u32 {
    integrity_of(unsafe { GetCurrentProcess() }).unwrap_or(0x2000)
}

/// Whether `pid` runs at a higher integrity level than SuperEditr. Windows silently drops input
/// SuperEditr sends to such a window, and UI Automation can't read it. A process whose level can't
/// be read counts as higher.
pub fn elevated(pid: u32) -> bool {
    let level = unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(process) => {
                let level = integrity_of(process);
                let _ = CloseHandle(process);
                level
            }
            Err(_) => None,
        }
    };
    level.is_none_or(|level| level > own_integrity())
}

/// The caret of the window's thread, in screen pixels, when the app uses the system caret.
pub fn caret_rect(window: WindowRef) -> Option<RECT> {
    unsafe {
        let thread = GetWindowThreadProcessId(hwnd(window.hwnd), None);
        let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        GetGUIThreadInfo(thread, &mut info).ok()?;
        if info.hwndCaret.0.is_null() {
            return None;
        }
        let mut rect = info.rcCaret;
        let mut origin = POINT { x: rect.left, y: rect.top };
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(info.hwndCaret, &mut origin);
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        rect = RECT { left: origin.x, top: origin.y, right: origin.x + w.max(1), bottom: origin.y + h.max(1) };
        Some(rect)
    }
}

pub fn cursor() -> Option<POINT> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some(point)
}

/// A monitor's whole area and work area (without the taskbar), in physical pixels, and its scale
/// (1.0 at 96 DPI).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    #[allow(dead_code)]
    pub area: RECT,
    pub work: RECT,
    pub scale: f64,
}

fn monitor_info(monitor: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { GetMonitorInfoW(monitor, &mut info) }.ok().ok()?;
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    Some(Monitor { area: info.rcMonitor, work: info.rcWork, scale: f64::from(dpi_x.max(48)) / 96.0 })
}

/// The monitor holding most of `rect`, or the nearest one.
pub fn monitor_for_rect(rect: RECT) -> Option<Monitor> {
    monitor_info(unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST) })
}

pub fn monitor_for_point(point: POINT) -> Option<Monitor> {
    monitor_info(unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) })
}
