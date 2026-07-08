//! Platform-neutral mapping table: what a Cmd(Alt)+key or Option(Win)+key
//! chord should turn into. Phase 2 (macOS side) reuses this module — the key
//! codes below use Windows virtual-key numeric values as the canonical
//! representation; the macOS backend translates at the edge.
//!
//! This module has no Windows dependencies so `cargo test` runs on any host.

// ---- canonical key codes (Windows VK values) ----
pub const VK_BACK: u16 = 0x08;
pub const VK_TAB: u16 = 0x09;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_SPACE: u16 = 0x20;
pub const VK_END: u16 = 0x23;
pub const VK_HOME: u16 = 0x24;
pub const VK_LEFT: u16 = 0x25;
pub const VK_UP: u16 = 0x26;
pub const VK_RIGHT: u16 = 0x27;
pub const VK_DOWN: u16 = 0x28;
pub const VK_DELETE: u16 = 0x2E;
pub const VK_F4: u16 = 0x73;
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_LMENU: u16 = 0xA4; // left Alt
pub const VK_OEM_1: u16 = 0xBA; // ;
pub const VK_OEM_PLUS: u16 = 0xBB; // =/+
pub const VK_OEM_COMMA: u16 = 0xBC;
pub const VK_OEM_MINUS: u16 = 0xBD;
pub const VK_OEM_PERIOD: u16 = 0xBE;
pub const VK_OEM_2: u16 = 0xBF; // /
pub const VK_OEM_4: u16 = 0xDB; // [
pub const VK_OEM_6: u16 = 0xDD; // ]
pub const VK_OEM_7: u16 = 0xDE; // '

/// What to inject in place of the physical chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// Modifier keys to hold around the key press (in order).
    pub mods: &'static [u16],
    pub vk: u16,
    /// Temporarily lift a physically held Shift while injecting
    /// (used by Alt+Shift+Z → Ctrl+Y, where Shift must not leak through).
    pub suppress_shift: bool,
}

const fn ctrl(vk: u16) -> Target {
    Target { mods: &[VK_LCONTROL], vk, suppress_shift: false }
}
const fn plain(vk: u16) -> Target {
    Target { mods: &[], vk, suppress_shift: false }
}

/// Cmd layer: physical Alt held. `shift` is the physical Shift state.
///
/// Returning `None` means "not a mapped chord" — the hook then forwards a
/// genuine Alt+key to the OS (this is how Alt+Tab, Alt+Space, Alt+F4 keep
/// working untouched).
pub fn cmd_map(vk: u16, shift: bool) -> Option<Target> {
    match vk {
        // Cmd+Q quits the app → Alt+F4
        0x51 /* Q */ => Some(Target { mods: &[VK_LMENU], vk: VK_F4, suppress_shift: false }),
        // Cmd+Z undo / Cmd+Shift+Z redo (Ctrl+Y, Shift must not leak)
        0x5A /* Z */ if shift => Some(Target { mods: &[VK_LCONTROL], vk: 0x59 /* Y */, suppress_shift: true }),
        0x5A /* Z */ => Some(ctrl(0x5A)),
        // Line / document navigation (physical Shift passes through → selection)
        VK_LEFT => Some(plain(VK_HOME)),
        VK_RIGHT => Some(plain(VK_END)),
        VK_UP => Some(ctrl(VK_HOME)),
        VK_DOWN => Some(ctrl(VK_END)),
        VK_RETURN => Some(ctrl(VK_RETURN)),
        // Everything Cmd+<letter> on macOS is Ctrl+<letter> on Windows:
        // A S F N P R T W C V X + all the rest (O open, B bold, L addressbar,
        // D bookmark, G find-next, ...). Blanket-map letters and digits.
        0x30..=0x39 /* 0-9 */ => Some(ctrl(vk)),
        0x41..=0x5A /* A-Z */ => Some(ctrl(vk)),
        // Common punctuation chords: Cmd+= / Cmd+- (zoom), Cmd+[ ] (back/fwd,
        // indent), Cmd+/ (comment), Cmd+, (settings), etc.
        VK_OEM_PLUS | VK_OEM_MINUS | VK_OEM_COMMA | VK_OEM_PERIOD | VK_OEM_2
        | VK_OEM_4 | VK_OEM_6 | VK_OEM_1 | VK_OEM_7 => Some(ctrl(vk)),
        // Tab / Space / everything else: forward as real Alt+key.
        _ => None,
    }
}

/// Option layer: physical Win held. Word-wise editing.
///
/// `None` → forward as a genuine Win+key so Win+L, Win+D, Win+Shift+S, ...
/// all keep working.
pub fn opt_map(vk: u16) -> Option<Target> {
    match vk {
        VK_LEFT => Some(ctrl(VK_LEFT)),
        VK_RIGHT => Some(ctrl(VK_RIGHT)),
        VK_UP => Some(ctrl(VK_UP)),     // paragraph up (macOS Option+↑)
        VK_DOWN => Some(ctrl(VK_DOWN)), // paragraph down
        VK_BACK => Some(ctrl(VK_BACK)), // delete word left
        VK_DELETE => Some(ctrl(VK_DELETE)), // delete word right
        _ => None,
    }
}

/// Process image names (lowercase) where remapping is disabled entirely.
/// Terminals: Alt+C→Ctrl+C would send SIGINT instead of copying.
pub const EXCLUDED_APPS: &[&str] = &[
    "windowsterminal.exe",
    "openconsole.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "powershell_ise.exe",
    "pwsh.exe",
    "wsl.exe",
    "wslhost.exe",
    "mintty.exe",
    "alacritty.exe",
    "wezterm-gui.exe",
    "conemu.exe",
    "conemu64.exe",
    "putty.exe",
    "kitty.exe",
    "tabby.exe",
    "hyper.exe",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_become_ctrl_chords() {
        for vk in [0x43u16, 0x56, 0x58, 0x41, 0x53, 0x46, 0x4E, 0x50, 0x52, 0x54, 0x57] {
            let t = cmd_map(vk, false).unwrap();
            assert_eq!(t.mods, &[VK_LCONTROL]);
            assert_eq!(t.vk, vk);
            assert!(!t.suppress_shift);
        }
    }

    #[test]
    fn undo_redo() {
        assert_eq!(cmd_map(0x5A, false).unwrap(), ctrl(0x5A)); // Ctrl+Z
        let redo = cmd_map(0x5A, true).unwrap();
        assert_eq!(redo.vk, 0x59); // Ctrl+Y
        assert!(redo.suppress_shift);
    }

    #[test]
    fn quit_is_alt_f4() {
        let q = cmd_map(0x51, false).unwrap();
        assert_eq!(q.mods, &[VK_LMENU]);
        assert_eq!(q.vk, VK_F4);
    }

    #[test]
    fn cmd_arrows_are_line_and_document_nav() {
        assert_eq!(cmd_map(VK_LEFT, false).unwrap(), plain(VK_HOME));
        assert_eq!(cmd_map(VK_RIGHT, false).unwrap(), plain(VK_END));
        assert_eq!(cmd_map(VK_UP, false).unwrap(), ctrl(VK_HOME));
        assert_eq!(cmd_map(VK_DOWN, false).unwrap(), ctrl(VK_END));
        // with Shift held the same targets are injected; Shift passes through
        assert_eq!(cmd_map(VK_LEFT, true).unwrap(), plain(VK_HOME));
    }

    #[test]
    fn option_layer_is_word_wise() {
        assert_eq!(opt_map(VK_LEFT).unwrap(), ctrl(VK_LEFT));
        assert_eq!(opt_map(VK_RIGHT).unwrap(), ctrl(VK_RIGHT));
        assert_eq!(opt_map(VK_BACK).unwrap(), ctrl(VK_BACK));
        assert_eq!(opt_map(0x4C /* L */), None); // Win+L must stay native
    }

    #[test]
    fn tab_and_space_are_never_remapped() {
        assert_eq!(cmd_map(VK_TAB, false), None); // Alt+Tab untouched
        assert_eq!(cmd_map(VK_TAB, true), None); // Alt+Shift+Tab untouched
        assert_eq!(cmd_map(VK_SPACE, false), None);
    }
}
