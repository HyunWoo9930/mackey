//! Foreground-app exclusion check. Called from the keyboard hook on each
//! Alt/Win key-down, so it must be fast: the result is cached per HWND and
//! only refreshed when the foreground window changes.

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::mapping::EXCLUDED_APPS;

// hook-thread only, same as the rest of the hook state
static mut CACHED_HWND: usize = 0;
static mut CACHED_EXCLUDED: bool = false;

pub fn excluded() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return false;
        }
        if hwnd as usize == CACHED_HWND {
            return CACHED_EXCLUDED;
        }
        let result = process_name(hwnd)
            .map(|name| EXCLUDED_APPS.contains(&name.as_str()))
            .unwrap_or(false);
        CACHED_HWND = hwnd as usize;
        CACHED_EXCLUDED = result;
        result
    }
}

unsafe fn process_name(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<String> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == 0 {
        return None;
    }
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if handle.is_null() {
        return None;
    }
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
    CloseHandle(handle);
    if ok == 0 {
        return None;
    }
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    full.rsplit(['\\', '/'])
        .next()
        .map(|s| s.to_ascii_lowercase())
}
