//! Host input for the virtual pad (`gamepad.rs`): a real XInput controller and a keyboard mapping.
//!
//! The title's `GameInputCreate` binds to the PC GameInput, which already enumerates the host's
//! keyboard, mouse and the gamepads it supports, with real readings. What it does not give a
//! console title is a pad it can play with when the host pad is only visible to XInput, or when
//! there is no pad at all (console titles are mostly pad-only). This module feeds the virtual pad:
//!
//! - XInput (`xinput1_4.dll`, loaded at run time): the first connected user index is mapped to
//!   `GameInputGamepadState` (`from_xinput`). No dead zone is applied: GameInput gamepad readings
//!   are normalised, not filtered, and dead zones are the title's job. Fed only while the host
//!   GameInput itself reports no gamepad reading, so a pad GameInput already serves is not
//!   doubled. `SetRumbleState` on the virtual pad sends the low/high frequency motors to that
//!   pad through `XInputSetState`; the trigger motors have no XInput equivalent and stay stored.
//! - Keyboard and mouse buttons (`from_keys`), sampled with `GetAsyncKeyState` only while the
//!   foreground window belongs to this process and is not one of the emulator's own dialogs.
//!   Off once the title asks GameInput for keyboard readings: it then reads the host keyboard
//!   itself and the mapping would double every key. Mapping: WASD left stick, arrows right
//!   stick (diagonals on the unit circle), Space A, Backspace B, E X, R Y, Q LB, F RB, Z or
//!   right mouse LT, X or left mouse RT, Left Shift LS, C RS, I/J/K/L d-pad, Esc Menu, Tab View.
//!
//! The two host sources are combined (`combine`), then merged with the scripted pad
//! (`tools\dev\pad.ps1`) by `merge`: while a command file is running, or while the scripted state
//! is not neutral (a `down` without its `up`), the scripted state is published as-is and host
//! input is ignored; otherwise the host state is published.
//!
//! The virtual pad plugs in when an XInput pad starts feeding it or on the first mapped key, and
//! unplugs when the XInput pad stops feeding it while the keyboard mapping is off.
//!
//! Configuration (environment first, then `tombox.cfg`): `TOMBOX_PAD_XINPUT` / `pad_xinput` and
//! `TOMBOX_PAD_KEYS` / `pad_keys`; `0` turns the source off, `force` (or `1`) keeps it on even
//! when GameInput already serves a pad / the title reads the keyboard, anything else is the
//! default described above. `TOMBOX_PAD=0` turns the whole virtual pad off.
//! `TOMBOX_HOST_PAD=0` / `TOMBOX_HOST_KBM=0` (host_input.rs) also turn the matching source off, until a takeover.

use crate::gamepad::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(vk: i32) -> i16;
    fn GetForegroundWindow() -> *mut c_void;
    fn GetWindowThreadProcessId(h: *mut c_void, pid: *mut u32) -> u32;
    fn GetClassNameW(h: *mut c_void, buf: *mut u16, n: i32) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(m: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
}

/// `XINPUT_GAMEPAD` (Xinput.h).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XinputGamepad {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XinputState {
    packet: u32,
    pad: XinputGamepad,
}

#[repr(C)]
struct XinputVibration {
    left: u16,
    right: u16,
}

/// `XINPUT_GAMEPAD_*` button bit -> `GameInputGamepadButtons` bit.
pub const XINPUT_BUTTONS: [(u16, u32); 14] = [
    (0x0001, BTN_UP),
    (0x0002, BTN_DOWN),
    (0x0004, BTN_LEFT),
    (0x0008, BTN_RIGHT),
    (0x0010, BTN_MENU),
    (0x0020, BTN_VIEW),
    (0x0040, BTN_LS),
    (0x0080, BTN_RS),
    (0x0100, BTN_LB),
    (0x0200, BTN_RB),
    (0x1000, BTN_A),
    (0x2000, BTN_B),
    (0x4000, BTN_X),
    (0x8000, BTN_Y),
];

/// Thumbstick value to [-1, 1]; positive Y is up in both XInput and GameInput.
pub fn thumb(v: i16) -> f32 {
    if v < 0 { v as f32 / 32768.0 } else { v as f32 / 32767.0 }
}

pub fn from_xinput(g: &XinputGamepad) -> PadState {
    let mut s = PadState::default();
    for (x, b) in XINPUT_BUTTONS {
        if g.buttons & x != 0 {
            s.buttons |= b;
        }
    }
    s.axes[AX_LT] = g.left_trigger as f32 / 255.0;
    s.axes[AX_RT] = g.right_trigger as f32 / 255.0;
    s.axes[AX_LX] = thumb(g.thumb_lx);
    s.axes[AX_LY] = thumb(g.thumb_ly);
    s.axes[AX_RX] = thumb(g.thumb_rx);
    s.axes[AX_RY] = thumb(g.thumb_ry);
    s
}

/// Default keyboard mapping: virtual-key code -> button.
pub const KEY_BUTTONS: [(i32, u32); 14] = [
    (0x20, BTN_A),          // Space
    (0x08, BTN_B),          // Backspace
    (b'E' as i32, BTN_X),
    (b'R' as i32, BTN_Y),
    (b'Q' as i32, BTN_LB),
    (b'F' as i32, BTN_RB),
    (0xA0, BTN_LS),         // Left Shift
    (b'C' as i32, BTN_RS),
    (b'I' as i32, BTN_UP),
    (b'K' as i32, BTN_DOWN),
    (b'J' as i32, BTN_LEFT),
    (b'L' as i32, BTN_RIGHT),
    (0x1B, BTN_MENU),       // Esc
    (0x09, BTN_VIEW),       // Tab
];

/// Keys and mouse buttons that pull a trigger fully.
pub const KEY_TRIGGERS: [(i32, usize); 4] = [
    (b'Z' as i32, AX_LT),
    (b'X' as i32, AX_RT),
    (0x02, AX_LT), // right mouse button
    (0x01, AX_RT), // left mouse button
];

/// (negative key, positive key, x axis, y axis) per stick: WASD left, arrows right.
pub const KEY_STICKS: [([i32; 4], usize, usize); 2] = [
    ([b'A' as i32, b'D' as i32, b'S' as i32, b'W' as i32], AX_LX, AX_LY),
    ([0x25, 0x27, 0x28, 0x26], AX_RX, AX_RY),
];

pub fn from_keys(down: impl Fn(i32) -> bool) -> PadState {
    let mut s = PadState::default();
    for (vk, b) in KEY_BUTTONS {
        if down(vk) {
            s.buttons |= b;
        }
    }
    for (vk, a) in KEY_TRIGGERS {
        if down(vk) {
            s.axes[a] = 1.0;
        }
    }
    let dir = |neg: i32, pos: i32| (down(pos) as i32 - down(neg) as i32) as f32;
    for ([l, r, d, u], ax, ay) in KEY_STICKS {
        let (x, y) = (dir(l, r), dir(d, u));
        let k = if x != 0.0 && y != 0.0 { core::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
        s.axes[ax] = x * k;
        s.axes[ay] = y * k;
    }
    s
}

/// Both host sources at once: buttons are OR-ed; each stick and each trigger comes from the
/// source that deflects it further (a stick is taken whole, never one axis from each source).
pub fn combine(a: PadState, b: PadState) -> PadState {
    let mut s = PadState { buttons: a.buttons | b.buttons, axes: a.axes };
    for t in [AX_LT, AX_RT] {
        if b.axes[t] > s.axes[t] {
            s.axes[t] = b.axes[t];
        }
    }
    for (x, y) in [(AX_LX, AX_LY), (AX_RX, AX_RY)] {
        if b.axes[x].powi(2) + b.axes[y].powi(2) > a.axes[x].powi(2) + a.axes[y].powi(2) {
            s.axes[x] = b.axes[x];
            s.axes[y] = b.axes[y];
        }
    }
    s
}

/// The published pad: the scripted state wins while a script runs or holds anything.
pub fn merge(script: PadState, script_active: bool, host: PadState) -> PadState {
    if script_active || script != PadState::default() { script } else { host }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Off,
    Auto,
    Force,
}

pub fn parse_mode(v: Option<&str>) -> Mode {
    match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("0" | "off" | "false" | "no") => Mode::Off,
        Some("1" | "on" | "force" | "always") => Mode::Force,
        _ => Mode::Auto,
    }
}

fn setting(env: &str, key: &str) -> Mode {
    parse_mode(std::env::var(env).ok().or_else(|| crate::cfg(key)).as_deref())
}

/// Windows of this process that are the emulator's own UI, not the title's: system dialogs
/// (gdk/win.rs) and the front panel (xfrontpaneldisplay). On the console those take input away
/// from the title.
pub const HELPER_CLASSES: [&str; 2] = [crate::gdk::win::DIALOG_CLASS, "TomBoxFrontPanel"];

pub fn is_title_class(class: &str) -> bool {
    !HELPER_CLASSES.contains(&class)
}

/// The foreground window belongs to this process and is not one of the emulator's dialogs.
pub fn title_foreground() -> bool {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        if pid != GetCurrentProcessId() {
            return false;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(h, cls.as_mut_ptr(), cls.len() as i32).max(0) as usize;
        is_title_class(&String::from_utf16_lossy(&cls[..n]))
    }
}

type GetState = unsafe extern "system" fn(u32, *mut XinputState) -> u32;
type SetState = unsafe extern "system" fn(u32, *mut XinputVibration) -> u32;

static XI_GET: AtomicUsize = AtomicUsize::new(0);
static XI_SET: AtomicUsize = AtomicUsize::new(0);
const NO_SLOT: u32 = u32::MAX;
static FEED_SLOT: AtomicU32 = AtomicU32::new(NO_SLOT);

fn load_xinput() -> bool {
    unsafe {
        let name: Vec<u16> = "xinput1_4.dll".encode_utf16().chain(Some(0)).collect();
        let m = LoadLibraryW(name.as_ptr());
        if m.is_null() {
            return false;
        }
        let g = GetProcAddress(m, b"XInputGetState\0".as_ptr());
        let s = GetProcAddress(m, b"XInputSetState\0".as_ptr());
        XI_GET.store(g as usize, Ordering::SeqCst);
        XI_SET.store(s as usize, Ordering::SeqCst);
        !g.is_null()
    }
}

fn xinput_state(slot: u32) -> Option<XinputGamepad> {
    let f = XI_GET.load(Ordering::SeqCst);
    if f == 0 {
        return None;
    }
    let f: GetState = unsafe { core::mem::transmute(f) };
    let mut st = XinputState::default();
    (unsafe { f(slot, &mut st) } == 0).then_some(st.pad)
}

/// Motor speed for `XINPUT_VIBRATION` from a GameInput rumble level in [0, 1].
pub fn motor_speed(v: f32) -> u16 {
    (if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 } * 65535.0).round() as u16
}

/// Sends the low/high frequency motors to the XInput pad feeding the virtual pad. Returns its
/// user index when it was sent.
pub fn forward_rumble(low: f32, high: f32) -> Option<u32> {
    let slot = FEED_SLOT.load(Ordering::SeqCst);
    let f = XI_SET.load(Ordering::SeqCst);
    if slot == NO_SLOT || f == 0 {
        return None;
    }
    let f: SetState = unsafe { core::mem::transmute(f) };
    let mut v = XinputVibration { left: motor_speed(low), right: motor_speed(high) };
    (unsafe { f(slot, &mut v) } == 0).then_some(slot)
}

const TICK: Duration = Duration::from_millis(4);
const SCAN: Duration = Duration::from_secs(1);
const HOST_CHECK: Duration = Duration::from_secs(2);

fn log(func: &str, detail: &str) {
    crate::record_raw("gamepad", func, detail);
}

/// Starts the host input thread once (called when the title creates its IGameInput).
pub fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let xi = setting("TOMBOX_PAD_XINPUT", "pad_xinput");
        let keys = setting("TOMBOX_PAD_KEYS", "pad_keys");
        if xi == Mode::Off && keys == Mode::Off {
            log("host", "XInput and keyboard sources both off; virtual pad is scripted only");
            return;
        }
        let xi = if xi != Mode::Off && !load_xinput() {
            log("host", "xinput1_4.dll not available - real controllers not polled");
            Mode::Off
        } else {
            xi
        };
        log("host", &format!("host input to the virtual pad: XInput {xi:?}, keyboard mapping {keys:?}"));
        let _ = std::thread::Builder::new().name("tombox-pad-host".into()).spawn(move || poll_loop(xi, keys));
    });
}

fn poll_loop(xi: Mode, keys: Mode) {
    let mut slot: Option<u32> = None;
    let mut last_scan: Option<Instant> = None;
    let mut last_check: Option<Instant> = None;
    let mut host_pad = false;
    let mut feeding = false;
    let mut keys_were_on = false;
    loop {
        let mut pad = PadState::default();
        if xi != Mode::Off {
            if slot.is_none() && last_scan.map_or(true, |t| t.elapsed() >= SCAN) {
                last_scan = Some(Instant::now());
                slot = (0..4).find(|i| xinput_state(*i).is_some());
                if let Some(i) = slot {
                    log("host", &format!("XInput pad connected at user index {i}"));
                    last_check = None;
                }
            }
            if let Some(i) = slot {
                match xinput_state(i) {
                    Some(g) => pad = from_xinput(&g),
                    None => {
                        log("host", &format!("XInput pad at user index {i} disconnected"));
                        slot = None;
                    }
                }
            }
            if slot.is_some() && xi == Mode::Auto && last_check.map_or(true, |t| t.elapsed() >= HOST_CHECK) {
                last_check = Some(Instant::now());
                let has = host_has_gamepad();
                if has != host_pad {
                    log("host", if has { "host GameInput reports a gamepad itself - XInput feed paused so the pad is not doubled" } else { "host GameInput reports no gamepad - XInput feeds the virtual pad" });
                }
                host_pad = has;
            }
        }
        // TOMBOX_HOST_KBM=0 hides the host keyboard from the title (host_input.rs); the mapping must not bring it back.
        let keys_on = !crate::host_input::kbm_hidden() && (keys == Mode::Force || (keys == Mode::Auto && !title_reads_keyboard()));
        if keys_were_on && !keys_on {
            log("host", "title reads the keyboard through GameInput - keyboard-to-pad mapping off");
        }
        keys_were_on = keys_on;
        let kb = if keys_on && title_foreground() { from_keys(|vk| unsafe { GetAsyncKeyState(vk) } < 0) } else { PadState::default() };
        // Same for TOMBOX_HOST_PAD=0: a hidden host gamepad does not come back through the virtual pad.
        let feed = slot.filter(|_| !crate::host_input::pad_hidden() && (xi == Mode::Force || !host_pad));
        FEED_SLOT.store(feed.unwrap_or(NO_SLOT), Ordering::SeqCst);
        set_host(combine(if feed.is_some() { pad } else { PadState::default() }, kb));
        if feed.is_some() && !feeding {
            host_connect(true, "XInput pad");
        } else if feed.is_none() && feeding && !keys_on {
            host_connect(false, "XInput pad gone");
        }
        if kb != PadState::default() {
            host_connect(true, "keyboard mapping");
        }
        feeding = feed.is_some();
        std::thread::sleep(TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn xinput_buttons_map_to_gameinput_bits() {
        let all = from_xinput(&XinputGamepad { buttons: 0xF3FF, ..Default::default() });
        assert_eq!(all.buttons, 0x3FFF);
        let a = from_xinput(&XinputGamepad { buttons: 0x1000 | 0x0010 | 0x0001, ..Default::default() });
        assert_eq!(a.buttons, BTN_A | BTN_MENU | BTN_UP);
        assert_eq!(from_xinput(&XinputGamepad { buttons: 0x0C00, ..Default::default() }).buttons, 0);
    }

    #[test]
    fn xinput_axes_normalise_to_gameinput_ranges() {
        let g = XinputGamepad { left_trigger: 255, right_trigger: 0, thumb_lx: -32768, thumb_ly: 32767, thumb_rx: 0, thumb_ry: -16384, ..Default::default() };
        let s = from_xinput(&g);
        assert_eq!(s.axes[AX_LT], 1.0);
        assert_eq!(s.axes[AX_RT], 0.0);
        assert_eq!(s.axes[AX_LX], -1.0);
        assert_eq!(s.axes[AX_LY], 1.0);
        assert_eq!(s.axes[AX_RX], 0.0);
        assert!(close(s.axes[AX_RY], -0.5));
        assert!(close(from_xinput(&XinputGamepad { left_trigger: 128, ..Default::default() }).axes[AX_LT], 128.0 / 255.0));
    }

    #[test]
    fn xinput_has_no_dead_zone_like_gameinput() {
        let s = from_xinput(&XinputGamepad { thumb_lx: 1, thumb_ly: -1, right_trigger: 1, ..Default::default() });
        assert!(s.axes[AX_LX] > 0.0 && s.axes[AX_LX] < 1e-4);
        assert!(s.axes[AX_LY] < 0.0 && s.axes[AX_LY] > -1e-4);
        assert!(s.axes[AX_RT] > 0.0);
        assert_eq!(from_xinput(&XinputGamepad::default()), PadState::default());
        for v in [i16::MIN, -1, 0, 1, i16::MAX] {
            assert!((-1.0..=1.0).contains(&thumb(v)));
        }
    }

    #[test]
    fn keys_map_to_buttons_sticks_and_triggers() {
        let set = |keys: &'static [i32]| from_keys(move |vk| keys.contains(&vk));
        assert_eq!(set(&[]), PadState::default());
        let s = set(&[b'W' as i32, 0x20, 0x01]);
        assert_eq!(s.buttons, BTN_A);
        assert_eq!(s.axes[AX_LY], 1.0);
        assert_eq!(s.axes[AX_LX], 0.0);
        assert_eq!(s.axes[AX_RT], 1.0);
        let s = set(&[b'A' as i32, b'D' as i32, 0x28]);
        assert_eq!(s.axes[AX_LX], 0.0, "opposite keys cancel");
        assert_eq!(s.axes[AX_RY], -1.0);
        let s = set(&[b'W' as i32, b'D' as i32]);
        assert!(close(s.axes[AX_LX], core::f32::consts::FRAC_1_SQRT_2));
        assert!(close(s.axes[AX_LX].hypot(s.axes[AX_LY]), 1.0), "diagonal stays on the unit circle");
        let s = set(&[0x1B, 0x09, b'I' as i32, b'L' as i32, 0xA0, b'C' as i32, b'Q' as i32, b'F' as i32, b'E' as i32, b'R' as i32, 0x08]);
        assert_eq!(s.buttons, BTN_MENU | BTN_VIEW | BTN_UP | BTN_RIGHT | BTN_LS | BTN_RS | BTN_LB | BTN_RB | BTN_X | BTN_Y | BTN_B);
        assert_eq!(set(&[b'Z' as i32, 0x02]).axes[AX_LT], 1.0);
    }

    #[test]
    fn combine_ors_buttons_and_takes_the_larger_stick_whole() {
        let mut pad = PadState::default();
        pad.buttons = BTN_A;
        pad.axes[AX_LX] = 0.3;
        pad.axes[AX_LY] = 0.3;
        pad.axes[AX_RT] = 0.4;
        let mut kb = PadState::default();
        kb.buttons = BTN_B;
        kb.axes[AX_LY] = 1.0;
        kb.axes[AX_RX] = -1.0;
        let s = combine(pad, kb);
        assert_eq!(s.buttons, BTN_A | BTN_B);
        assert_eq!((s.axes[AX_LX], s.axes[AX_LY]), (0.0, 1.0));
        assert_eq!((s.axes[AX_RX], s.axes[AX_RY]), (-1.0, 0.0));
        assert!(close(s.axes[AX_RT], 0.4));
        assert_eq!(combine(pad, PadState::default()), pad);
        assert_eq!(combine(PadState::default(), kb), kb);
    }

    #[test]
    fn scripted_pad_wins_while_running_or_holding() {
        let mut host = PadState::default();
        host.buttons = BTN_X;
        host.axes[AX_LX] = 1.0;
        let neutral = PadState::default();
        assert_eq!(merge(neutral, false, host), host);
        assert_eq!(merge(neutral, true, host), neutral, "a running script masks host input even between presses");
        let mut held = PadState::default();
        held.apply(Input::Button(BTN_A), true);
        assert_eq!(merge(held, false, host), held, "a scripted 'down' masks host input until its 'up'");
        assert_eq!(merge(held, true, neutral), held);
    }

    #[test]
    fn modes_and_title_window_classes() {
        assert_eq!(parse_mode(None), Mode::Auto);
        assert_eq!(parse_mode(Some(" 0 ")), Mode::Off);
        assert_eq!(parse_mode(Some("off")), Mode::Off);
        assert_eq!(parse_mode(Some("Force")), Mode::Force);
        assert_eq!(parse_mode(Some("1")), Mode::Force);
        assert_eq!(parse_mode(Some("auto")), Mode::Auto);
        assert!(is_title_class("UnrealWindow"));
        assert!(is_title_class("XboxEmulatorHostWindow"));
        assert!(!is_title_class("TomBoxGdkDialog"));
        assert!(!is_title_class("TomBoxFrontPanel"));
    }

    #[test]
    fn rumble_levels_to_xinput_motor_speeds() {
        assert_eq!(motor_speed(0.0), 0);
        assert_eq!(motor_speed(1.0), 65535);
        assert_eq!(motor_speed(2.0), 65535);
        assert_eq!(motor_speed(-1.0), 0);
        assert_eq!(motor_speed(f32::NAN), 0);
        assert_eq!(motor_speed(0.5), 32768);
    }
}
