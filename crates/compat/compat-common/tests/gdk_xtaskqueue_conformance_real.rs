use core::ffi::c_void;
use std::sync::OnceLock;

#[macro_use]
#[path = "support/xtq.rs"]
mod xtq;

extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
}

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
const XASYNC: Guid = Guid(0x073b7dcb, 0x1fcf, 0x4030, [0x94, 0xbe, 0xe3, 0xc9, 0xeb, 0x62, 0x34, 0x28]);

const NEWER_THAN_RUNTIME: &[&str] = &[
    "verify_shared_port_queue_rundown_keeps_other_waiters",
    "verify_shared_port_queue_rundown_drops_its_wait_handles",
    "verify_serialized_thread_pool_does_not_strand_items",
];

fn setup(scenario: &str) -> bool {
    if NEWER_THAN_RUNTIME.contains(&scenario) {
        eprintln!("SKIP: {scenario} checks a libHttpClient fix newer than the installed runtime");
        return false;
    }
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| unsafe {
        let name: Vec<u16> = "xgameruntime.dll".encode_utf16().chain(Some(0)).collect();
        let h = LoadLibraryExW(name.as_ptr(), core::ptr::null_mut(), 0x800);
        if h.is_null() {
            eprintln!("SKIP: no System32 xgameruntime.dll (Gaming Services not installed)");
            return false;
        }
        let init: unsafe extern "system" fn(u32, u32) -> i32 = core::mem::transmute(GetProcAddress(h, b"InitializeApiImpl\0".as_ptr()));
        let hr = init(0x4a61, 0x2712);
        if hr < 0 {
            eprintln!("SKIP: InitializeApiImpl failed hr={hr:#010x}");
            return false;
        }
        let query: unsafe extern "system" fn(*const Guid, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(GetProcAddress(h, b"QueryApiImpl\0".as_ptr()));
        let mut obj = core::ptr::null_mut();
        assert_eq!(query(&XASYNC, &XASYNC, &mut obj), 0);
        xtq::install(obj);
        true
    })
}

xtq_tests!(setup);
