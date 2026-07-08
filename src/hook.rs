//! Win32 adapter: low-level keyboard/mouse hooks driving the platform-free
//! decision engine in [`crate::engine`]. This layer only parses hook events,
//! consults the engine, and performs the returned injections with SendInput.
//!
//! Both hook procs run on the thread that installed them (the main message
//! loop), so the engine needs no locking.

use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, LLMHF_INJECTED, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN,
    WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
};

use crate::engine::{Engine, Inj, KeyEvent};
use crate::foreground;

/// Signature stamped on everything we inject (defense in depth on top of the
/// mandatory LLKHF_INJECTED pass-through).
const MAGIC: usize = 0x4D41_4B59; // "MAKY"

const LLKHF_EXTENDED: u32 = 0x01;

/// Mirror of the engine's enabled flag for the tray UI.
pub static ENABLED: AtomicBool = AtomicBool::new(true);

/// Test-only (MACKEY_TEST_TREAT_INJECTED=1): treat injected events that do
/// NOT carry our signature as physical, so a CI harness can exercise the
/// remapping end-to-end with SendKeys/SendInput on a real Windows runner.
/// Off by default — normal builds pass every injected event through.
static TEST_TREAT_INJECTED: AtomicBool = AtomicBool::new(false);

static mut ENGINE: Option<Engine> = None;
static mut KBD_HOOK: HHOOK = std::ptr::null_mut();
static mut MOUSE_HOOK: HHOOK = std::ptr::null_mut();

// Hook state is only touched from the hook thread.
#[allow(static_mut_refs)]
unsafe fn engine() -> &'static mut Engine {
    ENGINE.get_or_insert_with(Engine::new)
}

pub fn install() {
    if std::env::var_os("MACKEY_TEST_TREAT_INJECTED").is_some_and(|v| v == "1") {
        TEST_TREAT_INJECTED.store(true, Ordering::Relaxed);
    }
    unsafe {
        engine(); // init before the first event
        KBD_HOOK = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kbd_proc), std::ptr::null_mut(), 0);
        MOUSE_HOOK = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), std::ptr::null_mut(), 0);
        assert!(!KBD_HOOK.is_null(), "failed to install keyboard hook");
    }
}

pub fn uninstall() {
    unsafe {
        if !KBD_HOOK.is_null() {
            UnhookWindowsHookEx(KBD_HOOK);
            KBD_HOOK = std::ptr::null_mut();
        }
        if !MOUSE_HOOK.is_null() {
            UnhookWindowsHookEx(MOUSE_HOOK);
            MOUSE_HOOK = std::ptr::null_mut();
        }
    }
}

/// Toggle from the tray (same thread). On disable the engine hands back the
/// key-ups needed to leave the system in a clean stock state.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    unsafe {
        let cleanup = engine().set_enabled(on);
        inject(&cleanup);
    }
}

// ---------- input synthesis ----------

fn is_extended(vk: u16) -> bool {
    matches!(
        vk as u32,
        0x21..=0x2E // PgUp PgDn End Home arrows PrintScr Ins Del
            | 0x5B | 0x5C // Win keys
            | 0xA3 | 0xA5 // RCtrl RAlt
            | 0x6F // numpad divide
    )
}

fn to_input(k: &Inj) -> INPUT {
    let mut flags = if k.down { 0 } else { KEYEVENTF_KEYUP };
    let scan = if k.scan != 0 {
        if k.extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        k.scan
    } else {
        if is_extended(k.vk) {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        unsafe { MapVirtualKeyW(k.vk as u32, MAPVK_VK_TO_VSC) as u16 }
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: k.vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MAGIC,
            },
        },
    }
}

fn inject(keys: &[Inj]) {
    if keys.is_empty() {
        return;
    }
    let inputs: Vec<INPUT> = keys.iter().map(to_input).collect();
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
}

// ---------- keyboard hook ----------

const SWALLOW: LRESULT = 1;

unsafe fn pass(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

pub unsafe extern "system" fn kbd_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return pass(code, wparam, lparam);
    }
    let kb = &*(lparam as *const KBDLLHOOKSTRUCT);

    // Never touch injected events (ours or any other app's) → no feedback
    // loops, and other automation keeps working. Our own MAGIC-stamped
    // events always pass; in CI test mode, foreign injected events are
    // processed as if physical (see TEST_TREAT_INJECTED).
    if kb.flags & LLKHF_INJECTED != 0 || kb.dwExtraInfo == MAGIC {
        if kb.dwExtraInfo == MAGIC || !TEST_TREAT_INJECTED.load(Ordering::Relaxed) {
            return pass(code, wparam, lparam);
        }
    }

    let msg = wparam as u32;
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    if !down && !up {
        return pass(code, wparam, lparam);
    }

    let ev = KeyEvent {
        vk: kb.vkCode as u16,
        scan: kb.scanCode as u16,
        extended: kb.flags & LLKHF_EXTENDED != 0,
        down,
    };
    let out = engine().on_key(ev, foreground::excluded);
    inject(&out.inject);
    if out.pass {
        pass(code, wparam, lparam)
    } else {
        SWALLOW
    }
}

// ---------- mouse hook (Cmd+click → Ctrl+click, Cmd+wheel → Ctrl+wheel) ----------

pub unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return pass(code, wparam, lparam);
    }
    let ms = &*(lparam as *const MSLLHOOKSTRUCT);
    if ms.flags & LLMHF_INJECTED != 0 || ms.dwExtraInfo == MAGIC {
        return pass(code, wparam, lparam);
    }

    let msg = wparam as u32;
    let is_chord_event = matches!(
        msg,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL
            | WM_MOUSEHWHEEL
    );
    if !is_chord_event {
        return pass(code, wparam, lparam);
    }

    let Some(mod_vk) = engine().on_mouse_chord() else {
        return pass(code, wparam, lparam);
    };

    // First mouse event of the chord: swallow it and re-inject as
    // [modifier down, same mouse event] so the app sees them in order.
    // high word of mouseData: XBUTTON id (unsigned) or wheel delta (signed,
    // must be sign-extended when re-injected)
    let hiword = (ms.mouseData >> 16) as u16;
    let wheel_delta = hiword as i16 as i32 as u32;
    let (flags, data) = match msg {
        WM_LBUTTONDOWN => (MOUSEEVENTF_LEFTDOWN, 0u32),
        WM_RBUTTONDOWN => (MOUSEEVENTF_RIGHTDOWN, 0),
        WM_MBUTTONDOWN => (MOUSEEVENTF_MIDDLEDOWN, 0),
        WM_XBUTTONDOWN => (MOUSEEVENTF_XDOWN, hiword as u32),
        WM_MOUSEWHEEL => (MOUSEEVENTF_WHEEL, wheel_delta),
        _ => (MOUSEEVENTF_HWHEEL, wheel_delta),
    };
    let mouse = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MAGIC,
            },
        },
    };
    let modifier = to_input(&Inj { vk: mod_vk, scan: 0, extended: false, down: true });
    let seq = [modifier, mouse];
    SendInput(seq.len() as u32, seq.as_ptr(), size_of::<INPUT>() as i32);
    SWALLOW
}
