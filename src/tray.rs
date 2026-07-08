//! System tray icon + context menu (enable/disable, run at logon, quit).

use std::mem::{size_of, zeroed};
use std::sync::atomic::Ordering;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    GetCursorPos, LoadImageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, TrackPopupMenu, HICON, HMENU, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED,
    MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, SetTimer, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_COMMAND, WM_DESTROY, WM_LBUTTONDBLCLK, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

use crate::{autostart, hook};

const WM_TRAY: u32 = 0x8000 + 1; // WM_APP + 1
const CMD_TOGGLE: usize = 1;
const CMD_AUTOSTART: usize = 2;
const CMD_QUIT: usize = 3;

const ICON_ON: u16 = 1;
const ICON_OFF: u16 = 2;

static mut HWND_TRAY: HWND = std::ptr::null_mut();
static mut TASKBAR_CREATED_MSG: u32 = 0;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn load_icon(id: u16) -> HICON {
    LoadImageW(
        GetModuleHandleW(std::ptr::null()),
        id as usize as *const u16, // MAKEINTRESOURCE
        IMAGE_ICON,
        0,
        0,
        LR_DEFAULTSIZE | LR_SHARED,
    ) as HICON
}

unsafe fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

unsafe fn add_or_update_icon(hwnd: HWND, add: bool) {
    let on = hook::ENABLED.load(Ordering::Relaxed);
    let mut nid = notify_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = load_icon(if on { ICON_ON } else { ICON_OFF });
    let tip = wide(if on { "MacKey — 켜짐 (Alt=Cmd)" } else { "MacKey — 꺼짐" });
    nid.szTip[..tip.len().min(128)].copy_from_slice(&tip[..tip.len().min(128)]);
    Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &nid);
}

unsafe fn remove_icon(hwnd: HWND) {
    let nid = notify_data(hwnd);
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn toggle_enabled(hwnd: HWND) {
    let now = !hook::ENABLED.load(Ordering::Relaxed);
    hook::set_enabled(now);
    add_or_update_icon(hwnd, false);
}

unsafe fn show_menu(hwnd: HWND) {
    let menu: HMENU = CreatePopupMenu();
    let on = hook::ENABLED.load(Ordering::Relaxed);
    let auto = autostart::is_enabled();
    let check = |b: bool| if b { MF_CHECKED } else { MF_UNCHECKED };
    AppendMenuW(menu, MF_STRING | check(on), CMD_TOGGLE, wide("사용 (Alt = Cmd)").as_ptr());
    AppendMenuW(menu, MF_STRING | check(auto), CMD_AUTOSTART, wide("로그인 시 자동 실행").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    AppendMenuW(menu, MF_STRING, CMD_QUIT, wide("종료").as_ptr());

    let mut pt = zeroed();
    GetCursorPos(&mut pt);
    // required quirk: without SetForegroundWindow the menu won't dismiss on
    // an outside click
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
        pt.x,
        pt.y,
        0,
        hwnd,
        std::ptr::null(),
    ) as usize;
    DestroyMenu(menu);

    match cmd {
        CMD_TOGGLE => toggle_enabled(hwnd),
        CMD_AUTOSTART => {
            if auto {
                autostart::disable();
            } else {
                autostart::enable();
            }
        }
        CMD_QUIT => {
            DestroyWindow(hwnd);
        }
        _ => {}
    }
}

const WATCHDOG_TIMER_ID: usize = 1;

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            hook::watchdog_check();
            0
        }
        WM_TRAY => {
            match lparam as u32 {
                WM_RBUTTONUP => show_menu(hwnd),
                WM_LBUTTONDBLCLK => toggle_enabled(hwnd),
                _ => {}
            }
            0
        }
        WM_COMMAND => 0,
        WM_DESTROY => {
            remove_icon(hwnd);
            PostQuitMessage(0);
            0
        }
        m if m != 0 && m == TASKBAR_CREATED_MSG => {
            // Explorer restarted → the tray was wiped; re-add our icon
            add_or_update_icon(hwnd, true);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Create the hidden message window + tray icon. Must run on the same thread
/// as the message loop (which is also the hook thread).
pub fn init() {
    unsafe {
        TASKBAR_CREATED_MSG = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        let hinst = GetModuleHandleW(std::ptr::null());
        let class_name = wide("MacKeyTrayWindow");
        let mut wc: WNDCLASSW = zeroed();
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinst;
        wc.lpszClassName = class_name.as_ptr();
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            wide("MacKey").as_ptr(),
            0,
            0, 0, 0, 0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        HWND_TRAY = hwnd;
        add_or_update_icon(hwnd, true);
        // hook-health watchdog, once a minute (see hook::watchdog_check)
        SetTimer(hwnd, WATCHDOG_TIMER_ID, 60_000, None);
    }
}
