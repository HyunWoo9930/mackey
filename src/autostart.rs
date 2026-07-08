//! Run-at-logon via Task Scheduler. A registry Run key cannot elevate, and
//! this program requires administrator rights (so remapping also reaches
//! elevated windows) — a scheduled task with /RL HIGHEST is the standard way.

use std::os::windows::process::CommandExt;
use std::process::Command;

const TASK_NAME: &str = "MacKey";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn schtasks(args: &[&str]) -> bool {
    Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn is_enabled() -> bool {
    schtasks(&["/Query", "/TN", TASK_NAME])
}

/// The user explicitly turned autostart off in the tray — remember that so
/// the next launch doesn't silently re-register the task.
fn optout_marker() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|d| {
        let mut p = std::path::PathBuf::from(d);
        p.push("MacKey");
        p.push("autostart-optout");
        p
    })
}

pub fn user_opted_out() -> bool {
    optout_marker().map(|p| p.exists()).unwrap_or(false)
}

pub fn enable() -> bool {
    if let Some(p) = optout_marker() {
        let _ = std::fs::remove_file(p);
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_default();
    let tr = format!("\"{exe}\"");
    schtasks(&[
        "/Create", "/F",
        "/TN", TASK_NAME,
        "/SC", "ONLOGON",
        "/RL", "HIGHEST",
        "/TR", &tr,
    ])
}

pub fn disable() -> bool {
    if let Some(p) = optout_marker() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, b"1");
    }
    schtasks(&["/Delete", "/F", "/TN", TASK_NAME])
}
