use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

extern "system" {
    fn GetModuleHandleW(n: *const u16) -> *mut u8;
    fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old_protect: *mut u32) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn QueryPerformanceCounter(v: *mut i64) -> i32;
    fn QueryPerformanceFrequency(v: *mut i64) -> i32;
}

pub const KIND_RAW_DEVICE_REPORT: u32 = 0x0000_0001;
pub const KIND_CONTROLLER_AXIS: u32 = 0x0000_0002;
pub const KIND_CONTROLLER_BUTTON: u32 = 0x0000_0004;
pub const KIND_CONTROLLER_SWITCH: u32 = 0x0000_0008;
pub const KIND_KEYBOARD: u32 = 0x0000_0010;
pub const KIND_MOUSE: u32 = 0x0000_0020;
pub const KIND_TOUCH: u32 = 0x0000_0100;
pub const KIND_MOTION: u32 = 0x0000_1000;
pub const KIND_ARCADE_STICK: u32 = 0x0001_0000;
pub const KIND_FLIGHT_STICK: u32 = 0x0002_0000;
pub const KIND_GAMEPAD: u32 = 0x0004_0000;
pub const KIND_RACING_WHEEL: u32 = 0x0008_0000;
pub const KIND_UI_NAVIGATION: u32 = 0x0100_0000;
pub const KIND_KBM: u32 = KIND_KEYBOARD | KIND_MOUSE;

pub const STATUS_CONNECTED: u32 = 0x1;
pub const STATUS_INPUT_ENABLED: u32 = 0x2;

pub const S_OK: i32 = 0;
pub const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
pub const E_POINTER: i32 = 0x8000_4003u32 as i32;
pub const E_BOUNDS: i32 = 0x8000_000Bu32 as i32;
pub const READING_NOT_FOUND: i32 = 0x838A_0003u32 as i32;
pub const DEVICE_DISCONNECTED: i32 = 0x838A_0001u32 as i32;

pub const BUTTON_LEFT: u32 = 0x01;
pub const BUTTON_RIGHT: u32 = 0x02;
pub const BUTTON_MIDDLE: u32 = 0x04;
pub const BUTTON_4: u32 = 0x08;
pub const BUTTON_5: u32 = 0x10;
pub const BUTTONS_ALL: u32 = 0x1F;

pub const MAX_KEYS: usize = 16;
pub const HISTORY: usize = 32;
pub const DEVICE_INFO_SIZE: usize = 0x140;
pub const INFO_DEVICE_ID: usize = 0x20;
pub const INFO_ROOT_ID: usize = 0x40;
pub const INFO_FAMILY: usize = 0x60;
pub const INFO_SUPPORTED_INPUT: usize = 0x68;
pub const INFO_SUPPORTED_RUMBLE: usize = 0x6C;
pub const INFO_KEYBOARD_INFO: usize = 0xD0;
pub const INFO_MOUSE_INFO: usize = 0xD8;
pub const INFO_DISPLAY_NAME: usize = 0x128;
pub const MOTION_STATE_SIZE: usize = 0x44;
pub const ARCADE_STICK_STATE_SIZE: usize = 4;
pub const FLIGHT_STICK_STATE_SIZE: usize = 0x18;
pub const GAMEPAD_STATE_SIZE: usize = 0x1C;
pub const RACING_WHEEL_STATE_SIZE: usize = 0x1C;
pub const UI_NAVIGATION_STATE_SIZE: usize = 4;

pub const KEYBOARD_ID: [u8; 32] = *b"TomBoxVirtualKeyboard00000000001";
pub const MOUSE_ID: [u8; 32] = *b"TomBoxVirtualMouse00000000000001";

pub const KIND_NAMES: [(u32, &str); 13] = [
    (KIND_RAW_DEVICE_REPORT, "RawDeviceReport"),
    (KIND_CONTROLLER_AXIS, "ControllerAxis"),
    (KIND_CONTROLLER_BUTTON, "ControllerButton"),
    (KIND_CONTROLLER_SWITCH, "ControllerSwitch"),
    (KIND_KEYBOARD, "Keyboard"),
    (KIND_MOUSE, "Mouse"),
    (KIND_TOUCH, "Touch"),
    (KIND_MOTION, "Motion"),
    (KIND_ARCADE_STICK, "ArcadeStick"),
    (KIND_FLIGHT_STICK, "FlightStick"),
    (KIND_GAMEPAD, "Gamepad"),
    (KIND_RACING_WHEEL, "RacingWheel"),
    (KIND_UI_NAVIGATION, "UiNavigation"),
];

pub fn kind_names(kind: u32) -> String {
    if kind == 0 {
        return "Unknown".into();
    }
    let mut parts = Vec::new();
    let mut rest = kind;
    for (bit, name) in KIND_NAMES {
        if kind & bit != 0 {
            parts.push(name.to_string());
            rest &= !bit;
        }
    }
    if rest != 0 {
        parts.push(format!("{rest:#x}"));
    }
    parts.join("|")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Api {
    GetCurrentReading,
    GetNextReading,
    GetPreviousReading,
    GetTemporalReading,
    RegisterReadingCallback,
    RegisterDeviceCallback,
}

impl Api {
    pub fn name(self) -> &'static str {
        match self {
            Api::GetCurrentReading => "GetCurrentReading",
            Api::GetNextReading => "GetNextReading",
            Api::GetPreviousReading => "GetPreviousReading",
            Api::GetTemporalReading => "GetTemporalReading",
            Api::RegisterReadingCallback => "RegisterReadingCallback",
            Api::RegisterDeviceCallback => "RegisterDeviceCallback",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    AnyDevice,
    VirtualPad,
    VirtualKeyboard,
    VirtualMouse,
    HostDevice,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::AnyDevice => "any-device",
            Target::VirtualPad => "virtual-pad",
            Target::VirtualKeyboard => "virtual-keyboard",
            Target::VirtualMouse => "virtual-mouse",
            Target::HostDevice => "host-device",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindRecord {
    pub api: Api,
    pub kind: u32,
    pub filter: u32,
    pub target: Target,
    pub count: u64,
}

#[derive(Clone, Debug, Default)]
pub struct KindLog {
    pub records: Vec<KindRecord>,
}

impl KindLog {
    pub fn note(&mut self, api: Api, kind: u32, filter: u32, target: Target) -> KindRecord {
        if let Some(r) = self.records.iter_mut().find(|r| r.api == api && r.kind == kind && r.filter == filter && r.target == target) {
            r.count += 1;
            return *r;
        }
        let r = KindRecord { api, kind, filter, target, count: 1 };
        if self.records.len() < 512 {
            self.records.push(r);
        }
        r
    }
    pub fn requested(&self) -> u32 {
        self.records.iter().fold(0, |a, r| a | r.kind)
    }
    pub fn requested_by(&self, api: Api) -> u32 {
        self.records.iter().filter(|r| r.api == api).fold(0, |a, r| a | r.kind)
    }
    pub fn report(&self) -> String {
        let mut v = self.records.clone();
        v.sort_by_key(|r| (r.api, r.kind, r.filter, r.target));
        v.iter()
            .map(|r| format!("{} kind={:#x} ({}) filter={:#x} target={} calls={}", r.api.name(), r.kind, kind_names(r.kind), r.filter, r.target.name(), r.count))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn should_log_count(n: u64) -> bool {
    n == 1 || (n >= 10 && n.is_power_of_two()) || n % 100_000 == 0
}

static KIND_LOG: Mutex<KindLog> = Mutex::new(KindLog { records: Vec::new() });

fn log(func: &str, detail: &str) {
    crate::record_raw("gameinput_ext", func, detail);
}

pub fn note_kind(api: Api, kind: u32, filter: u32, target: Target) {
    let r = KIND_LOG.lock().unwrap_or_else(|e| e.into_inner()).note(api, kind, filter, target);
    if should_log_count(r.count) {
        log("kinds", &format!("{} kind={:#x} ({}) filter={:#x} target={} calls={}", api.name(), kind, kind_names(kind), filter, target.name(), r.count));
    }
}

pub fn requested_kinds() -> u32 {
    KIND_LOG.lock().unwrap_or_else(|e| e.into_inner()).requested()
}

pub fn kind_report() -> String {
    KIND_LOG.lock().unwrap_or_else(|e| e.into_inner()).report()
}

pub fn kind_records() -> Vec<KindRecord> {
    KIND_LOG.lock().unwrap_or_else(|e| e.into_inner()).records.clone()
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KeyState {
    pub scan_code: u32,
    pub code_point: u32,
    pub virtual_key: u8,
    pub is_dead_key: u8,
    pub reserved: [u8; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MouseState {
    pub buttons: u32,
    pub reserved: u32,
    pub position_x: i64,
    pub position_y: i64,
    pub wheel_x: i64,
    pub wheel_y: i64,
}

impl MouseState {
    pub fn to_bytes(&self) -> [u8; 40] {
        let mut o = [0u8; 40];
        o[0..4].copy_from_slice(&self.buttons.to_le_bytes());
        o[8..16].copy_from_slice(&self.position_x.to_le_bytes());
        o[16..24].copy_from_slice(&self.position_y.to_le_bytes());
        o[24..32].copy_from_slice(&self.wheel_x.to_le_bytes());
        o[32..40].copy_from_slice(&self.wheel_y.to_le_bytes());
        o
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RumbleParams {
    pub low_frequency: f32,
    pub high_frequency: f32,
    pub left_trigger: f32,
    pub right_trigger: f32,
}

fn clamp01(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

impl RumbleParams {
    pub fn clamped(&self) -> RumbleParams {
        RumbleParams {
            low_frequency: clamp01(self.low_frequency),
            high_frequency: clamp01(self.high_frequency),
            left_trigger: clamp01(self.left_trigger),
            right_trigger: clamp01(self.right_trigger),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyboardInfo {
    pub kind: i32,
    pub layout: u32,
    pub key_count: u32,
    pub function_key_count: u32,
    pub max_simultaneous_keys: u32,
    pub platform_type: u32,
    pub platform_subtype: u32,
    pub native_language: *const GiString,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseInfo {
    pub supported_buttons: u32,
    pub sample_rate: u32,
    pub sensor_dpi: u32,
    pub has_wheel_x: bool,
    pub has_wheel_y: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GiString {
    pub size_in_bytes: u32,
    pub code_point_count: u32,
    pub data: *const u8,
    pub code_points: *const u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyDef {
    pub vk: u8,
    pub scan: u32,
    pub lower: u32,
    pub upper: u32,
}

const fn kd(vk: u8, scan: u32, lower: u32, upper: u32) -> KeyDef {
    KeyDef { vk, scan, lower, upper }
}

const LETTER_SCANS: [u32; 26] = [
    0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32,
    0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
];
const DIGIT_SHIFTED: [u8; 10] = *b")!@#$%^&*(";

const NAMED_KEYS: [(&str, KeyDef); 52] = [
    ("esc", kd(0x1B, 0x01, 0x1B, 0x1B)),
    ("escape", kd(0x1B, 0x01, 0x1B, 0x1B)),
    ("tab", kd(0x09, 0x0F, 0x09, 0x09)),
    ("enter", kd(0x0D, 0x1C, 0x0D, 0x0D)),
    ("return", kd(0x0D, 0x1C, 0x0D, 0x0D)),
    ("space", kd(0x20, 0x39, 0x20, 0x20)),
    ("backspace", kd(0x08, 0x0E, 0x08, 0x08)),
    ("shift", kd(0xA0, 0x2A, 0, 0)),
    ("lshift", kd(0xA0, 0x2A, 0, 0)),
    ("rshift", kd(0xA1, 0x36, 0, 0)),
    ("ctrl", kd(0xA2, 0x1D, 0, 0)),
    ("lctrl", kd(0xA2, 0x1D, 0, 0)),
    ("rctrl", kd(0xA3, 0xE01D, 0, 0)),
    ("alt", kd(0xA4, 0x38, 0, 0)),
    ("lalt", kd(0xA4, 0x38, 0, 0)),
    ("ralt", kd(0xA5, 0xE038, 0, 0)),
    ("lwin", kd(0x5B, 0xE05B, 0, 0)),
    ("capslock", kd(0x14, 0x3A, 0, 0)),
    ("up", kd(0x26, 0xE048, 0, 0)),
    ("down", kd(0x28, 0xE050, 0, 0)),
    ("left", kd(0x25, 0xE04B, 0, 0)),
    ("right", kd(0x27, 0xE04D, 0, 0)),
    ("insert", kd(0x2D, 0xE052, 0, 0)),
    ("delete", kd(0x2E, 0xE053, 0x7F, 0x7F)),
    ("home", kd(0x24, 0xE047, 0, 0)),
    ("end", kd(0x23, 0xE04F, 0, 0)),
    ("pageup", kd(0x21, 0xE049, 0, 0)),
    ("pagedown", kd(0x22, 0xE051, 0, 0)),
    ("f1", kd(0x70, 0x3B, 0, 0)),
    ("f2", kd(0x71, 0x3C, 0, 0)),
    ("f3", kd(0x72, 0x3D, 0, 0)),
    ("f4", kd(0x73, 0x3E, 0, 0)),
    ("f5", kd(0x74, 0x3F, 0, 0)),
    ("f6", kd(0x75, 0x40, 0, 0)),
    ("f7", kd(0x76, 0x41, 0, 0)),
    ("f8", kd(0x77, 0x42, 0, 0)),
    ("f9", kd(0x78, 0x43, 0, 0)),
    ("f10", kd(0x79, 0x44, 0, 0)),
    ("f11", kd(0x7A, 0x57, 0, 0)),
    ("f12", kd(0x7B, 0x58, 0, 0)),
    ("minus", kd(0xBD, 0x0C, b'-' as u32, b'_' as u32)),
    ("equals", kd(0xBB, 0x0D, b'=' as u32, b'+' as u32)),
    ("lbracket", kd(0xDB, 0x1A, b'[' as u32, b'{' as u32)),
    ("rbracket", kd(0xDD, 0x1B, b']' as u32, b'}' as u32)),
    ("backslash", kd(0xDC, 0x2B, b'\\' as u32, b'|' as u32)),
    ("semicolon", kd(0xBA, 0x27, b';' as u32, b':' as u32)),
    ("quote", kd(0xDE, 0x28, b'\'' as u32, b'"' as u32)),
    ("grave", kd(0xC0, 0x29, b'`' as u32, b'~' as u32)),
    ("comma", kd(0xBC, 0x33, b',' as u32, b'<' as u32)),
    ("period", kd(0xBE, 0x34, b'.' as u32, b'>' as u32)),
    ("slash", kd(0xBF, 0x35, b'/' as u32, b'?' as u32)),
    ("pause", kd(0x13, 0xE11D, 0, 0)),
];

pub fn parse_key(name: &str) -> Option<KeyDef> {
    let n = name.trim().to_ascii_lowercase().replace(['-', '_'], "");
    if n.len() == 1 {
        let c = n.as_bytes()[0];
        if c.is_ascii_lowercase() {
            let i = (c - b'a') as usize;
            return Some(kd(b'A' + i as u8, LETTER_SCANS[i], c as u32, c.to_ascii_uppercase() as u32));
        }
        if c.is_ascii_digit() {
            let d = (c - b'0') as usize;
            let scan = if d == 0 { 0x0B } else { 0x01 + d as u32 };
            return Some(kd(c, scan, c as u32, DIGIT_SHIFTED[d] as u32));
        }
    }
    if let Some(hex) = n.strip_prefix("vk:").or_else(|| n.strip_prefix("vk")) {
        let v = u8::from_str_radix(hex.trim_start_matches("0x"), 16).ok()?;
        return Some(kd(v, 0, 0, 0));
    }
    NAMED_KEYS.iter().find(|(k, _)| *k == n).map(|(_, d)| *d)
}

pub fn parse_button(name: &str) -> Option<u32> {
    match name.trim().to_ascii_lowercase().as_str() {
        "left" | "l" | "lmb" | "1" => Some(BUTTON_LEFT),
        "right" | "r" | "rmb" | "2" => Some(BUTTON_RIGHT),
        "middle" | "m" | "mmb" | "3" => Some(BUTTON_MIDDLE),
        "x1" | "button4" | "back" | "4" => Some(BUTTON_4),
        "x2" | "button5" | "forward" | "5" => Some(BUTTON_5),
        _ => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KbmState {
    pub keys: Vec<KeyDef>,
    pub mouse: MouseState,
}

fn is_shift(vk: u8) -> bool {
    matches!(vk, 0x10 | 0xA0 | 0xA1)
}

impl KbmState {
    pub fn key(&mut self, k: KeyDef, down: bool) -> bool {
        let pos = self.keys.iter().position(|x| x.vk == k.vk && x.scan == k.scan);
        match (down, pos) {
            (true, None) if self.keys.len() < MAX_KEYS => {
                self.keys.push(k);
                true
            }
            (false, Some(i)) => {
                self.keys.remove(i);
                true
            }
            _ => false,
        }
    }
    pub fn shift_held(&self) -> bool {
        self.keys.iter().any(|k| is_shift(k.vk))
    }
    pub fn key_states(&self) -> Vec<KeyState> {
        let shift = self.shift_held();
        self.keys
            .iter()
            .map(|k| KeyState { scan_code: k.scan, code_point: if shift { k.upper } else { k.lower }, virtual_key: k.vk, is_dead_key: 0, reserved: [0; 2] })
            .collect()
    }
    pub fn button(&mut self, b: u32, down: bool) -> bool {
        let before = self.mouse.buttons;
        if down { self.mouse.buttons |= b & BUTTONS_ALL } else { self.mouse.buttons &= !b }
        before != self.mouse.buttons
    }
    pub fn move_by(&mut self, dx: i64, dy: i64) -> bool {
        self.mouse.position_x = self.mouse.position_x.wrapping_add(dx);
        self.mouse.position_y = self.mouse.position_y.wrapping_add(dy);
        dx != 0 || dy != 0
    }
    pub fn wheel(&mut self, dx: i64, dy: i64) -> bool {
        self.mouse.wheel_x = self.mouse.wheel_x.wrapping_add(dx);
        self.mouse.wheel_y = self.mouse.wheel_y.wrapping_add(dy);
        dx != 0 || dy != 0
    }
    pub fn release(&mut self) -> (bool, bool) {
        let k = !self.keys.is_empty();
        self.keys.clear();
        let m = self.mouse.buttons != 0;
        self.mouse.buttons = 0;
        (k, m)
    }
}

pub fn fill_key_states(states: &[KeyState], count: u32, out: *mut KeyState) -> u32 {
    if out.is_null() {
        return 0;
    }
    let n = (count as usize).min(states.len());
    unsafe { core::ptr::copy_nonoverlapping(states.as_ptr(), out, n) };
    n as u32
}

#[derive(Clone, Debug, PartialEq)]
pub enum KbmCmd {
    KeyTap(Vec<KeyDef>, u64),
    KeyDown(Vec<KeyDef>),
    KeyUp(Vec<KeyDef>),
    Move(i64, i64),
    Click(u32, u64),
    ButtonDown(u32),
    ButtonUp(u32),
    Wheel(i64, i64),
    Wait(u64),
    Release,
    Connect(u32),
    Disconnect(u32),
}

pub const DEFAULT_TAP_MS: u64 = 120;
pub const GAP_MS: u64 = 120;

fn parse_ms(s: Option<&&str>) -> Option<u64> {
    s.and_then(|t| t.trim_end_matches("ms").parse::<u64>().ok())
}

fn parse_keys(s: &str) -> Option<Vec<KeyDef>> {
    let v: Option<Vec<KeyDef>> = s.split('+').map(parse_key).collect();
    v.filter(|v| !v.is_empty())
}

fn parse_which(s: Option<&&str>) -> Option<u32> {
    match s.map(|s| s.to_ascii_lowercase()) {
        None => Some(KIND_KBM),
        Some(s) if s == "all" || s == "both" || s == "kbm" => Some(KIND_KBM),
        Some(s) if s == "keyboard" || s == "kb" || s == "kbd" => Some(KIND_KEYBOARD),
        Some(s) if s == "mouse" => Some(KIND_MOUSE),
        _ => None,
    }
}

pub fn parse_kbm_command(line: &str) -> Result<Option<KbmCmd>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
        return Ok(None);
    }
    let t: Vec<&str> = line.split_whitespace().collect();
    let verb = t[0].to_ascii_lowercase();
    let bad = || format!("bad command: {line}");
    let keys = |i: usize| t.get(i).and_then(|s| parse_keys(s)).ok_or_else(bad);
    let button = |i: usize| t.get(i).and_then(|s| parse_button(s)).ok_or_else(bad);
    let int = |i: usize| t.get(i).and_then(|s| s.parse::<i64>().ok()).ok_or_else(bad);
    let cmd = match verb.as_str() {
        "key" | "tap" => KbmCmd::KeyTap(keys(1)?, parse_ms(t.get(2)).unwrap_or(DEFAULT_TAP_MS)),
        "keydown" => KbmCmd::KeyDown(keys(1)?),
        "keyup" => KbmCmd::KeyUp(keys(1)?),
        "move" => KbmCmd::Move(int(1)?, int(2)?),
        "click" => KbmCmd::Click(button(1)?, parse_ms(t.get(2)).unwrap_or(DEFAULT_TAP_MS)),
        "mousedown" => KbmCmd::ButtonDown(button(1)?),
        "mouseup" => KbmCmd::ButtonUp(button(1)?),
        "wheel" => KbmCmd::Wheel(t.get(2).map(|_| int(2)).transpose()?.unwrap_or(0), int(1)?),
        "wait" | "sleep" => KbmCmd::Wait(parse_ms(t.get(1)).ok_or_else(bad)?),
        "release" | "reset" => KbmCmd::Release,
        "connect" | "plug" => KbmCmd::Connect(parse_which(t.get(1)).ok_or_else(bad)?),
        "disconnect" | "unplug" => KbmCmd::Disconnect(parse_which(t.get(1)).ok_or_else(bad)?),
        _ => return Err(bad()),
    };
    Ok(Some(cmd))
}

pub fn device_info_bytes(kind: u32) -> [u8; DEVICE_INFO_SIZE] {
    let mut b = [0u8; DEVICE_INFO_SIZE];
    let kb = kind == KIND_KEYBOARD;
    b[0..4].copy_from_slice(&(DEVICE_INFO_SIZE as u32).to_le_bytes());
    b[4..6].copy_from_slice(&0x045Eu16.to_le_bytes());
    b[6..8].copy_from_slice(&(if kb { 0x07A5u16 } else { 0x07A6u16 }).to_le_bytes());
    b[8..10].copy_from_slice(&1u16.to_le_bytes());
    b[12..14].copy_from_slice(&1u16.to_le_bytes());
    b[14..16].copy_from_slice(&(if kb { 6u16 } else { 2u16 }).to_le_bytes());
    b[16..18].copy_from_slice(&1u16.to_le_bytes());
    b[24..26].copy_from_slice(&1u16.to_le_bytes());
    let id = if kb { KEYBOARD_ID } else { MOUSE_ID };
    b[INFO_DEVICE_ID..INFO_DEVICE_ID + 32].copy_from_slice(&id);
    b[INFO_ROOT_ID..INFO_ROOT_ID + 32].copy_from_slice(&id);
    b[INFO_FAMILY..INFO_FAMILY + 4].copy_from_slice(&3i32.to_le_bytes());
    b[INFO_SUPPORTED_INPUT..INFO_SUPPORTED_INPUT + 4].copy_from_slice(&kind.to_le_bytes());
    b
}

pub fn keyboard_info() -> KeyboardInfo {
    KeyboardInfo { kind: 0, layout: 0x0409, key_count: 104, function_key_count: 12, max_simultaneous_keys: MAX_KEYS as u32, platform_type: 4, platform_subtype: 0, native_language: core::ptr::null() }
}

pub fn mouse_info() -> MouseInfo {
    MouseInfo { supported_buttons: BUTTONS_ALL, sample_rate: 1000, sensor_dpi: 800, has_wheel_x: true, has_wheel_y: true }
}

pub fn default_os_build() -> u32 {
    std::env::var("TOMBOX_GI_OSBUILD")
        .ok()
        .or_else(|| crate::cfg("gameinput_os_build"))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .or_else(crate::title_os::title_os_build)
        .unwrap_or(22000)
}

pub fn device_method_count(os_build: u32) -> usize {
    if os_build >= 22621 { 22 } else { 21 }
}

pub fn igameinput_method_count(os_build: u32) -> usize {
    if os_build < 22000 { 23 } else { 22 }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Feedback {
    pub device: usize,
    pub rumble: RumbleParams,
    pub rumble_calls: u64,
    pub motor_gain: [f32; 4],
    pub haptic_calls: u64,
    pub sync_enabled: bool,
    pub sync_hints: u64,
    pub power_off_calls: u64,
}

static FEEDBACK: Mutex<Vec<Feedback>> = Mutex::new(Vec::new());

fn with_feedback<R>(device: usize, f: impl FnOnce(&mut Feedback) -> R) -> R {
    let mut g = FEEDBACK.lock().unwrap_or_else(|e| e.into_inner());
    let i = match g.iter().position(|x| x.device == device) {
        Some(i) => i,
        None => {
            if g.len() >= 64 {
                g.remove(0);
            }
            g.push(Feedback { device, motor_gain: [1.0; 4], ..Default::default() });
            g.len() - 1
        }
    };
    f(&mut g[i])
}

pub fn feedback_state(device: usize) -> Option<Feedback> {
    FEEDBACK.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|x| x.device == device).copied()
}

pub fn store_rumble(device: usize, params: RumbleParams) -> (RumbleParams, bool) {
    let p = params.clamped();
    with_feedback(device, |f| {
        f.rumble_calls += 1;
        let changed = f.rumble != p;
        f.rumble = p;
        (p, changed)
    })
}

pub unsafe extern "system" fn dev_set_rumble_state(this: *mut c_void, params: *const RumbleParams) {
    let p = if params.is_null() { RumbleParams::default() } else { core::ptr::read_unaligned(params) };
    let (p, changed) = store_rumble(this as usize, p);
    let sent = if this == crate::gamepad::virtual_device() { crate::pad_host::forward_rumble(p.low_frequency, p.high_frequency) } else { None };
    if changed {
        let to = match sent {
            Some(i) => format!("low/high sent to XInput pad {i}; trigger motors stored, XInput has none"),
            None => "stored, not sent to a host device".into(),
        };
        log("SetRumbleState", &format!("device {this:p} low={:.2} high={:.2} lt={:.2} rt={:.2} ({to})",
            p.low_frequency, p.high_frequency, p.left_trigger, p.right_trigger));
    }
}

pub unsafe extern "system" fn dev_set_haptic_motor_state(this: *mut c_void, motor: u32, _params: *const c_void) -> i32 {
    let n = with_feedback(this as usize, |f| {
        f.haptic_calls += 1;
        f.haptic_calls
    });
    if should_log_count(n) {
        log("SetHapticMotorState", &format!("device {this:p} motor={motor} -> E_BOUNDS (no haptic motors reported) calls={n}"));
    }
    E_BOUNDS
}

pub unsafe extern "system" fn dev_create_force_feedback_effect(this: *mut c_void, motor: u32, _params: *const c_void, out: *mut *mut c_void) -> i32 {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    log("CreateForceFeedbackEffect", &format!("device {this:p} motor={motor} -> E_BOUNDS (no force-feedback motors reported)"));
    E_BOUNDS
}

pub unsafe extern "system" fn dev_is_force_feedback_motor_powered_on(_this: *mut c_void, _motor: u32) -> bool {
    false
}

pub unsafe extern "system" fn dev_set_force_feedback_motor_gain(this: *mut c_void, motor: u32, gain: f32) {
    if (motor as usize) < 4 {
        with_feedback(this as usize, |f| f.motor_gain[motor as usize] = clamp01(gain));
    }
}

pub unsafe extern "system" fn dev_set_input_synchronization_state(this: *mut c_void, enabled: u8) {
    with_feedback(this as usize, |f| f.sync_enabled = enabled != 0);
}

pub unsafe extern "system" fn dev_send_input_synchronization_hint(this: *mut c_void) {
    with_feedback(this as usize, |f| f.sync_hints += 1);
}

pub unsafe extern "system" fn dev_power_off(this: *mut c_void) {
    with_feedback(this as usize, |f| f.power_off_calls += 1);
    log("PowerOff", &format!("device {this:p} (ignored for a virtual device)"));
}

pub unsafe fn install_pad_feedback(vtbl: *mut usize) {
    if vtbl.is_null() {
        return;
    }
    *vtbl.add(6) = dev_create_force_feedback_effect as *const () as usize;
    *vtbl.add(7) = dev_is_force_feedback_motor_powered_on as *const () as usize;
    *vtbl.add(8) = dev_set_force_feedback_motor_gain as *const () as usize;
    *vtbl.add(9) = dev_set_haptic_motor_state as *const () as usize;
    *vtbl.add(10) = dev_set_rumble_state as *const () as usize;
    *vtbl.add(11) = dev_set_input_synchronization_state as *const () as usize;
    *vtbl.add(12) = dev_send_input_synchronization_hint as *const () as usize;
    *vtbl.add(13) = dev_power_off as *const () as usize;
}

#[repr(C)]
struct ExtDevice {
    vtbl: *const usize,
    kind: u32,
}

#[repr(C)]
struct ExtReading {
    vtbl: *const usize,
    refs: AtomicU32,
    kind: u32,
    device: usize,
    ts: u64,
    seq: u64,
    nkeys: u32,
    keys: [KeyState; MAX_KEYS],
    mouse: MouseState,
}

struct Objects {
    kb: usize,
    mouse: usize,
    reading_vtbl: usize,
    kb_info: usize,
    mouse_info: usize,
}

static OBJECTS: OnceLock<Objects> = OnceLock::new();
static STATE: Mutex<KbmState> = Mutex::new(KbmState { keys: Vec::new(), mouse: MouseState { buttons: 0, reserved: 0, position_x: 0, position_y: 0, wheel_x: 0, wheel_y: 0 } });
static KB_RING: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static MOUSE_RING: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static KB_SEQ: AtomicU64 = AtomicU64::new(1);
static MOUSE_SEQ: AtomicU64 = AtomicU64::new(1);
static KB_CONNECTED: AtomicBool = AtomicBool::new(false);
static MOUSE_CONNECTED: AtomicBool = AtomicBool::new(false);
static POLLS: AtomicU64 = AtomicU64::new(0);
static GI_THIS: AtomicUsize = AtomicUsize::new(0);
static NEXT: [AtomicUsize; 24] = [const { AtomicUsize::new(0) }; 24];
static ORIG_CREATE: AtomicUsize = AtomicUsize::new(0);
static DISABLED: AtomicBool = AtomicBool::new(false);
static ATTACHED_VTBL: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceRegistration {
    pub token: u64,
    pub ctx: usize,
    pub cb: usize,
    pub kind: u32,
    pub filter: u32,
}

pub fn registration_wants(r: &DeviceRegistration, device_kind: u32, current: u32, previous: u32) -> bool {
    r.cb != 0 && r.kind & device_kind != 0 && r.filter & (current ^ previous) != 0
}

pub fn regs_add(list: &mut Vec<DeviceRegistration>, r: DeviceRegistration) {
    if r.token != 0 {
        list.retain(|x| x.token != r.token);
    }
    list.push(r);
    if list.len() > 64 {
        list.remove(0);
    }
}

pub fn regs_remove(list: &mut Vec<DeviceRegistration>, token: u64) -> bool {
    let n = list.len();
    if token != 0 {
        list.retain(|x| x.token != token);
    }
    list.len() != n
}

static REGS: Mutex<Vec<DeviceRegistration>> = Mutex::new(Vec::new());

type DeviceCb = unsafe extern "system" fn(u64, *mut c_void, *mut c_void, u64, u32, u32);

fn objects() -> &'static Objects {
    OBJECTS.get_or_init(|| unsafe { build_objects(default_os_build()) })
}

fn leak_vtbl(slots: usize, fill: usize) -> &'static mut [usize] {
    Box::leak(vec![fill; slots].into_boxed_slice())
}

unsafe fn build_device_vtbl(os_build: u32) -> usize {
    let vt = leak_vtbl(32, ret_zero as *const () as usize);
    vt[0] = ext_qi as *const () as usize;
    vt[1] = static_addref as *const () as usize;
    vt[2] = static_release as *const () as usize;
    vt[3] = ext_dev_info as *const () as usize;
    vt[4] = ext_dev_status as *const () as usize;
    vt[5] = ext_dev_battery as *const () as usize;
    install_pad_feedback(vt.as_mut_ptr());
    vt[14] = raw_create_report as *const () as usize;
    vt[15] = raw_get_feature as *const () as usize;
    vt[16] = raw_notimpl as *const () as usize;
    vt[17] = raw_notimpl as *const () as usize;
    if device_method_count(os_build) == 22 {
        vt[18] = raw_send_with_response as *const () as usize;
        vt[19] = raw_ioctl as *const () as usize;
        vt[20] = raw_acquire as *const () as usize;
        vt[21] = ret_void as *const () as usize;
    } else {
        vt[18] = raw_ioctl as *const () as usize;
        vt[19] = raw_acquire as *const () as usize;
        vt[20] = ret_void as *const () as usize;
    }
    vt.as_ptr() as usize
}

unsafe fn build_reading_vtbl() -> usize {
    let vt = leak_vtbl(32, ret_zero as *const () as usize);
    vt[0] = rd_qi as *const () as usize;
    vt[1] = rd_addref as *const () as usize;
    vt[2] = rd_release as *const () as usize;
    vt[3] = rd_kind as *const () as usize;
    vt[4] = rd_seq as *const () as usize;
    vt[5] = rd_ts as *const () as usize;
    vt[6] = rd_device as *const () as usize;
    vt[7] = rd_raw as *const () as usize;
    vt[9] = rd_count_out as *const () as usize;
    vt[11] = rd_count_out as *const () as usize;
    vt[13] = rd_count_out as *const () as usize;
    vt[14] = rd_key_count as *const () as usize;
    vt[15] = rd_key_state as *const () as usize;
    vt[16] = rd_mouse_state as *const () as usize;
    vt[18] = rd_count_out as *const () as usize;
    vt[19] = rd_motion as *const () as usize;
    vt[20] = rd_arcade as *const () as usize;
    vt[21] = rd_flight as *const () as usize;
    vt[22] = rd_gamepad as *const () as usize;
    vt[23] = rd_wheel as *const () as usize;
    vt[24] = rd_uinav as *const () as usize;
    vt.as_ptr() as usize
}

unsafe fn build_objects(os_build: u32) -> Objects {
    let dev_vt = build_device_vtbl(os_build);
    let lang: &'static [u8] = b"en-US\0";
    let cps: &'static [u32] = &[b'e' as u32, b'n' as u32, b'-' as u32, b'U' as u32, b'S' as u32];
    let lang_s: &'static GiString = Box::leak(Box::new(GiString { size_in_bytes: 5, code_point_count: 5, data: lang.as_ptr(), code_points: cps.as_ptr() }));
    let mut ki = keyboard_info();
    ki.native_language = lang_s;
    let ki: &'static KeyboardInfo = Box::leak(Box::new(ki));
    let mi: &'static MouseInfo = Box::leak(Box::new(mouse_info()));
    let name_kb: &'static GiString = Box::leak(Box::new(make_string(b"TomBox Virtual Keyboard")));
    let name_ms: &'static GiString = Box::leak(Box::new(make_string(b"TomBox Virtual Mouse")));
    let kb_info = Box::leak(Box::new(InfoBuf(device_info_bytes(KIND_KEYBOARD))));
    kb_info.0[INFO_KEYBOARD_INFO..INFO_KEYBOARD_INFO + 8].copy_from_slice(&(ki as *const KeyboardInfo as u64).to_le_bytes());
    kb_info.0[INFO_DISPLAY_NAME..INFO_DISPLAY_NAME + 8].copy_from_slice(&(name_kb as *const GiString as u64).to_le_bytes());
    let ms_info = Box::leak(Box::new(InfoBuf(device_info_bytes(KIND_MOUSE))));
    ms_info.0[INFO_MOUSE_INFO..INFO_MOUSE_INFO + 8].copy_from_slice(&(mi as *const MouseInfo as u64).to_le_bytes());
    ms_info.0[INFO_DISPLAY_NAME..INFO_DISPLAY_NAME + 8].copy_from_slice(&(name_ms as *const GiString as u64).to_le_bytes());
    let kb = Box::leak(Box::new(ExtDevice { vtbl: dev_vt as *const usize, kind: KIND_KEYBOARD }));
    let mouse = Box::leak(Box::new(ExtDevice { vtbl: dev_vt as *const usize, kind: KIND_MOUSE }));
    Objects {
        kb: kb as *mut ExtDevice as usize,
        mouse: mouse as *mut ExtDevice as usize,
        reading_vtbl: build_reading_vtbl(),
        kb_info: kb_info.0.as_ptr() as usize,
        mouse_info: ms_info.0.as_ptr() as usize,
    }
}

fn make_string(s: &'static [u8]) -> GiString {
    let cps: Vec<u32> = s.iter().map(|c| *c as u32).collect();
    let cps: &'static [u32] = Box::leak(cps.into_boxed_slice());
    let mut d = s.to_vec();
    d.push(0);
    let d: &'static [u8] = Box::leak(d.into_boxed_slice());
    GiString { size_in_bytes: s.len() as u32, code_point_count: s.len() as u32, data: d.as_ptr(), code_points: cps.as_ptr() }
}

#[repr(C, align(8))]
struct InfoBuf([u8; DEVICE_INFO_SIZE]);

pub fn keyboard_device() -> *mut c_void {
    objects().kb as *mut c_void
}

pub fn mouse_device() -> *mut c_void {
    objects().mouse as *mut c_void
}

pub fn is_ext_device(p: *mut c_void) -> bool {
    !p.is_null() && (p as usize == objects().kb || p as usize == objects().mouse)
}

fn device_kind(p: *mut c_void) -> u32 {
    if p as usize == objects().kb { KIND_KEYBOARD } else { KIND_MOUSE }
}

pub fn is_ext_reading(p: *mut c_void) -> bool {
    !p.is_null() && unsafe { *(p as *const usize) == objects().reading_vtbl }
}

pub fn is_connected(kind: u32) -> bool {
    match kind {
        KIND_KEYBOARD => KB_CONNECTED.load(Ordering::SeqCst),
        KIND_MOUSE => MOUSE_CONNECTED.load(Ordering::SeqCst),
        _ => false,
    }
}

fn status_of(kind: u32) -> u32 {
    if is_connected(kind) { STATUS_CONNECTED | STATUS_INPUT_ENABLED } else { 0 }
}

pub fn classify(device: *mut c_void) -> Target {
    if device.is_null() {
        return Target::AnyDevice;
    }
    if let Some(o) = OBJECTS.get() {
        if device as usize == o.kb {
            return Target::VirtualKeyboard;
        }
        if device as usize == o.mouse {
            return Target::VirtualMouse;
        }
    }
    unsafe {
        let vt = *(device as *const *const usize);
        if vt.is_null() {
            return Target::HostDevice;
        }
        let f: unsafe extern "system" fn(*mut c_void) -> *const u8 = core::mem::transmute(*vt.add(3));
        let info = f(device);
        if !info.is_null() && core::slice::from_raw_parts(info.add(INFO_DEVICE_ID), 32) == crate::gamepad::DEVICE_ID {
            return Target::VirtualPad;
        }
    }
    Target::HostDevice
}

pub fn now_us() -> u64 {
    let gi = GI_THIS.load(Ordering::SeqCst);
    let f = NEXT[3].load(Ordering::SeqCst);
    if gi != 0 && f != 0 {
        let g: unsafe extern "system" fn(usize) -> u64 = unsafe { core::mem::transmute(f) };
        return unsafe { g(gi) };
    }
    let (mut c, mut q) = (0i64, 1i64);
    unsafe {
        QueryPerformanceCounter(&mut c);
        QueryPerformanceFrequency(&mut q);
    }
    ((c as i128 * 1_000_000) / q.max(1) as i128) as u64
}

fn ring(kind: u32) -> &'static Mutex<Vec<usize>> {
    if kind == KIND_KEYBOARD { &KB_RING } else { &MOUSE_RING }
}

fn seq_of(kind: u32) -> &'static AtomicU64 {
    if kind == KIND_KEYBOARD { &KB_SEQ } else { &MOUSE_SEQ }
}

fn snapshot(kind: u32) -> *mut ExtReading {
    let o = objects();
    let s = STATE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut keys = [KeyState::default(); MAX_KEYS];
    let ks = s.key_states();
    keys[..ks.len()].copy_from_slice(&ks);
    let r = Box::new(ExtReading {
        vtbl: o.reading_vtbl as *const usize,
        refs: AtomicU32::new(1),
        kind,
        device: if kind == KIND_KEYBOARD { o.kb } else { o.mouse },
        ts: now_us(),
        seq: seq_of(kind).load(Ordering::SeqCst),
        nkeys: if kind == KIND_KEYBOARD { ks.len() as u32 } else { 0 },
        keys,
        mouse: if kind == KIND_MOUSE { s.mouse } else { MouseState::default() },
    });
    Box::into_raw(r)
}

fn push_snapshot(kind: u32) {
    let r = snapshot(kind) as usize;
    let mut g = ring(kind).lock().unwrap_or_else(|e| e.into_inner());
    g.push(r);
    if g.len() > HISTORY {
        let old = g.remove(0);
        unsafe { rd_release(old as *mut c_void) };
    }
}

fn changed(kind: u32) {
    seq_of(kind).fetch_add(1, Ordering::SeqCst);
    push_snapshot(kind);
}

fn current(kind: u32) -> *mut c_void {
    let mut g = ring(kind).lock().unwrap_or_else(|e| e.into_inner());
    let seq = seq_of(kind).load(Ordering::SeqCst);
    let stale = g.last().map(|p| unsafe { (*(*p as *const ExtReading)).seq != seq }).unwrap_or(true);
    if stale {
        g.push(snapshot(kind) as usize);
        if g.len() > HISTORY {
            let old = g.remove(0);
            unsafe { rd_release(old as *mut c_void) };
        }
    }
    let p = *g.last().unwrap() as *mut c_void;
    unsafe { rd_addref(p) };
    p
}

fn neighbour(reference: *mut c_void, newer: bool) -> *mut c_void {
    let (kind, rseq) = unsafe {
        let r = &*(reference as *const ExtReading);
        (r.kind, r.seq)
    };
    let g = ring(kind).lock().unwrap_or_else(|e| e.into_inner());
    let found = if newer {
        g.iter().copied().find(|p| unsafe { (*(*p as *const ExtReading)).seq > rseq })
    } else {
        g.iter().rev().copied().find(|p| unsafe { (*(*p as *const ExtReading)).seq < rseq })
    };
    match found {
        Some(p) => {
            unsafe { rd_addref(p as *mut c_void) };
            p as *mut c_void
        }
        None => core::ptr::null_mut(),
    }
}

pub fn update_state(f: impl FnOnce(&mut KbmState) -> (bool, bool)) -> (bool, bool) {
    let (k, m) = {
        let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    };
    if k {
        changed(KIND_KEYBOARD);
    }
    if m {
        changed(KIND_MOUSE);
    }
    (k, m)
}

pub fn state() -> KbmState {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn key_event(k: KeyDef, down: bool) -> bool {
    update_state(|s| (s.key(k, down), false)).0
}

pub fn mouse_button(b: u32, down: bool) -> bool {
    update_state(|s| (false, s.button(b, down))).1
}

pub fn mouse_move(dx: i64, dy: i64) -> bool {
    update_state(|s| (false, s.move_by(dx, dy))).1
}

pub fn mouse_wheel(dx: i64, dy: i64) -> bool {
    update_state(|s| (false, s.wheel(dx, dy))).1
}

pub fn set_connected(kinds: u32, on: bool) {
    for kind in [KIND_KEYBOARD, KIND_MOUSE] {
        if kinds & kind == 0 {
            continue;
        }
        let flag = if kind == KIND_KEYBOARD { &KB_CONNECTED } else { &MOUSE_CONNECTED };
        if flag.swap(on, Ordering::SeqCst) == on {
            continue;
        }
        if !on {
            update_state(|s| {
                if kind == KIND_KEYBOARD {
                    let k = !s.keys.is_empty();
                    s.keys.clear();
                    (k, false)
                } else {
                    let m = s.mouse.buttons != 0;
                    s.mouse.buttons = 0;
                    (false, m)
                }
            });
        }
        let (cur, prev) = if on { (STATUS_CONNECTED | STATUS_INPUT_ENABLED, 0) } else { (0, STATUS_CONNECTED | STATUS_INPUT_ENABLED) };
        let regs: Vec<DeviceRegistration> = REGS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let dev = if kind == KIND_KEYBOARD { keyboard_device() } else { mouse_device() };
        let mut n = 0;
        for r in regs.iter().filter(|r| registration_wants(r, kind, cur, prev)) {
            notify(r, dev, cur, prev);
            n += 1;
        }
        log(if on { "connect" } else { "disconnect" }, &format!("virtual {} {} -> notified {n} device callback registration(s)",
            if kind == KIND_KEYBOARD { "keyboard" } else { "mouse" }, if on { "plugged in" } else { "unplugged" }));
    }
}

fn notify(r: &DeviceRegistration, dev: *mut c_void, cur: u32, prev: u32) {
    let f: DeviceCb = unsafe { core::mem::transmute(r.cb) };
    unsafe { f(r.token, r.ctx as *mut c_void, dev, now_us(), cur, prev) };
}

pub fn registrations() -> Vec<DeviceRegistration> {
    REGS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

unsafe extern "system" fn ret_zero(_this: *mut c_void) -> u64 {
    0
}
unsafe extern "system" fn ret_void(_this: *mut c_void) {}
unsafe extern "system" fn static_addref(_this: *mut c_void) -> u32 {
    2
}
unsafe extern "system" fn static_release(_this: *mut c_void) -> u32 {
    1
}
unsafe extern "system" fn ext_qi(this: *mut c_void, _iid: *const u8, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    *out = this;
    S_OK
}
unsafe extern "system" fn ext_dev_info(this: *mut c_void) -> *const u8 {
    let o = objects();
    (if this as usize == o.kb { o.kb_info } else { o.mouse_info }) as *const u8
}
unsafe extern "system" fn ext_dev_status(this: *mut c_void) -> u32 {
    status_of((*(this as *const ExtDevice)).kind)
}
unsafe extern "system" fn ext_dev_battery(_this: *mut c_void, out: *mut u8) {
    if !out.is_null() {
        core::ptr::write_bytes(out, 0, 16);
        core::ptr::write_unaligned(out.add(16) as *mut i32, -1);
    }
}
unsafe extern "system" fn raw_create_report(_this: *mut c_void, _id: u32, _kind: u32, out: *mut *mut c_void) -> i32 {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    E_NOTIMPL
}
unsafe extern "system" fn raw_get_feature(_this: *mut c_void, _id: u32, out: *mut *mut c_void) -> i32 {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    E_NOTIMPL
}
unsafe extern "system" fn raw_notimpl(_this: *mut c_void, _report: *mut c_void) -> i32 {
    E_NOTIMPL
}
unsafe extern "system" fn raw_send_with_response(_this: *mut c_void, _report: *mut c_void, out: *mut *mut c_void) -> i32 {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    E_NOTIMPL
}
unsafe extern "system" fn raw_ioctl(_this: *mut c_void, _code: u32, _in_size: usize, _input: *const c_void, _out_size: usize, _output: *mut c_void, written: *mut usize) -> i32 {
    if !written.is_null() {
        *written = 0;
    }
    E_NOTIMPL
}
unsafe extern "system" fn raw_acquire(_this: *mut c_void, _timeout: u64) -> bool {
    false
}

unsafe extern "system" fn rd_qi(this: *mut c_void, _iid: *const u8, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    rd_addref(this);
    *out = this;
    S_OK
}
unsafe extern "system" fn rd_addref(this: *mut c_void) -> u32 {
    (*(this as *mut ExtReading)).refs.fetch_add(1, Ordering::SeqCst) + 1
}
unsafe extern "system" fn rd_release(this: *mut c_void) -> u32 {
    let r = this as *mut ExtReading;
    let n = (*r).refs.fetch_sub(1, Ordering::SeqCst) - 1;
    if n == 0 {
        drop(Box::from_raw(r));
    }
    n
}
unsafe extern "system" fn rd_kind(this: *mut c_void) -> u32 {
    (*(this as *mut ExtReading)).kind
}
unsafe extern "system" fn rd_seq(this: *mut c_void, kind: u32) -> u64 {
    let r = &*(this as *mut ExtReading);
    if kind & r.kind != 0 { r.seq } else { 0 }
}
unsafe extern "system" fn rd_ts(this: *mut c_void) -> u64 {
    (*(this as *mut ExtReading)).ts
}
unsafe extern "system" fn rd_device(this: *mut c_void, out: *mut *mut c_void) {
    if !out.is_null() {
        *out = (*(this as *mut ExtReading)).device as *mut c_void;
    }
}
unsafe extern "system" fn rd_raw(_this: *mut c_void, out: *mut *mut c_void) -> bool {
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    false
}
unsafe extern "system" fn rd_count_out(_this: *mut c_void, _count: u32, _out: *mut c_void) -> u32 {
    0
}
unsafe extern "system" fn rd_key_count(this: *mut c_void) -> u32 {
    (*(this as *mut ExtReading)).nkeys
}
unsafe extern "system" fn rd_key_state(this: *mut c_void, count: u32, out: *mut KeyState) -> u32 {
    let r = &*(this as *mut ExtReading);
    fill_key_states(&r.keys[..r.nkeys as usize], count, out)
}
unsafe extern "system" fn rd_mouse_state(this: *mut c_void, out: *mut MouseState) -> bool {
    if out.is_null() {
        return false;
    }
    let r = &*(this as *mut ExtReading);
    if r.kind != KIND_MOUSE {
        core::ptr::write_unaligned(out, MouseState::default());
        return false;
    }
    core::ptr::write_unaligned(out, r.mouse);
    true
}
unsafe fn zero_false(out: *mut u8, n: usize) -> bool {
    if !out.is_null() {
        core::ptr::write_bytes(out, 0, n);
    }
    false
}
unsafe extern "system" fn rd_motion(_this: *mut c_void, out: *mut u8) -> bool {
    zero_false(out, MOTION_STATE_SIZE)
}
unsafe extern "system" fn rd_arcade(_this: *mut c_void, out: *mut u8) -> bool {
    zero_false(out, ARCADE_STICK_STATE_SIZE)
}
unsafe extern "system" fn rd_flight(_this: *mut c_void, out: *mut u8) -> bool {
    zero_false(out, FLIGHT_STICK_STATE_SIZE)
}
unsafe extern "system" fn rd_gamepad(_this: *mut c_void, out: *mut u8) -> bool {
    zero_false(out, GAMEPAD_STATE_SIZE)
}
unsafe extern "system" fn rd_wheel(_this: *mut c_void, out: *mut u8) -> bool {
    zero_false(out, RACING_WHEEL_STATE_SIZE)
}
unsafe extern "system" fn rd_uinav(_this: *mut c_void, out: *mut u8) -> bool {
    zero_false(out, UI_NAVIGATION_STATE_SIZE)
}

fn enabled() -> bool {
    !DISABLED.load(Ordering::SeqCst)
}

pub unsafe fn on_get_current_reading(kind: u32, device: *mut c_void, out: *mut *mut c_void) -> Option<i32> {
    note_kind(Api::GetCurrentReading, kind, 0, classify(device));
    if !enabled() {
        return None;
    }
    let target = if is_ext_device(device) {
        device_kind(device)
    } else if device.is_null() && kind & KIND_KBM != 0 && kind & KIND_GAMEPAD == 0 {
        if kind & KIND_KEYBOARD != 0 && is_connected(KIND_KEYBOARD) {
            KIND_KEYBOARD
        } else if kind & KIND_MOUSE != 0 && is_connected(KIND_MOUSE) {
            KIND_MOUSE
        } else {
            return None;
        }
    } else {
        return None;
    };
    if out.is_null() {
        return Some(E_POINTER);
    }
    if !is_connected(target) {
        *out = core::ptr::null_mut();
        return Some(DEVICE_DISCONNECTED);
    }
    if kind & target == 0 {
        *out = core::ptr::null_mut();
        return Some(READING_NOT_FOUND);
    }
    POLLS.fetch_add(1, Ordering::SeqCst);
    *out = current(target);
    Some(S_OK)
}

pub unsafe fn on_get_neighbour_reading(newer: bool, reference: *mut c_void, kind: u32, device: *mut c_void, out: *mut *mut c_void) -> Option<i32> {
    note_kind(if newer { Api::GetNextReading } else { Api::GetPreviousReading }, kind, 0, classify(device));
    if !enabled() || !(is_ext_reading(reference) || is_ext_device(device)) {
        return None;
    }
    if out.is_null() {
        return Some(E_POINTER);
    }
    *out = core::ptr::null_mut();
    if !is_ext_reading(reference) {
        return Some(READING_NOT_FOUND);
    }
    let rk = (*(reference as *const ExtReading)).kind;
    if kind & rk == 0 || (is_ext_device(device) && device_kind(device) != rk) {
        return Some(READING_NOT_FOUND);
    }
    let p = neighbour(reference, newer);
    if p.is_null() {
        return Some(READING_NOT_FOUND);
    }
    *out = p;
    Some(S_OK)
}

pub unsafe fn on_get_temporal_reading(device: *mut c_void, out: *mut *mut c_void) -> Option<i32> {
    note_kind(Api::GetTemporalReading, 0, 0, classify(device));
    if !enabled() || !is_ext_device(device) {
        return None;
    }
    if !out.is_null() {
        *out = core::ptr::null_mut();
    }
    Some(E_NOTIMPL)
}

pub unsafe fn on_register_reading_callback(device: *mut c_void, kind: u32, token: *mut u64) -> Option<i32> {
    note_kind(Api::RegisterReadingCallback, kind, 0, classify(device));
    if !enabled() || !is_ext_device(device) {
        return None;
    }
    if !token.is_null() {
        *token = 0;
    }
    Some(E_NOTIMPL)
}

pub fn device_callback_pre(device: *mut c_void, kind: u32, filter: u32) -> Option<i32> {
    note_kind(Api::RegisterDeviceCallback, kind, filter, classify(device));
    if enabled() && is_ext_device(device) { Some(S_OK) } else { None }
}

pub unsafe fn device_callback_post(hr: i32, device: *mut c_void, kind: u32, filter: u32, enum_kind: u32, ctx: *mut c_void, cb: usize, token: *mut u64) {
    if hr < 0 || !enabled() || cb == 0 || kind & KIND_KBM == 0 || !(device.is_null() || is_ext_device(device)) {
        return;
    }
    let mask = if is_ext_device(device) { kind & device_kind(device) } else { kind & KIND_KBM };
    if mask == 0 {
        return;
    }
    let r = DeviceRegistration { token: if token.is_null() { 0 } else { *token }, ctx: ctx as usize, cb, kind: mask, filter };
    let n = {
        let mut g = REGS.lock().unwrap_or_else(|e| e.into_inner());
        regs_add(&mut g, r);
        g.len()
    };
    let mut announced = Vec::new();
    if enum_kind != 0 {
        for k in [KIND_KEYBOARD, KIND_MOUSE] {
            if registration_wants(&r, k, status_of(k), 0) && is_connected(k) {
                notify(&r, if k == KIND_KEYBOARD { keyboard_device() } else { mouse_device() }, status_of(k), 0);
                announced.push(if k == KIND_KEYBOARD { "keyboard" } else { "mouse" });
            }
        }
    }
    log("RegisterDeviceCallback", &format!("kind={kind:#x} filter={filter:#x} enum={enum_kind} token={:#x} -> keyboard/mouse listener recorded ({n} total); announced now: [{}]",
        r.token, announced.join(",")));
}

pub unsafe fn on_unregister(token: u64) {
    if regs_remove(&mut REGS.lock().unwrap_or_else(|e| e.into_inner()), token) {
        log("UnregisterCallback", &format!("token={token:#x} keyboard/mouse listener removed"));
    }
}

pub unsafe fn on_find_device_from_id(id: *const [u8; 32], out: *mut *mut c_void) -> Option<i32> {
    if !enabled() || id.is_null() || out.is_null() {
        return None;
    }
    if *id == KEYBOARD_ID {
        *out = keyboard_device();
        return Some(S_OK);
    }
    if *id == MOUSE_ID {
        *out = mouse_device();
        return Some(S_OK);
    }
    None
}

pub unsafe fn on_find_device_from_object(obj: *mut c_void, out: *mut *mut c_void) -> Option<i32> {
    if !enabled() || out.is_null() || !is_ext_device(obj) {
        return None;
    }
    *out = obj;
    Some(S_OK)
}

fn next(i: usize) -> usize {
    NEXT[i].load(Ordering::SeqCst)
}

unsafe extern "system" fn gi_current(this: *mut c_void, kind: u32, device: *mut c_void, out: *mut *mut c_void) -> i32 {
    if let Some(hr) = on_get_current_reading(kind, device, out) {
        return hr;
    }
    let f: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut *mut c_void) -> i32 = core::mem::transmute(next(4));
    f(this, kind, device, out)
}

unsafe extern "system" fn gi_next(this: *mut c_void, reference: *mut c_void, kind: u32, device: *mut c_void, out: *mut *mut c_void) -> i32 {
    if let Some(hr) = on_get_neighbour_reading(true, reference, kind, device, out) {
        return hr;
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut c_void, *mut *mut c_void) -> i32 = core::mem::transmute(next(5));
    f(this, reference, kind, device, out)
}

unsafe extern "system" fn gi_prev(this: *mut c_void, reference: *mut c_void, kind: u32, device: *mut c_void, out: *mut *mut c_void) -> i32 {
    if let Some(hr) = on_get_neighbour_reading(false, reference, kind, device, out) {
        return hr;
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut c_void, *mut *mut c_void) -> i32 = core::mem::transmute(next(6));
    f(this, reference, kind, device, out)
}

unsafe extern "system" fn gi_temporal(this: *mut c_void, ts: u64, device: *mut c_void, out: *mut *mut c_void) -> i32 {
    if let Some(hr) = on_get_temporal_reading(device, out) {
        return hr;
    }
    let f: unsafe extern "system" fn(*mut c_void, u64, *mut c_void, *mut *mut c_void) -> i32 = core::mem::transmute(next(7));
    f(this, ts, device, out)
}

unsafe extern "system" fn gi_reading_cb(this: *mut c_void, device: *mut c_void, kind: u32, threshold: f32, ctx: *mut c_void, cb: usize, token: *mut u64) -> i32 {
    if let Some(hr) = on_register_reading_callback(device, kind, token) {
        return hr;
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, f32, *mut c_void, usize, *mut u64) -> i32 = core::mem::transmute(next(8));
    f(this, device, kind, threshold, ctx, cb, token)
}

pub const SYNTHETIC_TOKEN_BASE: u64 = 0x7B0B_7B0B_0000_0000;
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

pub fn is_synthetic_token(token: u64) -> bool {
    token & 0xFFFF_FFFF_0000_0000 == SYNTHETIC_TOKEN_BASE
}

pub fn synthetic_token() -> u64 {
    SYNTHETIC_TOKEN_BASE | (NEXT_TOKEN.fetch_add(1, Ordering::SeqCst) & 0xFFFF_FFFF)
}

unsafe extern "system" fn gi_device_cb(this: *mut c_void, device: *mut c_void, kind: u32, filter: u32, enum_kind: u32, ctx: *mut c_void, cb: usize, token: *mut u64) -> i32 {
    let hr = match device_callback_pre(device, kind, filter) {
        Some(hr) => {
            if !token.is_null() {
                *token = synthetic_token();
            }
            hr
        }
        None => {
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32, u32, *mut c_void, usize, *mut u64) -> i32 = core::mem::transmute(next(9));
            f(this, device, kind, filter, enum_kind, ctx, cb, token)
        }
    };
    device_callback_post(hr, device, kind, filter, enum_kind, ctx, cb, token);
    hr
}

unsafe extern "system" fn gi_stop(this: *mut c_void, token: u64) {
    on_unregister(token);
    if is_synthetic_token(token) {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, u64) = core::mem::transmute(next(12));
    f(this, token)
}

unsafe extern "system" fn gi_unregister(this: *mut c_void, token: u64, timeout: u64) -> u32 {
    on_unregister(token);
    if is_synthetic_token(token) {
        return 1;
    }
    let f: unsafe extern "system" fn(*mut c_void, u64, u64) -> u32 = core::mem::transmute(next(13));
    f(this, token, timeout)
}

unsafe extern "system" fn gi_find_id(this: *mut c_void, id: *const [u8; 32], out: *mut *mut c_void) -> i32 {
    if let Some(hr) = on_find_device_from_id(id, out) {
        return hr;
    }
    let f: unsafe extern "system" fn(*mut c_void, *const [u8; 32], *mut *mut c_void) -> i32 = core::mem::transmute(next(16));
    f(this, id, out)
}

unsafe extern "system" fn gi_find_obj(this: *mut c_void, obj: *mut c_void, out: *mut *mut c_void) -> i32 {
    if let Some(hr) = on_find_device_from_object(obj, out) {
        return hr;
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> i32 = core::mem::transmute(next(17));
    f(this, obj, out)
}

pub const PATCHED_SLOTS: [usize; 10] = [4, 5, 6, 7, 8, 9, 12, 13, 16, 17];

fn layer_fn(i: usize) -> usize {
    match i {
        4 => gi_current as *const () as usize,
        5 => gi_next as *const () as usize,
        6 => gi_prev as *const () as usize,
        7 => gi_temporal as *const () as usize,
        8 => gi_reading_cb as *const () as usize,
        9 => gi_device_cb as *const () as usize,
        12 => gi_stop as *const () as usize,
        13 => gi_unregister as *const () as usize,
        16 => gi_find_id as *const () as usize,
        _ => gi_find_obj as *const () as usize,
    }
}

pub unsafe fn attach(obj: *mut c_void) -> bool {
    if obj.is_null() {
        return false;
    }
    let vt = *(obj as *const *mut usize);
    if vt.is_null() {
        return false;
    }
    objects();
    if *vt.add(4) == gi_current as *const () as usize {
        GI_THIS.store(obj as usize, Ordering::SeqCst);
        return true;
    }
    let prev = ATTACHED_VTBL.load(Ordering::SeqCst);
    if prev != 0 && prev != vt as usize {
        log("attach", &format!("IGameInput {obj:p} has vtable {vt:p}, not the attached {prev:#x}; left unchanged"));
        return false;
    }
    ATTACHED_VTBL.store(vt as usize, Ordering::SeqCst);
    GI_THIS.store(obj as usize, Ordering::SeqCst);
    let mut old = 0u32;
    if VirtualProtect(vt as *mut c_void, 22 * 8, 0x04, &mut old) == 0 {
        log("attach", &format!("IGameInput {obj:p}: VirtualProtect on vtable {vt:p} failed; keyboard/mouse layer not attached"));
        return false;
    }
    NEXT[3].store(*vt.add(3), Ordering::SeqCst);
    for i in PATCHED_SLOTS {
        NEXT[i].store(*vt.add(i), Ordering::SeqCst);
    }
    for i in PATCHED_SLOTS {
        core::ptr::write_volatile(vt.add(i), layer_fn(i));
    }
    let mut tmp = 0u32;
    VirtualProtect(vt as *mut c_void, 22 * 8, old, &mut tmp);
    if std::env::var("TOMBOX_KBM_BOOT").ok().as_deref() == Some("1") {
        set_connected(KIND_KBM, true);
    }
    start_channel();
    log("attach", &format!("IGameInput {obj:p} vtable {vt:p}: keyboard/mouse + kind-mask layer attached (os build layout {}, device methods {}); control dir {}",
        default_os_build(), device_method_count(default_os_build()), channel_dir().display()));
    true
}

unsafe extern "system" fn create_hook(out: *mut *mut c_void) -> i32 {
    let f: unsafe extern "system" fn(*mut *mut c_void) -> i32 = core::mem::transmute(ORIG_CREATE.load(Ordering::SeqCst));
    let hr = f(out);
    if hr >= 0 && !out.is_null() && !(*out).is_null() {
        attach(*out);
    }
    hr
}

pub fn ensure() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe { install() });
}

unsafe fn install() {
    if std::env::var("TOMBOX_GIEXT").ok().as_deref() == Some("0") {
        return;
    }
    if std::env::var("TOMBOX_KBM").ok().as_deref() == Some("0") {
        DISABLED.store(true, Ordering::SeqCst);
    }
    let marker = format!("{}", GetCurrentProcessId());
    if std::env::var("TOMBOX_GIEXT_OWNER").ok().as_deref() == Some(marker.as_str()) {
        return;
    }
    std::env::set_var("TOMBOX_GIEXT_OWNER", &marker);
    let base = GetModuleHandleW(core::ptr::null());
    if base.is_null() {
        return;
    }
    let Some(slot) = crate::gamepad::find_import_slot(base, b"gameinput.dll", 1, b"GameInputCreate") else {
        log("install", "host EXE does not import GameInput.dll #1 (GameInputCreate) - keyboard/mouse layer not installed");
        return;
    };
    let mut old = 0u32;
    if VirtualProtect(slot as *mut c_void, 8, 0x04, &mut old) == 0 {
        log("install", "VirtualProtect on the GameInput import slot failed - keyboard/mouse layer not installed");
        return;
    }
    ORIG_CREATE.store(core::ptr::read_volatile(slot), Ordering::SeqCst);
    core::ptr::write_volatile(slot, create_hook as *const () as usize);
    let mut tmp = 0u32;
    VirtualProtect(slot as *mut c_void, 8, old, &mut tmp);
    log("install", &format!("GameInputCreate import (slot EXE+{:#x}) chained; IGameInput objects get the keyboard/mouse + kind-mask layer", slot as usize - base as usize));
}

pub fn channel_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("TOMBOX_KBM_DIR") {
        return std::path::PathBuf::from(p);
    }
    let mut d = std::env::temp_dir();
    d.push("tombox");
    d.push("kbm");
    d
}

fn start_channel() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new().name("tombox-kbm".into()).spawn(channel_loop);
    });
}

fn hold_for(ms: u64) {
    let p = POLLS.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(ms));
    let t0 = Instant::now();
    while POLLS.load(Ordering::SeqCst) < p + 2 && t0.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn apply_command(cmd: &KbmCmd) {
    match cmd {
        KbmCmd::KeyTap(keys, ms) => {
            update_state(|s| (keys.iter().fold(false, |a, k| s.key(*k, true) | a), false));
            hold_for(*ms);
            update_state(|s| (keys.iter().fold(false, |a, k| s.key(*k, false) | a), false));
            hold_for(GAP_MS);
        }
        KbmCmd::KeyDown(keys) => {
            update_state(|s| (keys.iter().fold(false, |a, k| s.key(*k, true) | a), false));
        }
        KbmCmd::KeyUp(keys) => {
            update_state(|s| (keys.iter().fold(false, |a, k| s.key(*k, false) | a), false));
        }
        KbmCmd::Move(dx, dy) => {
            mouse_move(*dx, *dy);
        }
        KbmCmd::Click(b, ms) => {
            mouse_button(*b, true);
            hold_for(*ms);
            mouse_button(*b, false);
            hold_for(GAP_MS);
        }
        KbmCmd::ButtonDown(b) => {
            mouse_button(*b, true);
        }
        KbmCmd::ButtonUp(b) => {
            mouse_button(*b, false);
        }
        KbmCmd::Wheel(dx, dy) => {
            mouse_wheel(*dx, *dy);
        }
        KbmCmd::Wait(ms) => std::thread::sleep(Duration::from_millis(*ms)),
        KbmCmd::Release => {
            update_state(|s| s.release());
        }
        KbmCmd::Connect(k) => set_connected(*k, true),
        KbmCmd::Disconnect(k) => set_connected(*k, false),
    }
}

fn channel_loop() {
    let dir = channel_dir();
    let _ = std::fs::create_dir_all(&dir);
    log("channel", &format!("polling {} for *.cmd files", dir.display()));
    loop {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("cmd")).unwrap_or(false))
                .collect())
            .unwrap_or_default();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            let fname = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            for line in text.lines() {
                match parse_kbm_command(line) {
                    Ok(Some(cmd)) => {
                        log("cmd", &format!("{fname}: {}", line.trim()));
                        if !matches!(cmd, KbmCmd::Disconnect(_) | KbmCmd::Connect(_)) {
                            set_connected(KIND_KBM, true);
                        }
                        apply_command(&cmd);
                    }
                    Ok(None) => {}
                    Err(e) => log("cmd", &format!("{fname}: {e}")),
                }
            }
            let _ = std::fs::remove_file(&f);
            log("cmd", &format!("{fname}: done (keyboard/mouse readings served so far: {}); kinds requested so far: {}",
                POLLS.load(Ordering::SeqCst), kind_names(requested_kinds())));
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

pub fn pe_rva_to_offset(file: &[u8], rva: u32) -> Option<usize> {
    let rd32 = |o: usize| file.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let rd16 = |o: usize| file.get(o..o + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap()));
    let nt = rd32(0x3c)? as usize;
    let nsec = rd16(nt + 6)? as usize;
    let opt_size = rd16(nt + 20)? as usize;
    let sec = nt + 24 + opt_size;
    for i in 0..nsec {
        let s = sec + i * 40;
        let vsize = rd32(s + 8)?;
        let va = rd32(s + 12)?;
        let raw_size = rd32(s + 16)?;
        let raw = rd32(s + 20)?;
        if rva >= va && rva < va + vsize.max(raw_size) {
            return Some((rva - va + raw) as usize);
        }
    }
    None
}

fn pe_dir(file: &[u8], index: usize) -> Option<(u32, u32)> {
    let rd32 = |o: usize| file.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if file.get(0..2)? != b"MZ" {
        return None;
    }
    let nt = rd32(0x3c)? as usize;
    if rd32(nt)? != 0x4550 {
        return None;
    }
    let opt = nt + 24;
    let magic = u16::from_le_bytes(file.get(opt..opt + 2)?.try_into().ok()?);
    let dirs = opt + if magic == 0x20b { 112 } else { 96 };
    Some((rd32(dirs + index * 8)?, rd32(dirs + index * 8 + 4)?))
}

fn c_str(file: &[u8], off: usize) -> Option<String> {
    let s = file.get(off..)?;
    let end = s.iter().position(|c| *c == 0)?;
    Some(String::from_utf8_lossy(&s[..end]).into_owned())
}

pub fn pe_export_by_ordinal(file: &[u8], ordinal: u32) -> Option<(String, u32)> {
    let rd32 = |o: usize| file.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let rd16 = |o: usize| file.get(o..o + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap()));
    let (rva, _) = pe_dir(file, 0)?;
    if rva == 0 {
        return None;
    }
    let e = pe_rva_to_offset(file, rva)?;
    let base = rd32(e + 16)?;
    let nfun = rd32(e + 20)?;
    let nnames = rd32(e + 24)?;
    let funs = pe_rva_to_offset(file, rd32(e + 28)?)?;
    let names = pe_rva_to_offset(file, rd32(e + 32)?)?;
    let ords = pe_rva_to_offset(file, rd32(e + 36)?)?;
    let idx = ordinal.checked_sub(base)?;
    if idx >= nfun {
        return None;
    }
    let addr = rd32(funs + idx as usize * 4)?;
    for i in 0..nnames as usize {
        if rd16(ords + i * 2)? as u32 == idx {
            let name = c_str(file, pe_rva_to_offset(file, rd32(names + i * 4)?)?)?;
            return Some((name, addr));
        }
    }
    Some((String::new(), addr))
}

pub fn pe_import_ordinals(file: &[u8], dll: &str) -> Option<Vec<u16>> {
    let rd32 = |o: usize| file.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let rd64 = |o: usize| file.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
    let (rva, _) = pe_dir(file, 1)?;
    let mut d = pe_rva_to_offset(file, rva)?;
    loop {
        let oft = rd32(d)?;
        let name = rd32(d + 12)?;
        let ft = rd32(d + 16)?;
        if name == 0 && ft == 0 {
            return None;
        }
        if c_str(file, pe_rva_to_offset(file, name)?)?.eq_ignore_ascii_case(dll) {
            let mut t = pe_rva_to_offset(file, if oft != 0 { oft } else { ft })?;
            let mut out = Vec::new();
            loop {
                let e = rd64(t)?;
                if e == 0 {
                    break;
                }
                if e & (1 << 63) != 0 {
                    out.push((e & 0xffff) as u16);
                }
                t += 8;
            }
            return Some(out);
        }
        d += 20;
    }
}
