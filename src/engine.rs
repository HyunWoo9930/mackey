//! The remapping decision engine — a pure, platform-free state machine.
//!
//! MAC-ONLY MODE: the keyboard behaves exclusively like a Mac. Alt and the
//! 한/영 key act as Cmd, Win acts as Option, physical Ctrl is the emacs
//! layer, Caps Lock toggles 한/영. Windows-native behaviors that don't exist
//! on a Mac (Ctrl+C copy, Alt-tap menu focus, Win-tap Start menu, Win+L, …)
//! are deliberately removed. The one native survivor is Alt+Tab, which IS the
//! mac Cmd+Tab in the same physical position.
//!
//! The Windows hook layer ([`crate::hook`]) translates raw hook events into
//! calls here and performs the returned injections with `SendInput`. Keeping
//! the logic free of Win32 types means every acceptance scenario can be
//! simulated and unit-tested on any host (see the tests at the bottom), and
//! Phase 2 (macOS/CGEventTap) can drive the same engine.

use crate::mapping::{self, CtrlOp, Target};

// modifier VKs (canonical Windows values, shared with mapping.rs)
// Physical keyboards always emit the left/right-specific codes in a
// low-level hook; the generic VK_SHIFT/VK_CONTROL/VK_MENU forms appear only
// in synthetic input (SendKeys, automation) and are treated as the left key.
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_RCONTROL: u16 = 0xA3;
pub const VK_TAB: u16 = 0x09;
pub const VK_CAPITAL: u16 = 0x14;
pub const VK_HANGUL: u16 = 0x15;

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
    /// physical VK that opened the session (VK_LMENU / VK_HANGUL / VK_RWIN …)
    vk: u16,
    /// foreground app excluded → whole session passes untouched
    passthrough: bool,
    /// a genuine Alt-down was forwarded to the OS (Cmd+Tab = Alt+Tab)
    forwarded: bool,
    /// modifier injected for a mouse chord (Cmd+click → Ctrl+click)
    mouse_mod: Option<u16>,
}

/// Physical Ctrl tracking. Ctrl itself passes through (it is a real modifier
/// on the Mac too); only the chords on top of it are rewritten.
#[derive(Default)]
struct CtrlState {
    ldown: bool,
    rdown: bool,
    /// foreground app excluded when Ctrl went down (terminals: Ctrl+C must
    /// stay SIGINT, so the whole layer is off there)
    passthrough: bool,
}

impl CtrlState {
    fn any(&self) -> bool {
        self.ldown || self.rdown
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Cmd,
    Opt,
}

#[derive(Default)]
pub struct Engine {
    cmd: Session, // physical Alt or 한/영 (right-Cmd position)
    opt: Session, // physical Win
    ctrl: CtrlState,
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
                    cleanup.push(Inj::up(VK_LMENU));
                }
                *sess = Session::default();
            }
            self.ctrl.passthrough = false;
        }
        cleanup
    }

    /// Process a physical (non-injected) key event. `foreground_excluded` is
    /// only consulted when a modifier goes down.
    pub fn on_key(&mut self, ev: KeyEvent, foreground_excluded: impl FnOnce() -> bool) -> Output {
        // Physical Shift and Ctrl always pass through; we track their state.
        match ev.vk {
            VK_LSHIFT | VK_SHIFT => {
                self.lshift = ev.down;
                return Output::pass();
            }
            VK_RSHIFT => {
                self.rshift = ev.down;
                return Output::pass();
            }
            VK_LCONTROL | VK_RCONTROL | VK_CONTROL => {
                let first = !self.ctrl.any() && ev.down;
                if ev.vk == VK_RCONTROL {
                    self.ctrl.rdown = ev.down;
                } else {
                    self.ctrl.ldown = ev.down;
                }
                if first && self.enabled {
                    self.ctrl.passthrough = foreground_excluded();
                }
                return Output::pass();
            }
            _ => {}
        }

        if !self.enabled {
            return Output::pass();
        }

        let layer = match ev.vk {
            // 한/영 sits right of Space — exactly where right Cmd lives on a
            // Mac keyboard — so it IS a Cmd key. (한/영 toggling moves to
            // Caps Lock and Ctrl+Space, like macOS.)
            VK_LMENU | VK_RMENU | VK_MENU | VK_HANGUL => Some(Layer::Cmd),
            VK_LWIN | VK_RWIN => Some(Layer::Opt),
            _ => None,
        };
        if let Some(layer) = layer {
            return self.on_modifier(layer, ev, foreground_excluded);
        }

        // Caps Lock = input-source toggle (한/영), the modern macOS default.
        // Caps state itself never toggles.
        if ev.vk == VK_CAPITAL {
            if ev.down {
                return Output::swallow_inject(vec![
                    Inj::down(VK_HANGUL),
                    Inj::up(VK_HANGUL),
                ]);
            }
            return Output::swallow();
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
            // close the genuine Alt we forwarded (Cmd+Tab session)
            inject.push(Inj::up(VK_LMENU));
        }
        // Clean tap → nothing. A Cmd or Option tap does nothing on a Mac, so
        // no menu-bar focus and no Start menu here either.
        Output::swallow_inject(inject)
    }

    fn on_plain_key(&mut self, ev: KeyEvent) -> Output {
        let (lshift, rshift) = (self.lshift, self.rshift);
        let shift = lshift || rshift;

        // Cmd layer wins, then Option, then the Ctrl (emacs) layer.
        if self.cmd.active && !self.cmd.passthrough {
            if !ev.down {
                return Output::pass(); // ups are harmless without their downs
            }
            let sess = &mut self.cmd;
            return match mapping::cmd_map(ev.vk, shift) {
                Some(t) => {
                    let mut inject = Vec::new();
                    if sess.forwarded {
                        // a real Alt-down is out there (Cmd+Tab, then a
                        // mapped chord): lift it so the chord stays pure
                        inject.push(Inj::up(VK_LMENU));
                        sess.forwarded = false;
                    }
                    chord(&mut inject, &t, lshift, rshift);
                    Output::swallow_inject(inject)
                }
                None if ev.vk == VK_TAB => {
                    // Cmd+Tab = the native Alt+Tab switcher (same physical
                    // position on a Mac). Forward a genuine Alt.
                    let mut inject = Vec::new();
                    if !sess.forwarded {
                        inject.push(Inj::down(VK_LMENU));
                        sess.forwarded = true;
                    }
                    inject.push(Inj { vk: ev.vk, scan: ev.scan, extended: ev.extended, down: true });
                    Output::swallow_inject(inject)
                }
                // mac-only: an unmapped Cmd chord does nothing at all —
                // no Alt menu accelerators, no Alt+Esc, no Alt+F4.
                None => Output::swallow(),
            };
        }

        if self.opt.active && !self.opt.passthrough {
            if !ev.down {
                return Output::pass();
            }
            return match mapping::opt_map(ev.vk) {
                Some(t) => {
                    let mut inject = Vec::new();
                    chord(&mut inject, &t, lshift, rshift);
                    Output::swallow_inject(inject)
                }
                // mac-only: Win+L / Win+D / … don't exist on a Mac keyboard
                None => Output::swallow(),
            };
        }

        if self.ctrl.any() && !self.ctrl.passthrough {
            if !ev.down {
                return Output::pass();
            }
            return match mapping::ctrl_map(ev.vk) {
                Some(op) => {
                    // Physical Ctrl already reached the OS — lift whichever
                    // Ctrl keys are held around the injected mac meaning.
                    let mut inject = Vec::new();
                    let mut held = [0u16; 2];
                    let mut n = 0;
                    if self.ctrl.ldown {
                        held[n] = VK_LCONTROL;
                        n += 1;
                    }
                    if self.ctrl.rdown {
                        held[n] = VK_RCONTROL;
                        n += 1;
                    }
                    for &c in &held[..n] {
                        inject.push(Inj::up(c));
                    }
                    match op {
                        CtrlOp::Chord(t) => chord(&mut inject, &t, lshift, rshift),
                        CtrlOp::Seq(steps) => {
                            for &(vk, down) in steps {
                                inject.push(if down { Inj::down(vk) } else { Inj::up(vk) });
                            }
                        }
                    }
                    for &c in &held[..n] {
                        inject.push(Inj::down(c));
                    }
                    Output::swallow_inject(inject)
                }
                None if mapping::ctrl_passthrough(ev.vk) => Output::pass(),
                // mac-only: Ctrl+C is not copy, Ctrl+Z is not undo, … —
                // Windows-native Ctrl shortcuts are removed.
                None => Output::swallow(),
            };
        }

        Output::pass()
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
    const VK_E: u16 = 0x45;
    const VK_F: u16 = 0x46;
    const VK_L: u16 = 0x4C;
    const VK_Q: u16 = 0x51;
    const VK_T: u16 = 0x54;
    const VK_V: u16 = 0x56;
    const VK_W: u16 = 0x57;
    const VK_Y: u16 = 0x59;
    const VK_Z: u16 = 0x5A;
    const VK_SPACE: u16 = 0x20;
    const VK_NEXT: u16 = 0x22;
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

    #[test]
    fn cmd_shift_4_opens_region_snip() {
        let mut e = Engine::new();
        // physical Shift passes through and completes the injected Win+S
        assert!(e.on_key(ev(VK_LSHIFT, true), || false).pass);
        e.on_key(ev(VK_LMENU, true), || false);
        expect_chord(&e.on_key(ev(0x34 /* 4 */, true), || false), &[VK_LWIN], 0x53 /* S */);
    }

    #[test]
    fn cmd_shift_3_full_screenshot_lifts_shift() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LSHIFT, true), || false);
        e.on_key(ev(VK_LMENU, true), || false);
        // pure Win+PrintScreen must reach the OS (Shift lifted around it)
        let out = e.on_key(ev(0x33 /* 3 */, true), || false);
        assert_eq!(
            out.inject,
            vec![
                Inj::up(VK_LSHIFT),
                Inj::down(VK_LWIN),
                Inj::down(0x2C), // PrintScreen
                Inj::up(0x2C),
                Inj::up(VK_LWIN),
                Inj::down(VK_LSHIFT),
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

    // -- criterion 4: Alt+Tab works exactly like stock (mac Cmd+Tab) --

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
        e.on_key(ev(VK_TAB, true), || false); // forwards real Alt (Cmd+Tab)
        let out = e.on_key(ev(VK_C, true), || false); // then Cmd+C
        // Alt must go up before Ctrl+C so the app can't see Ctrl+Alt+C
        assert_eq!(out.inject[0], Inj::up(VK_LMENU));
        assert_eq!(&out.inject[1..], &[
            Inj::down(VK_LCONTROL),
            Inj::down(VK_C),
            Inj::up(VK_C),
            Inj::up(VK_LCONTROL),
        ]);
    }

    // -- mac-only: solo taps and native chords do nothing --

    #[test]
    fn clean_alt_tap_does_nothing_like_a_cmd_tap() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        let up = e.on_key(ev(VK_LMENU, false), || false);
        assert!(!up.pass && up.inject.is_empty()); // no menu-bar focus
    }

    #[test]
    fn clean_win_tap_does_nothing_like_an_option_tap() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LWIN, true), || false);
        let up = e.on_key(ev(VK_LWIN, false), || false);
        assert!(!up.pass && up.inject.is_empty()); // no Start menu
    }

    #[test]
    fn windows_native_chords_are_dead() {
        let mut e = Engine::new();
        // Win+L (lock) doesn't exist on a Mac keyboard → swallowed
        e.on_key(ev(VK_LWIN, true), || false);
        let out = e.on_key(ev(VK_L, true), || false);
        assert!(!out.pass && out.inject.is_empty());
        e.on_key(ev(VK_LWIN, false), || false);
        // Alt+F4 direct → dead too (quit is Cmd+Q)
        e.on_key(ev(VK_LMENU, true), || false);
        let f4 = e.on_key(ev(VK_F4, true), || false);
        assert!(!f4.pass && f4.inject.is_empty());
        e.on_key(ev(VK_LMENU, false), || false);
    }

    // -- 한/영 key = right Cmd; Caps Lock = input toggle --

    #[test]
    fn hangul_key_acts_as_right_cmd() {
        let mut e = Engine::new();
        e.on_key(ev(VK_HANGUL, true), || false); // 한/영 down = Cmd down
        expect_chord(&e.on_key(ev(VK_C, true), || false), &[VK_LCONTROL], VK_C);
        // solo release does NOT toggle the IME (swallowed, nothing injected)
        let up = e.on_key(ev(VK_HANGUL, false), || false);
        assert!(!up.pass && up.inject.is_empty());
        // and 한/영+Tab still reaches the app switcher via forwarded Alt
        e.on_key(ev(VK_HANGUL, true), || false);
        let tab = e.on_key(ev(VK_TAB, true), || false);
        assert_eq!(tab.inject[0], Inj::down(VK_LMENU));
        let rel = e.on_key(ev(VK_HANGUL, false), || false);
        assert_eq!(rel.inject, vec![Inj::up(VK_LMENU)]);
    }

    #[test]
    fn caps_lock_becomes_hangul_toggle() {
        let mut e = Engine::new();
        let out = e.on_key(ev(VK_CAPITAL, true), || false);
        assert!(!out.pass); // caps state never toggles
        assert_eq!(out.inject, vec![Inj::down(VK_HANGUL), Inj::up(VK_HANGUL)]);
        assert!(!e.on_key(ev(VK_CAPITAL, false), || false).pass);
    }

    #[test]
    fn cmd_space_opens_search() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        // Cmd+Space ≈ Spotlight → a Win tap (Windows search)
        expect_chord(&e.on_key(ev(VK_SPACE, true), || false), &[], VK_LWIN);
    }

    // -- Ctrl layer: emacs bindings, Windows-native Ctrl chords removed --

    #[test]
    fn ctrl_is_the_emacs_layer_not_windows_shortcuts() {
        let mut e = Engine::new();
        assert!(e.on_key(ev(VK_LCONTROL, true), || false).pass); // Ctrl passes
        // Ctrl+A → Home (line start), physical Ctrl lifted around it
        let a = e.on_key(ev(VK_A, true), || false);
        assert!(!a.pass);
        assert_eq!(
            a.inject,
            vec![
                Inj::up(VK_LCONTROL),
                Inj::down(VK_HOME),
                Inj::up(VK_HOME),
                Inj::down(VK_LCONTROL),
            ]
        );
        // Ctrl+E → End
        let ee = e.on_key(ev(VK_E, true), || false);
        assert_eq!(ee.inject[1], Inj::down(VK_END));
        // Ctrl+V → Page Down (emacs), NOT paste
        let v = e.on_key(ev(VK_V, true), || false);
        assert_eq!(v.inject[1], Inj::down(VK_NEXT));
        // Ctrl+C → dead (not copy). Copy is Cmd(Alt)+C.
        let c = e.on_key(ev(VK_C, true), || false);
        assert!(!c.pass && c.inject.is_empty());
        // Ctrl+Z → dead (not undo). Undo is Cmd(Alt)+Z.
        let z = e.on_key(ev(VK_Z, true), || false);
        assert!(!z.pass && z.inject.is_empty());
        // Ctrl+Tab still cycles tabs (same on mac)
        assert!(e.on_key(ev(VK_TAB, true), || false).pass);
        assert!(e.on_key(ev(VK_LCONTROL, false), || false).pass);
        // after release the plain keys are back to normal
        let (d, _) = press(&mut e, VK_C);
        assert!(d.pass);
    }

    #[test]
    fn ctrl_space_toggles_hangul() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LCONTROL, true), || false);
        let out = e.on_key(ev(VK_SPACE, true), || false);
        assert_eq!(
            out.inject,
            vec![
                Inj::up(VK_LCONTROL),
                Inj::down(VK_HANGUL),
                Inj::up(VK_HANGUL),
                Inj::down(VK_LCONTROL),
            ]
        );
    }

    #[test]
    fn ctrl_arrows_switch_desktops() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LCONTROL, true), || false);
        // Ctrl+→ = next Space → Win+Ctrl+→
        let out = e.on_key(ev(VK_RIGHT, true), || false);
        assert_eq!(
            out.inject,
            vec![
                Inj::up(VK_LCONTROL),
                Inj::down(VK_LWIN),
                Inj::down(VK_LCONTROL),
                Inj::down(VK_RIGHT),
                Inj::up(VK_RIGHT),
                Inj::up(VK_LCONTROL),
                Inj::up(VK_LWIN),
                Inj::down(VK_LCONTROL),
            ]
        );
    }

    #[test]
    fn ctrl_k_kills_to_line_end_without_clipboard() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LCONTROL, true), || false);
        let out = e.on_key(ev(0x4B /* K */, true), || false);
        assert_eq!(
            out.inject,
            vec![
                Inj::up(VK_LCONTROL),
                Inj::down(VK_LSHIFT),
                Inj::down(VK_END),
                Inj::up(VK_END),
                Inj::up(VK_LSHIFT),
                Inj::down(0x2E), // Delete
                Inj::up(0x2E),
                Inj::down(VK_LCONTROL),
            ]
        );
    }

    // -- criterion 7: tray off → instant stock keyboard --

    #[test]
    fn disable_passes_everything_and_releases_held_keys() {
        let mut e = Engine::new();
        e.on_key(ev(VK_LMENU, true), || false);
        e.on_key(ev(VK_TAB, true), || false); // forwarded Alt is out there
        let cleanup = e.set_enabled(false);
        assert_eq!(cleanup, vec![Inj::up(VK_LMENU)]); // released immediately
        // stock behavior from here on — including native Ctrl+C
        assert!(e.on_key(ev(VK_LMENU, true), || false).pass);
        assert!(e.on_key(ev(VK_C, true), || false).pass);
        assert!(e.on_key(ev(VK_LMENU, false), || false).pass);
        assert!(e.on_key(ev(VK_LCONTROL, true), || false).pass);
        assert!(e.on_key(ev(VK_C, true), || false).pass); // Ctrl+C native again
        assert!(e.on_key(ev(VK_LCONTROL, false), || false).pass);
        assert!(e.on_key(ev(VK_CAPITAL, true), || false).pass); // caps native
        // and back on
        e.set_enabled(true);
        e.on_key(ev(VK_LMENU, true), || false);
        expect_chord(&e.on_key(ev(VK_C, true), || false), &[VK_LCONTROL], VK_C);
    }

    // -- criterion 8: terminals — everything native, Ctrl+C stays SIGINT --

    #[test]
    fn excluded_foreground_passes_whole_session() {
        let mut e = Engine::new();
        assert!(e.on_key(ev(VK_LMENU, true), || true).pass); // Alt passes
        assert!(e.on_key(ev(VK_C, true), || true).pass);
        assert!(e.on_key(ev(VK_C, false), || true).pass);
        assert!(e.on_key(ev(VK_LMENU, false), || true).pass);
        // Ctrl+C in a terminal = SIGINT, untouched
        assert!(e.on_key(ev(VK_LCONTROL, true), || true).pass);
        assert!(e.on_key(ev(VK_C, true), || true).pass);
        assert!(e.on_key(ev(VK_LCONTROL, false), || true).pass);
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
        assert!(!e.on_key(ev(VK_LMENU, true), || false).pass); // auto-repeat
        let up = e.on_key(ev(VK_LMENU, false), || false);
        assert!(up.inject.is_empty());
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
    fn generic_modifier_vks_from_synthetic_input_work() {
        // SendKeys/automation sends VK_MENU (0x12) instead of VK_LMENU —
        // must open the Cmd session all the same (used by the CI harness)
        let mut e = Engine::new();
        assert!(!e.on_key(ev(VK_MENU, true), || false).pass);
        expect_chord(&e.on_key(ev(VK_A, true), || false), &[VK_LCONTROL], VK_A);
        let up = e.on_key(ev(VK_MENU, false), || false);
        assert!(!up.pass && up.inject.is_empty());
        // generic VK_CONTROL drives the emacs layer too
        assert!(e.on_key(ev(VK_CONTROL, true), || false).pass);
        let a = e.on_key(ev(VK_A, true), || false);
        assert_eq!(a.inject[1], Inj::down(VK_HOME));
        assert_eq!(a.inject[0], Inj::up(VK_LCONTROL)); // lifted as left ctrl
        assert!(e.on_key(ev(VK_CONTROL, false), || false).pass);
    }

    #[test]
    fn keys_without_session_pass_untouched() {
        let mut e = Engine::new();
        let (d, u) = press(&mut e, VK_C);
        assert!(d.pass && u.pass);
        assert!(d.inject.is_empty() && u.inject.is_empty());
    }
}
