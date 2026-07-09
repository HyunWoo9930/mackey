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
pub const VK_CAPITAL: u16 = 0x14; // Caps Lock
pub const VK_HANGUL: u16 = 0x15; // 한/영
pub const VK_SPACE: u16 = 0x20;
pub const VK_NEXT: u16 = 0x22; // Page Down
pub const VK_END: u16 = 0x23;
pub const VK_HOME: u16 = 0x24;
pub const VK_LEFT: u16 = 0x25;
pub const VK_UP: u16 = 0x26;
pub const VK_RIGHT: u16 = 0x27;
pub const VK_DOWN: u16 = 0x28;
pub const VK_SNAPSHOT: u16 = 0x2C; // PrintScreen
pub const VK_DELETE: u16 = 0x2E;
pub const VK_LWIN: u16 = 0x5B;
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
        // Screenshots, macOS habits kept:
        // Cmd+Shift+3 (full screen → file) → Win+PrintScreen; the physical
        // Shift is lifted so the OS sees the pure save-to-file chord.
        0x33 /* 3 */ if shift => Some(Target { mods: &[VK_LWIN], vk: VK_SNAPSHOT, suppress_shift: true }),
        // Cmd+Shift+4 / +5 (region / toolbar) → Win+Shift+S snip overlay;
        // the physically held Shift passes through and completes the chord.
        0x34 | 0x35 /* 4 5 */ if shift => Some(Target { mods: &[VK_LWIN], vk: 0x53 /* S */, suppress_shift: false }),
        // Line / document navigation (physical Shift passes through → selection)
        VK_LEFT => Some(plain(VK_HOME)),
        VK_RIGHT => Some(plain(VK_END)),
        VK_UP => Some(ctrl(VK_HOME)),
        VK_DOWN => Some(ctrl(VK_END)),
        VK_RETURN => Some(ctrl(VK_RETURN)),
        // Cmd+Space ≈ Spotlight → Windows search (a Win-key tap)
        VK_SPACE => Some(plain(VK_LWIN)),
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
/// `None` → the chord is swallowed (mac-only mode: Win+L, Win+D and friends
/// do not exist on a Mac, so they must not fire).
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

/// What a physical-Ctrl chord does in mac-only mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlOp {
    /// Inject a chord (physical Ctrl is lifted around it by the engine).
    Chord(Target),
    /// Inject a literal key sequence (vk, is_down).
    Seq(&'static [(u16, bool)]),
}

/// Ctrl layer: on a Mac, Ctrl in a text field is the emacs/readline layer —
/// and crucially Ctrl+C / Ctrl+V are NOT copy/paste. Windows-native Ctrl
/// shortcuts must die so only the mac semantics remain.
///
/// `Some` → inject the mac meaning. `None` + [`ctrl_passthrough`] false →
/// the chord is swallowed (Windows-native shortcut removed).
pub fn ctrl_map(vk: u16) -> Option<CtrlOp> {
    use CtrlOp::{Chord, Seq};
    match vk {
        0x41 /* A */ => Some(Chord(plain(VK_HOME))),   // line start
        0x45 /* E */ => Some(Chord(plain(VK_END))),    // line end
        0x46 /* F */ => Some(Chord(plain(VK_RIGHT))),  // char forward
        0x42 /* B */ => Some(Chord(plain(VK_LEFT))),   // char back
        0x4E /* N */ => Some(Chord(plain(VK_DOWN))),   // next line
        0x50 /* P */ => Some(Chord(plain(VK_UP))),     // prev line
        0x44 /* D */ => Some(Chord(plain(VK_DELETE))), // forward delete
        0x48 /* H */ => Some(Chord(plain(VK_BACK))),   // backspace
        0x56 /* V */ => Some(Chord(plain(VK_NEXT))),   // page down
        // kill to end of line (no clipboard, like the mac kill buffer):
        // Shift+End to select, then Delete
        0x4B /* K */ => Some(Seq(&[
            (VK_LSHIFT, true),
            (VK_END, true),
            (VK_END, false),
            (VK_LSHIFT, false),
            (VK_DELETE, true),
            (VK_DELETE, false),
        ])),
        // Ctrl+Space = input-source toggle (한/영), the macOS default
        VK_SPACE => Some(Seq(&[(VK_HANGUL, true), (VK_HANGUL, false)])),
        // Ctrl+←/→ = switch desktop (macOS Spaces) → Win+Ctrl+←/→
        VK_LEFT => Some(Chord(Target { mods: &[VK_LWIN, VK_LCONTROL], vk: VK_LEFT, suppress_shift: false })),
        VK_RIGHT => Some(Chord(Target { mods: &[VK_LWIN, VK_LCONTROL], vk: VK_RIGHT, suppress_shift: false })),
        // Ctrl+↑ = Mission Control → Win+Tab (task view)
        VK_UP => Some(Chord(Target { mods: &[VK_LWIN], vk: VK_TAB, suppress_shift: false })),
        _ => None,
    }
}

/// Keys that keep working under physical Ctrl (identical on mac):
/// Ctrl+Tab cycles tabs, Ctrl+Enter behaves per-app.
pub fn ctrl_passthrough(vk: u16) -> bool {
    matches!(vk, VK_TAB | VK_RETURN)
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
    fn screenshot_chords() {
        // Cmd+Shift+3 → Win+PrintScreen, physical Shift lifted around it
        let full = cmd_map(0x33, true).unwrap();
        assert_eq!(full.mods, &[VK_LWIN]);
        assert_eq!(full.vk, VK_SNAPSHOT);
        assert!(full.suppress_shift);
        // Cmd+Shift+4 / +5 → Win+(passed-through Shift)+S snip overlay
        let region = cmd_map(0x34, true).unwrap();
        assert_eq!(region.mods, &[VK_LWIN]);
        assert_eq!(region.vk, 0x53);
        assert!(!region.suppress_shift);
        assert_eq!(cmd_map(0x35, true), cmd_map(0x34, true));
        // without Shift the digits stay Ctrl+digit (tab switching)
        assert_eq!(cmd_map(0x33, false).unwrap(), ctrl(0x33));
        assert_eq!(cmd_map(0x34, false).unwrap(), ctrl(0x34));
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
    fn tab_is_never_remapped_and_space_is_search() {
        assert_eq!(cmd_map(VK_TAB, false), None); // Alt+Tab untouched
        assert_eq!(cmd_map(VK_TAB, true), None); // Alt+Shift+Tab untouched
        assert_eq!(cmd_map(VK_SPACE, false), Some(plain(VK_LWIN))); // ≈Spotlight
    }

    #[test]
    fn ctrl_layer_is_emacs_not_windows() {
        use CtrlOp::Chord;
        assert_eq!(ctrl_map(0x41), Some(Chord(plain(VK_HOME)))); // Ctrl+A
        assert_eq!(ctrl_map(0x45), Some(Chord(plain(VK_END)))); // Ctrl+E
        assert_eq!(ctrl_map(0x43), None); // Ctrl+C: NOT copy anymore
        assert_eq!(ctrl_map(0x58), None); // Ctrl+X: NOT cut
        assert_eq!(ctrl_map(0x5A), None); // Ctrl+Z: NOT undo
        assert!(ctrl_passthrough(VK_TAB)); // Ctrl+Tab cycles tabs (mac parity)
        assert!(!ctrl_passthrough(0x43));
    }
}
