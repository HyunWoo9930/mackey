//! MacKey — make a Windows keyboard behave like a Mac keyboard.
//! Alt acts as Cmd, Win acts as Option. Zero-config tray-resident remapper.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod autostart;
pub mod engine;
#[cfg(windows)]
mod foreground;
#[cfg(windows)]
mod hook;
pub mod mapping;
#[cfg(windows)]
mod tray;

#[cfg(windows)]
fn main() {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::{
        CreateMutexW, GetCurrentProcess, SetPriorityClass, HIGH_PRIORITY_CLASS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    unsafe {
        // single instance
        let name: Vec<u16> = "Local\\MacKeySingleton\0".encode_utf16().collect();
        CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        // Low-level hook callbacks must return fast even under system load,
        // otherwise Windows silently removes the hook. High priority keeps
        // the hook thread responsive during an 8h+ workday.
        SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);

        // First run ever → register run-at-logon automatically (zero-config).
        // The installer also does this; this covers portable use. Respect an
        // explicit opt-out made in the tray menu.
        if !autostart::is_enabled() && !autostart::user_opted_out() {
            autostart::enable();
        }

        hook::install();
        tray::init();

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        hook::set_enabled(false); // release anything still held
        hook::uninstall();
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("MacKey Phase 1 targets Windows. Build with: cargo build --release --target x86_64-pc-windows-gnu");
}
