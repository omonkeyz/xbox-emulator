mod alloc;
pub mod epoch;
mod heap;
mod hooks;
mod iat;
mod phys;
mod pool;
mod range_hooks;
mod stats;
mod sys;
mod virt;
mod write_note;

use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use compat_common::record;
use sys::Handle;

const M: &str = "xmem";

#[repr(C)]
pub struct XMemOption {
    pub id: u32,
    pub pad: u32,
    pub value: u64,
}

pub const XMEM_OPTION_WRITE_WATCH_SUSPEND: u32 = 0x10000;
pub const XMEM_OPTION_REALLOC_REPORTS_FREE: u32 = 3;
pub const XMEM_OPTION_POOL_GRANULE: u32 = 6;

static OPTIONS: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());

pub fn option(id: u32) -> Option<u64> {
    OPTIONS.lock().unwrap().iter().find(|(i, _)| *i == id).map(|(_, v)| *v)
}

#[no_mangle]
pub unsafe extern "system" fn XMemSetOption(opt: *const XMemOption) {
    if opt.is_null() {
        return;
    }
    let id = (*opt).id;
    let value = if id == 1 || id == 2 || id == 3 || id == 4 || id == 5 || id == 6 || id == 0x10001 {
        (*opt).value & 0xffff_ffff
    } else {
        (*opt).value
    };
    if id == XMEM_OPTION_POOL_GRANULE && !pool::set_large_granule(value == 0) {
        record(M, "XMemSetOption", "pool granule option after the first pool was created; console rejects it (E_NOTIMPL trace)");
    }
    if id == XMEM_OPTION_REALLOC_REPORTS_FREE {
        hooks::REALLOC_REPORTS_FREE.store(value as u32, Ordering::Release);
    }
    if id == XMEM_OPTION_WRITE_WATCH_SUSPEND {
        let v = value as u32;
        virt::WRITE_WATCH_OPTION.store(v as u64, Ordering::Release);
        if v != 0 {
            virt::WRITE_WATCH_SUSPENDED.store(1, Ordering::Release);
        }
    }
    let mut g = OPTIONS.lock().unwrap();
    match g.iter_mut().find(|(i, _)| *i == id) {
        Some(e) => e.1 = value,
        None => g.push((id, value)),
    }
    record(M, "XMemSetOption", &format!("id={id:#x} value={value:#x}"));
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct GraphicsMemoryConfiguration {
    pub page_table_bytes: u32,
    pub frame_buffer_preserve_bytes: u32,
    pub output0: u64,
    pub output1: u64,
}

static GRAPHICS_CONFIG: Mutex<Option<GraphicsMemoryConfiguration>> = Mutex::new(None);

#[no_mangle]
pub unsafe extern "system" fn XMemRegisterGraphicsMemoryConfiguration(cfg: *const GraphicsMemoryConfiguration) {
    if cfg.is_null() {
        return;
    }
    let mut g = GRAPHICS_CONFIG.lock().unwrap();
    if g.is_none() {
        *g = Some(*cfg);
        record(M, "XMemRegisterGraphicsMemoryConfiguration", &format!("{:?}", *cfg));
    }
}

pub fn graphics_configuration() -> Option<GraphicsMemoryConfiguration> {
    *GRAPHICS_CONFIG.lock().unwrap()
}

#[no_mangle]
pub static XMemHistoryData: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

#[link(name = "kernel32")]
extern "system" {
    fn DuplicateHandle(sp: Handle, sh: Handle, tp: Handle, out: *mut Handle, access: u32, inherit: i32, options: u32) -> i32;
}

#[repr(C)]
struct UnicodeString {
    len: u16,
    max: u16,
    buf: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    len: u32,
    root: Handle,
    name: *const UnicodeString,
    attributes: u32,
    sd: *const core::ffi::c_void,
    qos: *const core::ffi::c_void,
}

const STATUS_NOT_IMPLEMENTED: i32 = 0xc000_0002u32 as i32;

/// Address of an ntdll export, 0 when this ntdll has none by that name.
fn ntdll_export(name: &'static [u8]) -> usize {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleA(name: *const u8) -> usize;
        fn GetProcAddress(h: usize, name: *const u8) -> usize;
    }
    unsafe { GetProcAddress(GetModuleHandleA(b"ntdll.dll\0".as_ptr()), name.as_ptr()) }
}

// The two partition calls are looked up on first use and not imported. An ntdll without
// them (Wine, which turns a missing import into a stub that aborts the process when
// called) gives STATUS_NOT_IMPLEMENTED, and `open_partition` treats that like any other
// partition it cannot open.
#[allow(non_snake_case)]
unsafe fn NtOpenPartition(out: *mut Handle, access: u32, attrs: *const ObjectAttributes) -> i32 {
    static F: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    match *F.get_or_init(|| ntdll_export(b"NtOpenPartition\0")) {
        0 => STATUS_NOT_IMPLEMENTED,
        f => core::mem::transmute::<usize, unsafe extern "system" fn(*mut Handle, u32, *const ObjectAttributes) -> i32>(f)(out, access, attrs),
    }
}

#[allow(non_snake_case)]
unsafe fn NtManagePartition(target: Handle, source: Handle, class: u32, info: *mut u8, len: u32) -> i32 {
    static F: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    match *F.get_or_init(|| ntdll_export(b"NtManagePartition\0")) {
        0 => STATUS_NOT_IMPLEMENTED,
        f => core::mem::transmute::<usize, unsafe extern "system" fn(Handle, Handle, u32, *mut u8, u32) -> i32>(f)(target, source, class, info, len),
    }
}

pub const PARTITION_SYSTEM: u32 = 0;
pub const PARTITION_TITLE: u32 = 1;
pub const PARTITION_TOOLS: u32 = 2;
const PARTITION_NAMES: [&str; 3] = ["\\KernelObjects\\MemoryPartition0", "\\GLOBAL??\\MemoryPartitionXboxTitle", "\\GLOBAL??\\MemoryPartitionXboxTools"];
const PARTITION_ALL_ACCESS: u32 = 0x1f_0003;
const ERROR_PARTITION_NOT_FOUND: u32 = 0x10d8;
const DUPLICATE_SAME_ACCESS: u32 = 2;

#[derive(Clone, Copy)]
enum Partition {
    Real(usize, u32),
    Process,
    Missing,
}

fn open_partition(i: usize) -> Partition {
    let w: Vec<u16> = PARTITION_NAMES[i].encode_utf16().collect();
    let us = UnicodeString { len: (w.len() * 2) as u16, max: (w.len() * 2) as u16, buf: w.as_ptr() };
    let oa = ObjectAttributes { len: core::mem::size_of::<ObjectAttributes>() as u32, root: core::ptr::null_mut(), name: &us, attributes: 0x40, sd: core::ptr::null(), qos: core::ptr::null() };
    let mut h: Handle = core::ptr::null_mut();
    let st = unsafe { NtOpenPartition(&mut h, PARTITION_ALL_ACCESS, &oa) };
    if st >= 0 && !h.is_null() {
        let mut info = [0u8; 0xf0];
        let q = unsafe { NtManagePartition(h, core::ptr::null_mut(), 0, info.as_mut_ptr(), info.len() as u32) };
        let id = if q >= 0 { u32::from_le_bytes(info[0xe8..0xec].try_into().unwrap()) } else { 0 };
        record(M, "XMemGetPartitionHandleAndId", &format!("opened {} id={id}", PARTITION_NAMES[i]));
        return Partition::Real(h as usize, id);
    }
    let stand_in = i as u32 == PARTITION_SYSTEM || i as u32 == PARTITION_TITLE;
    record(M, "XMemGetPartitionHandleAndId", &format!("{} not openable (status={st:#010x}); {}", PARTITION_NAMES[i], if stand_in { "title memory lives in the PC system partition, using the process as its handle" } else { "no tools partition on PC" }));
    if stand_in { Partition::Process } else { Partition::Missing }
}

fn partition(i: usize) -> Partition {
    static P: std::sync::OnceLock<[Partition; 3]> = std::sync::OnceLock::new();
    P.get_or_init(|| [open_partition(0), open_partition(1), open_partition(2)])[i]
}

#[no_mangle]
pub unsafe extern "system" fn XMemGetPartitionHandleAndId(partition_index: u32, handle: *mut Handle, id: *mut u32) -> u8 {
    if partition_index > PARTITION_TOOLS {
        sys::SetLastError(sys::ERROR_INVALID_PARAMETER);
        return 0;
    }
    let (source, pid) = match partition(partition_index as usize) {
        Partition::Real(h, pid) => (h as Handle, pid),
        Partition::Process => (sys::GetCurrentProcess(), 0),
        Partition::Missing => {
            sys::SetLastError(ERROR_PARTITION_NOT_FOUND);
            return 0;
        }
    };
    if !handle.is_null() {
        let cur = sys::GetCurrentProcess();
        if DuplicateHandle(cur, source, cur, handle, 0, 0, DUPLICATE_SAME_ACCESS) == 0 {
            return 0;
        }
        if let Partition::Process = partition(partition_index as usize) {
            hooks::install_partition_hooks();
        }
    }
    if !id.is_null() {
        *id = pid;
    }
    1
}

#[no_mangle]
pub unsafe extern "system" fn XMemSetWinHttpHooks(table: *const [usize; 7]) {
    if table.is_null() {
        return;
    }
    let t = core::ptr::read_unaligned(table);
    for (slot, v) in hooks::WINHTTP_HOOKS.iter().zip(t) {
        slot.store(v, Ordering::Release);
    }
    if t.iter().any(|&v| v != 0) {
        hooks::install_winhttp_hooks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_configuration_registers_once() {
        let a = GraphicsMemoryConfiguration { page_table_bytes: 0x80_0000, frame_buffer_preserve_bytes: 0x1000, output0: 1, output1: 2 };
        let b = GraphicsMemoryConfiguration { page_table_bytes: 1, ..a };
        unsafe { XMemRegisterGraphicsMemoryConfiguration(&a) };
        unsafe { XMemRegisterGraphicsMemoryConfiguration(&b) };
        assert_eq!(graphics_configuration(), Some(a));
        assert_eq!(core::mem::size_of::<GraphicsMemoryConfiguration>(), 0x18);
    }

    #[test]
    fn options_are_stored() {
        let o = XMemOption { id: 5, pad: 0, value: 0xffff_ffff_0000_0007 };
        unsafe { XMemSetOption(&o) };
        assert_eq!(option(5), Some(7));
    }

    #[test]
    fn partition_query_follows_xbthunk() {
        extern "system" {
            fn CloseHandle(h: usize) -> i32;
            fn GetProcessId(h: usize) -> u32;
            fn GetCurrentProcessId() -> u32;
        }
        let mut h: Handle = core::ptr::null_mut();
        let mut id = 9u32;
        assert_eq!(unsafe { XMemGetPartitionHandleAndId(1, &mut h, &mut id) }, 1);
        assert!(!h.is_null());
        assert_eq!(id, 0);
        assert_eq!(unsafe { GetProcessId(h as usize) }, unsafe { GetCurrentProcessId() });
        assert_eq!(unsafe { CloseHandle(h as usize) }, 1);
        assert_eq!(unsafe { XMemGetPartitionHandleAndId(0, core::ptr::null_mut(), &mut id) }, 1);
        assert_eq!(unsafe { XMemGetPartitionHandleAndId(2, &mut h, &mut id) }, 0);
        assert_eq!(unsafe { sys::GetLastError() }, ERROR_PARTITION_NOT_FOUND);
        assert_eq!(unsafe { XMemGetPartitionHandleAndId(3, &mut h, &mut id) }, 0);
        assert_eq!(unsafe { sys::GetLastError() }, sys::ERROR_INVALID_PARAMETER);
        assert_eq!(core::mem::size_of_val(&XMemHistoryData), 0x28);
        let me = unsafe { sys::GetCurrentProcess() } as u64;
        let kept = hooks::strip_for_test(&[[3, me], [1, 0x1000], [3, 0]]);
        assert_eq!(kept, vec![[1, 0x1000], [3, 0]]);
    }

    static WH: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

    unsafe extern "system" fn fake_close(h: usize) -> usize {
        WH.lock().unwrap().push(h);
        7
    }

    #[test]
    fn winhttp_hooks_replace_calls() {
        let table = [0usize, 0, 0, 0, fake_close as *const () as usize, 0, 0];
        unsafe { XMemSetWinHttpHooks(&table) };
        assert_eq!(hooks::WINHTTP_HOOKS[4].load(Ordering::Acquire), fake_close as *const () as usize);
        let w = hooks::winhttp_rule(b"WINHTTP.dll", Some(b"WinHttpCloseHandle"), 0x1234).unwrap();
        let f: unsafe extern "system" fn(usize) -> usize = unsafe { core::mem::transmute(w) };
        assert_eq!(unsafe { f(0x55) }, 7);
        assert_eq!(*WH.lock().unwrap(), vec![0x55]);
        assert!(hooks::winhttp_rule(b"WINHTTP.dll", Some(b"WinHttpCloseHandle"), 0x9999).is_none());
        assert!(hooks::winhttp_rule(b"WINHTTP.dll", Some(b"WinHttpCloseHandle"), w).is_none());
        unsafe { XMemSetWinHttpHooks(&[0; 7]) };
    }
}
