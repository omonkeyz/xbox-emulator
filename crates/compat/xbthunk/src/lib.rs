use core::ffi::c_void;
use std::sync::{Mutex, OnceLock};

type Handle = *mut c_void;

pub const ERROR_INVALID_PARAMETER: u32 = 0x57;
pub const ERROR_BAD_ARGUMENTS: u32 = 0xa0;
pub const ERROR_PARTITION_NOT_FOUND: u32 = 0x10d8;
pub const REDIRECTED_FUNCTIONS: u32 = 0x4a;
const PARTITION_NAMES: [&str; 3] = ["\\KernelObjects\\MemoryPartition0", "\\GLOBAL??\\MemoryPartitionXboxTitle", "\\GLOBAL??\\MemoryPartitionXboxTools"];
const PARTITION_ALL_ACCESS: u32 = 0x1f_0003;
const DUPLICATE_SAME_ACCESS: u32 = 2;
const PARTITION_HEAP_TYPE: [u32; 3] = [2, 0, 2];
const HEAP_FLAGS: u32 = 0x102;

pub const REDIRECT_FLAGS: [u8; 74] = [
    1, 2, 2, 1, 0, 2, 0, 0, 2, 0, 0, 0, 0, 1, 1, 1, 0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 0, 1, 0, 0, 0, 0, 2, 1, 2, 2, 2, 1, 1, 3, 3, 1, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 1,
];

pub const REDIRECT_FUNCTIONS: [&str; 74] = [
    "CreateThread", "GetProcessHeap", "HeapAlloc", "HeapCreate", "HeapDestroy", "HeapFree", "HeapLock", "HeapQueryInformation", "HeapReAlloc",
    "HeapSetInformation", "HeapSize", "HeapUnlock", "HeapWalk", "VirtualAlloc", "VirtualAllocEx", "VirtualFree", "VirtualFreeEx", "CreateFileA",
    "CreateFileW", "CreateFile2", "CreateEventA", "CreateEventW", "CreateSemaphoreA", "CreateSemaphoreW", "CreateSemaphoreExA", "CreateSemaphoreExW",
    "RtlAllocateHeap", "RtlCreateHeap", "RtlDestroyHeap", "RtlFreeHeap", "RtlReAllocateHeap", "RtlSizeHeap", "HeapAlloc", "HeapCreate", "HeapReAlloc",
    "HeapFree", "GetProcessHeap", "VirtualAlloc", "VirtualAllocEx", "VirtualFree", "VirtualFreeEx", "CreateThread", "CreateFileMappingA",
    "CreateFileMappingW", "FatalExit", "CreateFileA", "CreateFileW", "CreateFile2", "CreateEventA", "CreateEventW", "CreateSemaphoreA",
    "CreateSemaphoreW", "CreateSemaphoreExA", "CreateSemaphoreExW", "CoGetMalloc", "GlobalAlloc", "GlobalAlloc", "LocalAlloc", "LocalAlloc",
    "CoTaskMemAlloc", "CoTaskMemAlloc", "WinHttpOpen", "WinHttpConnect", "WinHttpOpenRequest", "WinHttpCloseHandle", "WinHttpReceiveResponse",
    "WinHttpSetOption", "CoCreateGuid", "WinHttpWebSocketCompleteUpgrade", "OutputDebugStringA", "OutputDebugStringW", "OutputDebugStringA",
    "OutputDebugStringW", "GetProcessHeap",
];

pub const SKIP_LIST: [&str; 10] = [
    "d3dcompiler_47.dll", "dxcompiler_x.dll", "dxcompiler_xs.dll", "hix_xs.dll", "mwdbgsrv.exe", "newbe.dll", "newbe_xs.dll", "scdxil.dll", "sc_dll.dll", "webio.dll",
];

fn module_override(index: u32) -> Option<&'static str> {
    match index {
        67 => Some("pgort140.dll"),
        73 => Some("dbghelp.dll"),
        _ => None,
    }
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
    sd: *const c_void,
    qos: *const c_void,
}

#[repr(C)]
#[derive(Default)]
pub struct SegmentHeapParameters {
    pub version_size: u32,
    pub flags: u32,
    pub source_flags: u32,
    pub memory_type_mask: u32,
    pub numa_node: u32,
    pub partition: usize,
    pub reserved: [usize; 2],
    pub tail: [u64; 4],
}

const STATUS_NOT_IMPLEMENTED: i32 = 0xc000_0002u32 as i32;

/// Address of an ntdll export, null when this ntdll has none by that name.
fn ntdll_export(name: &'static [u8]) -> *const c_void {
    let ntdll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
    unsafe { GetProcAddress(GetModuleHandleW(ntdll.as_ptr()), name.as_ptr()) }
}

// The two partition calls are looked up on first use and not imported. An ntdll without
// them (Wine, which turns a missing import into a stub that aborts the process when
// called) gives STATUS_NOT_IMPLEMENTED, and `open_partition` treats that like any other
// partition it cannot open.
#[allow(non_snake_case)]
unsafe fn NtOpenPartition(out: *mut Handle, access: u32, attrs: *const ObjectAttributes) -> i32 {
    static F: OnceLock<usize> = OnceLock::new();
    match *F.get_or_init(|| ntdll_export(b"NtOpenPartition\0") as usize) {
        0 => STATUS_NOT_IMPLEMENTED,
        f => core::mem::transmute::<usize, unsafe extern "system" fn(*mut Handle, u32, *const ObjectAttributes) -> i32>(f)(out, access, attrs),
    }
}

#[allow(non_snake_case)]
unsafe fn NtManagePartition(target: Handle, source: Handle, class: u32, info: *mut u8, len: u32) -> i32 {
    static F: OnceLock<usize> = OnceLock::new();
    match *F.get_or_init(|| ntdll_export(b"NtManagePartition\0") as usize) {
        0 => STATUS_NOT_IMPLEMENTED,
        f => core::mem::transmute::<usize, unsafe extern "system" fn(Handle, Handle, u32, *mut u8, u32) -> i32>(f)(target, source, class, info, len),
    }
}

#[link(name = "ntdll", kind = "raw-dylib")]
extern "system" {
    fn RtlCreateHeap(flags: u32, base: *mut c_void, reserve: usize, commit: usize, lock: *mut c_void, params: *mut c_void) -> Handle;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn DuplicateHandle(sp: Handle, s: Handle, tp: Handle, t: *mut Handle, access: u32, inherit: i32, options: u32) -> i32;
    fn SetLastError(e: u32);
    fn GetModuleHandleW(name: *const u16) -> isize;
    fn GetProcAddress(m: isize, name: *const u8) -> *const c_void;
    fn GetModuleFileNameW(m: isize, buf: *mut u16, n: u32) -> u32;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Partition {
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
        return Partition::Real(h as usize, id);
    }
    if i == 0 || i == 1 {
        Partition::Process
    } else {
        Partition::Missing
    }
}

pub fn partition(i: usize) -> Partition {
    static P: OnceLock<[Partition; 3]> = OnceLock::new();
    P.get_or_init(|| [open_partition(0), open_partition(1), open_partition(2)])[i]
}

pub fn heap_partition(i: u32) -> Result<usize, u32> {
    if i > 2 {
        return Err(ERROR_INVALID_PARAMETER);
    }
    match partition(i as usize) {
        Partition::Real(h, _) => Ok(h),
        Partition::Process => Ok(0),
        Partition::Missing => Err(ERROR_PARTITION_NOT_FOUND),
    }
}

pub fn segment_parameters(partition: usize, memory_type: u32) -> SegmentHeapParameters {
    let mut mask = 1u32.wrapping_shl(memory_type);
    if memory_type == 4 {
        mask |= 8;
    }
    SegmentHeapParameters {
        version_size: 0x50_0003,
        numa_node: 0xffff_ffff,
        memory_type_mask: mask & 0xff,
        partition: if memory_type >= 2 { partition } else { 0 },
        ..Default::default()
    }
}

pub unsafe fn partition_create_heap(partition: usize, memory_type: u32, flags: u32, base: *mut c_void, reserve: usize, commit: usize, lock: *mut c_void, params: *mut c_void) -> Handle {
    let mut built = segment_parameters(partition, memory_type);
    let (flags, reserve, commit, params) = if params.is_null() {
        (flags | HEAP_FLAGS, 0, 0, (&mut built as *mut SegmentHeapParameters).cast())
    } else {
        (flags, reserve, commit, params)
    };
    let mut h = RtlCreateHeap(flags, base, reserve, commit, lock, params);
    if h.is_null() && params == (&mut built as *mut SegmentHeapParameters).cast() && partition == 0 && built.memory_type_mask != 1 {
        built.memory_type_mask = 1;
        h = RtlCreateHeap(flags, base, reserve, commit, lock, params);
    }
    if h.is_null() {
        SetLastError(ERROR_BAD_ARGUMENTS);
    }
    h
}

unsafe fn create_heap(partition: usize, memory_type: u32, flags: u32, size: usize) -> Handle {
    partition_create_heap(partition, memory_type, flags, core::ptr::null_mut(), size, size, core::ptr::null_mut(), core::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "system" fn XbCreatePartitionHeap(partition_index: u32, memory_type: u32, size: usize) -> Handle {
    match heap_partition(partition_index) {
        Ok(h) => create_heap(h, memory_type, 0x100, size),
        Err(e) => {
            SetLastError(e);
            core::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn XbGetPartitionHandleAndId(partition_index: u32, handle: *mut Handle, id: *mut u32) -> u8 {
    if partition_index > 2 {
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let (source, pid) = match partition(partition_index as usize) {
        Partition::Real(h, pid) => (h as Handle, pid),
        Partition::Process => (GetCurrentProcess(), 0),
        Partition::Missing => {
            SetLastError(ERROR_PARTITION_NOT_FOUND);
            return 0;
        }
    };
    if !handle.is_null() {
        let cur = GetCurrentProcess();
        if DuplicateHandle(cur, source, cur, handle, 0, 1, DUPLICATE_SAME_ACCESS) == 0 {
            return 0;
        }
    }
    if !id.is_null() {
        *id = pid;
    }
    1
}

#[no_mangle]
pub unsafe extern "system" fn XbGetPartitionHeap(partition_index: u32) -> Handle {
    static HEAPS: Mutex<[usize; 3]> = Mutex::new([0; 3]);
    let h = match heap_partition(partition_index) {
        Ok(h) => h,
        Err(e) => {
            SetLastError(e);
            return core::ptr::null_mut();
        }
    };
    let mut g = HEAPS.lock().unwrap_or_else(|e| e.into_inner());
    let slot = &mut g[partition_index as usize];
    if *slot == 0 {
        *slot = create_heap(h, PARTITION_HEAP_TYPE[partition_index as usize], 0, 0) as usize;
    }
    *slot as Handle
}

fn image_name() -> String {
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(0, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

pub static LAST_AVM_ERROR: Mutex<Option<(i32, usize, usize, u32, u64, u32)>> = Mutex::new(None);

#[no_mangle]
pub extern "system" fn XbReportUnknownNtAvmError(status: i32, address: usize, region: usize, size: u32, extra: u64, kind: u32) {
    *LAST_AVM_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = Some((status, address, region, size, extra, kind));
    compat_common::record(
        "xbthunk",
        "XbReportUnknownNtAvmError",
        &format!("status={:#010x} address={address:#x} region={region:#x} size={size:#x} extra={extra:#x} kind={kind} image={}", status as u32, image_name()),
    );
}

pub static WINHTTP_HOOKS: Mutex<[usize; 7]> = Mutex::new([0; 7]);

#[no_mangle]
pub unsafe extern "system" fn XbSetWinHttpHooks(table: *const [usize; 7]) {
    let t = core::ptr::read_unaligned(table);
    *WINHTTP_HOOKS.lock().unwrap_or_else(|e| e.into_inner()) = t;
    let name: Vec<u16> = "xmem.dll".encode_utf16().chain(Some(0)).collect();
    let m = GetModuleHandleW(name.as_ptr());
    if m != 0 {
        let f = GetProcAddress(m, b"XMemSetWinHttpHooks\0".as_ptr());
        if !f.is_null() && GetProcAddress(m, b"TomboxAddressSpaceEpoch\0".as_ptr()) != core::ptr::null() {
            let f: unsafe extern "system" fn(*const [usize; 7]) = core::mem::transmute(f);
            f(table);
        }
    }
}

fn ends_with_ci(s: &[u16], suffix: &str) -> bool {
    let suf: Vec<u16> = suffix.encode_utf16().collect();
    if s.len() < suf.len() {
        return false;
    }
    s[s.len() - suf.len()..].iter().zip(&suf).all(|(&a, &b)| upcase(a) == upcase(b))
}

fn upcase(c: u16) -> u16 {
    char::from_u32(c as u32).map_or(c, |ch| {
        let mut u = ch.to_uppercase();
        match (u.next(), u.next()) {
            (Some(x), None) if (x as u32) < 0x10000 => x as u32 as u16,
            _ => c,
        }
    })
}

fn eq_ci(a: &[u16], b: &str) -> bool {
    let b: Vec<u16> = b.encode_utf16().collect();
    a.len() == b.len() && a.iter().zip(&b).all(|(&x, &y)| upcase(x) == upcase(y))
}

pub fn should_redirect(module: &[u16], index: u32) -> bool {
    if index >= REDIRECTED_FUNCTIONS || ends_with_ci(module, "xmem.dll") {
        return false;
    }
    if let Some(m) = module_override(index) {
        return ends_with_ci(module, m);
    }
    let mut bit = 2u8;
    if !ends_with_ci(module, "ucrtbase.dll") && !SKIP_LIST.iter().any(|s| eq_ci(module, s)) {
        let prefix: Vec<u16> = "C:\\".encode_utf16().collect();
        if module.len() >= prefix.len() && module[..prefix.len()].iter().zip(&prefix).all(|(&a, &b)| upcase(a) == upcase(b)) {
            bit = 1;
        }
    }
    REDIRECT_FLAGS[index as usize] & bit != 0
}

#[no_mangle]
pub unsafe extern "system" fn __ShouldApplyRedirectionToFunction__(module: *const u16, index: u32) -> u8 {
    if index >= REDIRECTED_FUNCTIONS {
        return 0;
    }
    let mut n = 0usize;
    if !module.is_null() {
        while *module.add(n) != 0 {
            n += 1;
        }
    }
    let s = if module.is_null() { &[][..] } else { core::slice::from_raw_parts(module, n) };
    should_redirect(s, index) as u8
}

#[no_mangle]
pub extern "system" fn __ShouldApplyRedirection__() -> u8 {
    1
}

pub mod redirect;

#[cfg(test)]
pub(crate) static TEST_SERIAL: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests;
