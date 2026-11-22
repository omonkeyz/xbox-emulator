use core::ffi::c_void;
use std::sync::OnceLock;

extern "system" {
    fn GetModuleHandleA(n: *const u8) -> *mut c_void;
    fn LoadLibraryA(n: *const u8) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
}

pub fn is_xbox_one() -> bool {
    static IS_XONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if IS_XONE.load(core::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    if unsafe { !GetModuleHandleA(b"d3d12_x.dll\0".as_ptr()).is_null() } {
        IS_XONE.store(true, core::sync::atomic::Ordering::Relaxed);
        return true;
    }
    false
}

pub unsafe fn xg_module() -> (*mut c_void, usize) {
    let mut h = GetModuleHandleA(b"xg_xs.dll\0".as_ptr());
    if h.is_null() && !is_xbox_one() {
        h = LoadLibraryA(b"xg_xs.dll\0".as_ptr());
    }
    if !h.is_null() {
        return (h, 0x18 + 0x28);
    }
    let mut h = GetModuleHandleA(b"xg_x.dll\0".as_ptr());
    if h.is_null() {
        h = LoadLibraryA(b"xg_x.dll\0".as_ptr());
    }
    (h, 0x38)
}

pub fn xg_size(desc: &[u8]) -> Option<u64> {
    if desc.len() < 0x38 || !is_xbox_one() {
        return None;
    }
    static CREATE: OnceLock<usize> = OnceLock::new();
    let f = *CREATE.get_or_init(|| unsafe {
        let (h, _) = xg_module();
        if h.is_null() { 0 } else { GetProcAddress(h, b"XGCreateTextureComputer\0".as_ptr()) as usize }
    });
    if f == 0 {
        return None;
    }
    unsafe {
        let create: unsafe extern "system" fn(*const u8, *mut *mut c_void) -> i32 = core::mem::transmute(f);
        let mut comp: *mut c_void = core::ptr::null_mut();
        if create(desc.as_ptr(), &mut comp) < 0 || comp.is_null() {
            return None;
        }
        let vt = *(comp as *const *const usize);
        let size: unsafe extern "system" fn(*mut c_void) -> u64 = core::mem::transmute(*vt.add(3));
        let n = size(comp);
        let rel: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt.add(1));
        rel(comp);
        Some(n)
    }
}

pub fn committed_state(heap_type: u32, state: u32) -> u32 {
    match heap_type {
        2 if state != 0xac3 => 0xac3,
        3 if state != 0x400 => 0x400,
        _ => state,
    }
}

pub fn committed_flags(flags: u32) -> u32 {
    if flags & 0x8 != 0 && flags & 0x2 == 0 { flags & !0x8 } else { flags }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_and_readback_states() {
        assert_eq!(committed_state(2, 0x8c3), 0xac3);
        assert_eq!(committed_state(2, 0xac3), 0xac3);
        assert_eq!(committed_state(3, 0), 0x400);
        assert_eq!(committed_state(1, 0x8c3), 0x8c3);
        assert_eq!(committed_state(4, 0x4), 0x4);
    }

    #[test]
    fn deny_srv_needs_depth() {
        assert_eq!(committed_flags(0x8), 0);
        assert_eq!(committed_flags(0xa), 0xa);
        assert_eq!(committed_flags(0x4 | 0x8), 0x4);
        assert_eq!(committed_flags(0x1), 0x1);
    }
}
