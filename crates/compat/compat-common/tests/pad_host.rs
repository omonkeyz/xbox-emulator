//! Keyboard-to-pad mapping end to end: a window of this process in the foreground, a key
//! injected with SendInput, and the reading the virtual pad would publish.
use compat_common::gamepad::{published, PadState, AX_LY};
use core::ffi::c_void;
use std::time::{Duration, Instant};

#[link(name = "user32")]
extern "system" {
    fn CreateWindowExW(ex: u32, cls: *const u16, name: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: *mut c_void, menu: *mut c_void, inst: *mut c_void, p: *mut c_void) -> *mut c_void;
    fn DestroyWindow(h: *mut c_void) -> i32;
    fn SetForegroundWindow(h: *mut c_void) -> i32;
    fn GetForegroundWindow() -> *mut c_void;
    fn SendInput(n: u32, inputs: *const Input, size: i32) -> u32;
    fn PeekMessageW(m: *mut [u8; 48], h: *mut c_void, lo: u32, hi: u32, flags: u32) -> i32;
    fn DispatchMessageW(m: *const [u8; 48]) -> isize;
}

#[repr(C)]
struct Input {
    kind: u32,
    align: u32,
    vk: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra: usize,
    pad: [u8; 8],
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn pump() {
    let mut m = [0u8; 48];
    while unsafe { PeekMessageW(&mut m, core::ptr::null_mut(), 0, 0, 1) } != 0 {
        unsafe { DispatchMessageW(&m) };
    }
}

fn key(vk: u16, up: bool) {
    let i = Input { kind: 1, align: 0, vk, scan: 0, flags: if up { 2 } else { 0 }, time: 0, extra: 0, pad: [0; 8] };
    assert_eq!(unsafe { SendInput(1, &i, core::mem::size_of::<Input>() as i32) }, 1);
}

fn wait_for(f: impl Fn(PadState) -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) {
        pump();
        if f(published()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn foreground_key_reaches_the_published_pad_reading() {
    let mut log = std::env::temp_dir();
    log.push("tombox_pad_host_test.log");
    std::env::set_var("TOMBOX_LOG", log);
    std::env::set_var("TOMBOX_PAD_XINPUT", "0");
    std::env::remove_var("TOMBOX_PAD_KEYS");
    assert_eq!(core::mem::size_of::<Input>(), 40);
    let (cls, name) = (wide("Static"), wide("tombox pad host test"));
    let h = unsafe { CreateWindowExW(0, cls.as_ptr(), name.as_ptr(), 0x10CF_0000, 50, 50, 300, 120, core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut()) };
    assert!(!h.is_null());
    unsafe { SetForegroundWindow(h) };
    pump();
    if unsafe { GetForegroundWindow() } != h {
        eprintln!("skipped: this process cannot take the foreground (foreground lock), so no key is injected");
        unsafe { DestroyWindow(h) };
        return;
    }
    compat_common::pad_host::start();
    assert!(wait_for(|s| s == PadState::default()), "pad not neutral before any key: {:?}", published());
    key(0x57, false);
    let pressed = wait_for(|s| s.axes[AX_LY] == 1.0);
    key(0x57, true);
    assert!(pressed, "W did not reach the left stick: {:?}", published());
    assert!(wait_for(|s| s == PadState::default()), "release not published: {:?}", published());
    unsafe { DestroyWindow(h) };
}
