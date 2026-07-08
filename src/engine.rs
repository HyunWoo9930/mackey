//! The remapping decision engine — a pure, platform-free state machine.
//!
//! The Windows hook layer ([`crate::hook`]) translates raw hook events into
//! calls here and performs the returned injections with `SendInput`. Keeping
//! the logic free of Win32 types means every acceptance scenario can be
//! simulated and unit-tested on any host (see the tests at the bottom), and
//! Phase 2 (macOS/CGEventTap) can drive the same engine.

use crate::mapping::{self, Target};

// modifier VKs (canonical Windows values, shared with mapping.rs)
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_LCONTROL: u16 = 0xA2;

/// A physical key event as seen by the hook.
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub vk: u16,
    /// hardware scan code (forwarded keys are replayed with it)
    pub scan: u16,
    pub extended: bool,
    pub down: bool,
}

/// One key to synthesize. `scan == 0` → derive the scan code from `vk`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inj {
    pub vk: u16,
    pub scan: u16,
    pub extended: bool,
    pub down: bool,
}

impl Inj {
    fn down(vk: u16) -> Self {
        Inj { vk, scan: 0, extended: false, down: true }
    }
    fn up(vk: u16) -> Self {
        Inj { vk, scan: 0, extended: false, down: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Let the physical event through to the OS.
    Pass,
    /// Suppress the physical event (injections, if any, replace it).
    Swallow,
}

#[derive(Debug, Default)]
pub struct Output {
    pub pass: bool,
    pub inject: Vec<Inj>,
}

impl Output {
    fn pass() -> Self {
        Output { pass: true, inject: Vec::new() }
    }
    fn swallow() -> Self {
        Output { pass: false, inject: Vec::new() }
    }
    fn swallow_inject(inject: Vec<Inj>) -> Self {
        Output { pass: false, inject }
    }
}

#[derive(Default)]
struct Session {
    active: bool,
    /// physical VK that opened the session (VK_LMENU / VK_RWIN / ...)
    vk: u16,
    /// foreground app excluded → whole session passes untouched
    passthrough: bool,
    /// a genuine modifier-down was forwarded to the OS (Alt+Tab, Win+L, ...)
    forwarded: bool,
    /// some chord fired → no tap replay on release
    dirty: bool,
    /// modifier injected for a mouse chord (Cmd+click → Ctrl+click)
    mouse_mod: Option<u16>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Cmd,
    Opt,
}

#[derive(Default)]
pub struct Engine {
    cmd: Session, // physical Alt
    opt: Session, // physical Win
    lshift: bool,
    rshift: bool,
    enabled: bool,
}

impl Engine {
    pub fn new() -> Self {
        Engine { enabled: true, ..Default::default() }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Toggle. On disable, returns the injections needed to leave the system
    /// in a clean stock state (release anything we are still holding).
    pub fn set_enabled(&mut self, on: bool) -> Vec<Inj> {
        self.enabled = on;
        let mut cleanup = Vec::new();
        if !on {
            for sess in [&mut self.cmd, &mut self.opt] {
                if let Some(m) = sess.mouse_mod.take() {
                    cleanup.push(Inj::up(m));
                }
                if sess.active && sess.forwarded {
                    cleanup.push(Inj::up(sess.vk));
                }
                *sess = Session::default();
            }
        }
        cleanup
    }

    /// Process a physical (non-injected) key event. `foreground_excluded` is
    /// only consulted when a modifier session opens.
    pub fn on_key(&mut self, ev: KeyEvent, foreground_excluded: impl FnOnce() -> bool) -> Output {
        // Physical Shift always passes through; we just track it.
        match ev.vk {
            VK_LSHIFT => {
                self.lshift = ev.down;
                return Output::pass();
            }
            VK_RSHIFT => {
                self.rshift = ev.down;
                return Output::pass();
            }
            _ => {}
        }

        if !self.enabled {
            return Output::pass();
        }

        let layer = match ev.vk {
            VK_LMENU | VK_RMENU => Some(Layer::Cmd),
            VK_LWIN | VK_RWIN => Some(Layer::Opt),
            _ => None,
        };
        if let Some(layer) = layer {
            return self.on_modifier(layer, ev, foreground_excluded);
        }
        self.on_plain_key(ev)
    }

    fn session(&mut self, layer: Layer) -> &mut Session {
        match layer {
            Layer::Cmd => &mut self.cmd,
            Layer::Opt => &mut self.opt,
        }
    }

    fn on_modifier(
        &mut self,
        layer: Layer,
        ev: KeyEvent,
        foreground_excluded: impl FnOnce() -> bool,
    ) -> Output {
        let sess = self.session(layer);
        if ev.down {
            if sess.active {
                // auto-repeat of the held modifier
                return if sess.passthrough { Output::pass() } else { Output::swallow() };
            }
            *sess = Session { active: true, vk: ev.vk, ..Session::default() };
            if foreground_excluded() {
                sess.passthrough = true;
                return Output::pass();
            }
            // swallowed: the OS never sees the modifier
            return Output::swallow();
        }

        // modifier up
        if !sess.active || sess.vk != ev.vk {
            return Output::pass();
        }
        let was = std::mem::take(sess);
        if was.passthrough {
            return Output::pass();
        }
        let mut inject = Vec::new();
        if let Some(m) = was.mouse_mod {
            inject.push(Inj::up(m));
        }
        if was.forwarded {
            // close the genuine modifier we forwarded (Alt+Tab session etc.)
            inject.push(Inj::up(ev.vk));
        } else if !was.dirty {
            // clean tap → replay it so Alt-tap menu focus / Win-tap Start
            // menu keep their stock behavior
            inject.push(Inj::down(ev.vk));
            inject.push(Inj::up(ev.vk));
        }
        // dirty && !forwarded → swallow silently: no Alt-up ever reaches the
        // OS, so the menu bar cannot activate after Alt+C etc.
        Output::swallow_inject(inject)
    }

    fn on_plain_key(&mut self, ev: KeyEvent) -> Output {
        let (lshift, rshift) = (self.lshift, self.rshift);
        let shift = lshift || rshift;

        // Cmd layer wins if both modifiers are held.
        let (layer, target) = if self.cmd.active && !self.cmd.passthrough {
            (Layer::Cmd, mapping::cmd_map(ev.vk, shift))
        } else if self.opt.active && !self.opt.passthrough {
            (Layer::Opt, mapping::opt_map(ev.vk))
        } else {
            return Output::pass();
        };

        if !ev.down {
            // Key-ups pass through: apps ignore an up without a matching
            // down, and forwarded chords need the up to reach them anyway.
            return Output::pass();
        }

        let sess = self.session(layer);
        match target {
            Some(t) => {
                let mut inject = Vec::new();
                if sess.forwarded {
                    // a real modifier-down is out there (e.g. Alt+Space then
                    // Alt+C): lift it so the injected chord stays pure
                    inject.push(Inj::up(sess.vk));
                    sess.forwarded = false;
                }
                sess.dirty = true;
                chord(&mut inject, &t, lshift, rshift);
                Output::swallow_inject(inject)
            }
            None => {
                // Unmapped chord → make it a genuine modifier+key, injected
                // in one batch so ordering is guaranteed.
                sess.dirty = true;
                let mut inject = Vec::new();
                if !sess.forwarded {
                    inject.push(Inj::down(sess.vk));
                    sess.forwarded = true;
                }
                inject.push(Inj { vk: ev.vk, scan: ev.scan, extended: ev.extended, down: true });
                Output::swallow_inject(inject)
            }
        }
    }

    /// A mouse button-down / wheel event arrived. Returns the modifier to
    /// hold for the rest of the session (Cmd+click → Ctrl+click, Option+click
    /// → Alt+click) if this event opens a mouse chord; the hook then swallows
    /// the event and re-injects [modifier down, event] in order.
    pub fn on_mouse_chord(&mut self) -> Option<u16> {
        if !self.enabled {
            return None;
        }
        let (sess, mod_vk) = if self.cmd.active && !self.cmd.passthrough && !self.cmd.forwarded {
            (&mut self.cmd, VK_LCONTROL)
        } else if self.opt.active && !self.opt.passthrough && !self.opt.forwarded {
            (&mut self.opt, VK_LMENU)
        } else {
            return None;
        };
        if sess.mouse_mod.is_some() {
            return None; // modifier already held; let the event through
        }
        sess.mouse_mod = Some(mod_vk);
        sess.dirty = true;
        Some(mod_vk)
    }
}

/// Emit a mapped chord: [Shift lift?] mods↓ key↓ key↑ mods↑ [Shift restore?]
fn chord(out: &mut Vec<Inj>, t: &Target, lshift: bool, rshift: bool) {
    let mut lifted = [0u16; 2];
    let mut n = 0;
    if t.suppress_shift {
        if lshift {
            lifted[n] = VK_LSHIFT;
            n += 1;
        }
        if rshift {
            lifted[n] = VK_RSHIFT;
            n += 1;
        }
    }
    for &sh in &lifted[..n] {
        out.push(Inj::up(sh));
    }
    for &m in t.mods {
        out.push(Inj::down(m));
    }
    out.push(Inj::down(t.vk));
    out.push(Inj::up(t.vk));
    for &m in t.mods.iter().rev() {
        out.push(Inj::up(m));
    }
    for &sh in &lifted[..n] {
        out.push(Inj::down(sh));
    }
}

// ===========================================================================
// Acceptance-criteria simulations. Each test replays the exact physical key
// sequence a user would type and asserts on everything the OS would receive.
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;

    const VK_A: u16 = 0x41;
    const VK_C: u16 = 0x43;
    const VK_F: u16 = 0x46;
    const VK_L: u16 = 0x4C;
    const VK_Q: u16 = 0x51;
    const VK_T: u16 = 0x54;
    const VK_V: u16 = 0x56;
    const VK_W: u16 = 0x57;
    const VK_Y: u16 = 0x59;
    const VK_Z: u16 = 0x5A;
    const VK_TAB: u16 = 0x09;
    const VK_END: u16 = 0x23;
    const VK_HOME: u16 = 0x24;
    const VK_LEFT: u16 = 0x25;
    const VK_UP: u16 = 0x26;
    const VK_RIGHT: u16 = 0x27;
    const VK_DOWN: u16 = 0x28;
    const VK_F4: u16 = 0x73;

    fn ev(vk: u16, down: bool) -> KeyEvent {
        KeyEvent { vk, scan: 0x11, extended: false, down }
    }

    fn press(e: &mut Engine, vk: u16) -> (Output, Output) {
        (e.on_key(ev(vk, true), || false), e.on_key(ev(vk, false), || false))
    }

    /// injected keydown+keyup of `vk` framed by `mods` — the canonical chord
    fn expect_chord(out: &Output, mods: &[u16], vk: u16) {
        assert!(!out.pass, "physical key must be swallowed");
        let mut want: Vec<Inj> = Vec::new();
        for &m in mods {
            want.push(Inj::down(m));
        }
        want.push(Inj::down(vk));
        want.push(Inj::up(vk));
        for &m in mods.iter().rev() {
            want.push(Inj::up(m));
        }
        assert_eq!(out.inject, want);
    }

    // -- criterion 2: Notepad — Alt+A → Alt+C → Alt+V, arrows, selection --

    #[test]
    fn notepad_select_copy_paste_no_menu_flash() {
        let mut e = Engine::new();
        assert!(!e.on_key(ev(VK_LMENU, true), || false).pass); // Alt swallowed
        for vk in [VK_A, VK_C, VK_V] {
            let out = e.on_key(ev(vk, true), || false);
            expect_chord(&out, &[VK_LCONTROL], vk); // pure Ctrl+key, no Alt
            assert!(e.on_key(ev(vk, false), || false).pass);
        }
        // releasing Alt after chords: swallowed, nothing injected → the OS
        // never saw any Alt at all → menu bar cannot activate
        let up = e.on_key(ev(VK_LMENU, false), || false);
        assert!(!up.pass);
        assert!(up.inject.is_empty());
    }

    #[test]
    fn line_and_document_navigation() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        expect_chord(&e.on_key(ev(VK_LEFT, true), || false), &[], VK_HOME);
        expect_chord(&e.on_key(ev(VK_RIGHT, true), || false), &[], VK_END);
        expect_chord(&e.on_key(ev(VK_UP, true), || false), &[VK_LCONTROL], VK_HOME);
        expect_chord(&e.on_key(ev(VK_DOWN, true), || false), &[VK_LCONTROL], VK_END);
    }

    #[test]
    fn word_wise_navigation_on_option_layer() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LWIN, true), || false);
        expect_chord(&e.on_key(ev(VK_LEFT, true), || false), &[VK_LCONTROL], VK_LEFT);
        expect_chord(&e.on_key(ev(VK_RIGHT, true), || false), &[VK_LCONTROL], VK_RIGHT);
        expect_chord(&e.on_key(ev(0x08, true), || false), &[VK_LCONTROL], 0x08); // Win+BS
        // Win up after chords: swallowed → Start menu does NOT open
        let up = e.on_key(ev(VK_LWIN, false), || false);
        assert!(!up.pass && up.inject.is_empty());
    }

    #[test]
    fn shift_passes_through_for_selection() {
        let mut e = Engine::new();
        // physical Shift is never touched → injected Home combines into
        // Shift+Home (select to line start)
        assert!(e.on_key(ev(VK_LSHIFT, true), || false).pass);
        e.on_key(ev(VK_LMENU, true), || false);
        expect_chord(&e.on_key(ev(VK_LEFT, true), || false), &[], VK_HOME);
        assert!(e.on_key(ev(VK_LSHIFT, false), || false).pass);
    }

    #[test]
    fn undo_redo_shift_suppression() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        // Alt+Z → Ctrl+Z
        expect_chord(&e.on_key(ev(VK_Z, true), || false), &[VK_LCONTROL], VK_Z);
        e.on_key(ev(VK_Z, false), || false);
        // Alt+Shift+Z → Ctrl+Y with the held Shift lifted around the chord
        e.on_key(ev(VK_RSHIFT, true), || false);
        let out = e.on_key(ev(VK_Z, true), || false);
        assert_eq!(
            out.inject,
            vec![
                Inj::up(VK_RSHIFT),
                Inj::down(VK_LCONTROL),
                Inj::down(VK_Y),
                Inj::up(VK_Y),
                Inj::up(VK_LCONTROL),
                Inj::down(VK_RSHIFT),
            ]
        );
    }

    // -- criterion 3: Chrome — Alt+T/W/F new tab, close tab, find, quit --

    #[test]
    fn chrome_shortcuts() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        for vk in [VK_T, VK_W, VK_F] {
            expect_chord(&e.on_key(ev(vk, true), || false), &[VK_LCONTROL], vk);
            e.on_key(ev(vk, false), || false);
        }
        // Alt+Q → Alt+F4 (key between Alt down/up → no menu activation)
        expect_chord(&e.on_key(ev(VK_Q, true), || false), &[VK_LMENU], VK_F4);
    }

    // -- criterion 4: Alt+Tab works exactly like stock --

    #[test]
    fn alt_tab_forwards_a_genuine_alt() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        // first Tab: forward real Alt down + Tab down, in one ordered batch
        let out = e.on_key(ev(VK_TAB, true), || false);
        assert!(!out.pass);
        assert_eq!(out.inject[0], Inj::down(VK_LMENU));
        assert_eq!(out.inject[1].vk, VK_TAB);
        assert!(out.inject[1].down);
        assert!(e.on_key(ev(VK_TAB, false), || false).pass);
        // second Tab while holding: Alt already forwarded, only Tab injected
        let out2 = e.on_key(ev(VK_TAB, true), || false);
        assert_eq!(out2.inject.len(), 1);
        assert_eq!(out2.inject[0].vk, VK_TAB);
        // release: the forwarded Alt is closed → switcher commits
        let up = e.on_key(ev(VK_LMENU, false), || false);
        assert_eq!(up.inject, vec![Inj::up(VK_LMENU)]);
    }

    #[test]
    fn mapped_chord_after_forwarded_alt_lifts_it_first() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        e.on_key(ev(0x20 /* Space */, true), || false); // forwards real Alt
        let out = e.on_key(ev(VK_C, true), || false); // then Alt+C
        // Alt must go up before Ctrl+C so the app can't see Ctrl+Alt+C
        assert_eq!(out.inject[0], Inj::up(VK_LMENU));
        assert_eq!(&out.inject[1..], &[
            Inj::down(VK_LCONTROL),
            Inj::down(VK_C),
            Inj::up(VK_C),
            Inj::up(VK_LCONTROL),
        ]);
    }

    // -- menu-bar / Start-menu stock behavior on clean taps --

    #[test]
    fn clean_alt_tap_replays_for_menu_focus() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        let up = e.on_key(ev(VK_LMENU, false), || false);
        assert_eq!(up.inject, vec![Inj::down(VK_LMENU), Inj::up(VK_LMENU)]);
    }

    #[test]
    fn clean_win_tap_replays_for_start_menu() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LWIN, true), || false);
        let up = e.on_key(ev(VK_LWIN, false), || false);
        assert_eq!(up.inject, vec![Inj::down(VK_LWIN), Inj::up(VK_LWIN)]);
    }

    #[test]
    fn unmapped_win_combo_stays_native() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LWIN, true), || false);
        let out = e.on_key(ev(VK_L, true), || false); // Win+L lock
        assert_eq!(out.inject[0], Inj::down(VK_LWIN));
        assert_eq!(out.inject[1].vk, VK_L);
        let up = e.on_key(ev(VK_LWIN, false), || false);
        assert_eq!(up.inject, vec![Inj::up(VK_LWIN)]); // and no Start menu
    }

    // -- criterion 7: tray off → instant stock keyboard --

    #[test]
    fn disable_passes_everything_and_releases_held_keys() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        e.on_key(ev(VK_TAB, true), || false); // forwarded Alt is out there
        let cleanup = e.set_enabled(false);
        assert_eq!(cleanup, vec![Inj::up(VK_LMENU)]); // released immediately
        // stock behavior from here on
        assert!(e.on_key(ev(VK_LMENU, true), || false).pass);
        assert!(e.on_key(ev(VK_C, true), || false).pass);
        assert!(e.on_key(ev(VK_LMENU, false), || false).pass);
        // and back on
        e.set_enabled(true);
        e.on_key(ev(VK_LMENU, true), || false);
        expect_chord(&e.on_key(ev(VK_C, true), || false), &[VK_LCONTROL], VK_C);
    }

    // -- criterion 8: terminals — mapping disabled, Alt stays native --

    #[test]
    fn excluded_foreground_passes_whole_session() {
        let mut e = Engine::new();
        assert!(e.on_key(ev(VK_LMENU, true), || true).pass); // Alt passes
        assert!(e.on_key(ev(VK_C, true), || true).pass); // no Ctrl+C → no SIGINT
        assert!(e.on_key(ev(VK_C, false), || true).pass);
        assert!(e.on_key(ev(VK_LMENU, false), || true).pass);
    }

    // -- mouse chords --

    #[test]
    fn cmd_click_holds_ctrl_until_release() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        assert_eq!(e.on_mouse_chord(), Some(VK_LCONTROL)); // first click
        assert_eq!(e.on_mouse_chord(), None); // Ctrl already held
        let up = e.on_key(ev(VK_LMENU, false), || false);
        assert_eq!(up.inject, vec![Inj::up(VK_LCONTROL)]); // released with Alt
    }

    #[test]
    fn mouse_chord_inactive_without_session() {
        let mut e = Engine::new();
        assert_eq!(e.on_mouse_chord(), None);
    }

    // -- repeats and edge cases --

    #[test]
    fn modifier_autorepeat_is_swallowed() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        e.on_key(ev(VK_C, true), || false);
        // Alt auto-repeat must not reset the dirty flag
        assert!(!e.on_key(ev(VK_LMENU, true), || false).pass);
        let up = e.on_key(ev(VK_LMENU, false), || false);
        assert!(up.inject.is_empty()); // still no menu replay
    }

    #[test]
    fn chord_autorepeat_repeats_the_mapping() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        for _ in 0..3 {
            // held Alt+← keeps emitting Home (cursor stays at line start)
            expect_chord(&e.on_key(ev(VK_LEFT, true), || false), &[], VK_HOME);
        }
    }

    #[test]
    fn keys_without_session_pass_untouched() {
        let mut e = Engine::new();
        let (d, u) = press(&mut e, VK_C);
        assert!(d.pass && u.pass);
        assert!(d.inject.is_empty() && u.inject.is_empty());
    }
}
