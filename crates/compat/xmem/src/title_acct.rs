use core::ffi::c_void;
use core::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::sys::*;
use compat_common::record;

const M: &str = "xmem";
const MEM_PRIVATE: u32 = 0x2_0000;
const MEM_FREE: u32 = 0x1_0000;
const MEM_DECOMMIT: u32 = 0x4000;
const MEM_PRESERVE_PLACEHOLDER: u32 = 0x2;
pub const MEM_LARGE_PAGES: u32 = 0x2000_0000;
const SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const SCN_UNINITIALIZED: u32 = 0x80;

const HOST_MODULES: &[&str] = &["dxcompiler", "dxil", "d3d12core", "d3d12sdklayers", "d3d10warp"];
const RUST_MARKS: [&[u8]; 4] = [b"/rustc/", b"\\rustc\\", b"library/core/src", b"library\\core\\src"];

pub fn enabled() -> bool {
    static ON: AtomicUsize = AtomicUsize::new(2);
    match ON.load(Ordering::Relaxed) {
        0 => false,
        1 => true,
        _ => {
            let on = env_trimmed_is(b"TOMBOX_XMEM_TITLE_ACCT\0", b"0") != Some(true);
            ON.store(on as usize, Ordering::Relaxed);
            on
        }
    }
}

pub fn host_module_name(stem: &str) -> bool {
    HOST_MODULES.contains(&stem) || stem.ends_with("_sys") || stem.ends_with("_console")
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        match hay[i..hay.len() - needle.len() + 1].iter().position(|&b| b == first) {
            Some(k) => {
                let at = i + k;
                if &hay[at..at + needle.len()] == needle {
                    return true;
                }
                i = at + 1;
            }
            None => return false,
        }
    }
    false
}

pub fn has_rust_marks(bytes: &[u8]) -> bool {
    RUST_MARKS.iter().any(|m| find(bytes, m))
}

static BUILT: Mutex<Option<HashMap<(usize, usize), bool>>> = Mutex::new(None);

pub unsafe fn built_by_us(base: usize) -> bool {
    let size = match crate::iat::image_size(base) {
        Some(s) => s,
        None => return false,
    };
    if let Some(&v) = BUILT.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(&(base, size)) {
        return v;
    }
    let rd = |a: usize, n: usize| -> Option<Vec<u8>> {
        let mut v = vec![0u8; n];
        if crate::iat::copy_out(a, &mut v) { Some(v) } else { None }
    };
    let Some(dos) = rd(base + 0x3c, 4) else { return false };
    let pe = base + u32::from_le_bytes(dos[..4].try_into().unwrap()) as usize;
    let Some(fh) = rd(pe, 24) else { return false };
    let nsec = u16::from_le_bytes([fh[6], fh[7]]) as usize;
    let opt = u16::from_le_bytes([fh[20], fh[21]]) as usize;
    let first = pe + 24 + opt;
    let mut found = false;
    for i in 0..nsec.min(96) {
        let Some(h) = rd(first + i * 40, 40) else { break };
        let u32at = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
        let (vsize, va, ch) = (u32at(8) as usize, u32at(12) as usize, u32at(36));
        if ch & (SCN_MEM_EXECUTE | SCN_UNINITIALIZED) != 0 || vsize == 0 || va + vsize > size {
            continue;
        }
        if let Some(data) = rd(base + va, vsize) {
            if has_rust_marks(&data) {
                found = true;
                break;
            }
        }
    }
    BUILT.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert((base, size), found);
    found
}

static VIRT: AtomicI64 = AtomicI64::new(0);

#[cfg(test)]
thread_local! {
    static LOCAL_VIRT: core::cell::Cell<i64> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
pub fn local_virt() -> i64 {
    LOCAL_VIRT.with(|c| c.get())
}

pub fn note_virt(delta: i64) {
    if delta == 0 {
        return;
    }
    VIRT.fetch_add(delta, Ordering::AcqRel);
    #[cfg(test)]
    LOCAL_VIRT.with(|c| c.set(c.get() + delta));
}

pub fn virt_bytes() -> u64 {
    VIRT.load(Ordering::Acquire).max(0) as u64
}

pub const GPU_PAGE: u64 = 0x1_0000;

static GPU: AtomicI64 = AtomicI64::new(0);
static GPU_LIVE: AtomicI64 = AtomicI64::new(0);
static GPU_CLAMPED: AtomicI64 = AtomicI64::new(0);

pub fn gpu_round(bytes: u64) -> u64 {
    bytes.saturating_add(GPU_PAGE - 1) & !(GPU_PAGE - 1)
}

pub fn gpu_charge(bytes: u64, release: bool) -> u64 {
    if !enabled() || bytes == 0 {
        return gpu_bytes();
    }
    let n = gpu_round(bytes).min(i64::MAX as u64) as i64;
    if release {
        let mut clamped = false;
        let _ = GPU.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
            clamped = v < n;
            Some((v - n).max(0))
        });
        if clamped {
            GPU_CLAMPED.fetch_add(1, Ordering::Relaxed);
        }
        GPU_LIVE.fetch_sub(1, Ordering::AcqRel);
    } else {
        GPU.fetch_add(n, Ordering::AcqRel);
        GPU_LIVE.fetch_add(1, Ordering::AcqRel);
    }
    gpu_bytes()
}

pub fn gpu_bytes() -> u64 {
    GPU.load(Ordering::Acquire).max(0) as u64
}

pub fn gpu_objects() -> (i64, i64) {
    (GPU_LIVE.load(Ordering::Acquire), GPU_CLAMPED.load(Ordering::Relaxed))
}

fn query(addr: usize) -> Option<MemoryBasicInformation> {
    let mut m = MemoryBasicInformation::default();
    let n = unsafe { VirtualQueryEx(GetCurrentProcess(), addr as *const c_void, &mut m, core::mem::size_of::<MemoryBasicInformation>()) };
    if n == 0 { None } else { Some(m) }
}

pub fn committed_private(lo: usize, hi: usize) -> u64 {
    let mut total = 0u64;
    let mut p = lo;
    while p < hi {
        let m = match query(p) {
            Some(m) => m,
            None => break,
        };
        let end = m.base.saturating_add(m.region_size);
        if end <= p {
            break;
        }
        if m.state == MEM_COMMIT && m.kind == MEM_PRIVATE {
            total += (end.min(hi) - p.max(m.base)) as u64;
        }
        p = end;
    }
    total
}

pub fn extent_end(addr: usize) -> usize {
    let first = match query(addr) {
        Some(m) if m.state != MEM_FREE => m,
        _ => return addr,
    };
    let base = first.alloc_base;
    let mut p = first.base;
    loop {
        match query(p) {
            Some(m) if m.alloc_base == base && m.state != MEM_FREE && m.region_size != 0 => p = m.base + m.region_size,
            _ => return p,
        }
    }
}

pub fn commit_range(addr: usize, size: usize, kind: u32) -> (usize, usize) {
    let g = if kind & MEM_LARGE_PAGES != 0 { 0x20_0000 } else { 0x1000 };
    let lo = addr & !(g - 1);
    (lo, round_up(addr.saturating_add(size.max(1)), g))
}

pub fn free_range(addr: usize, size: usize) -> (usize, usize) {
    let lo = addr & !0xfff;
    let hi = if size == 0 { crate::phys::owned_extent(addr).map_or_else(|| extent_end(addr), |(_, e)| e.max(extent_end(addr))) } else { round_up(addr.saturating_add(size), 0x1000) };
    (lo, hi.max(lo))
}

pub fn track_commit(range: Option<(usize, usize)>, op: impl FnOnce() -> usize) -> usize {
    if !enabled() {
        return op();
    }
    match range {
        None => {
            let r = op();
            if r != 0 {
                let end = crate::phys::owned_extent(r).map_or(0, |(_, e)| e).max(extent_end(r));
                note_virt(committed_private(r, end) as i64);
            }
            r
        }
        Some((lo, hi)) => {
            let before = committed_private(lo, hi);
            let r = op();
            if r != 0 {
                note_virt(committed_private(lo, hi) as i64 - before as i64);
            }
            r
        }
    }
}

pub fn track_alloc(addr: usize, size: usize, kind: u32, op: impl FnOnce() -> usize) -> usize {
    if kind & MEM_COMMIT == 0 {
        return op();
    }
    track_commit(if addr == 0 { None } else { Some(commit_range(addr, size, kind)) }, op)
}

fn track_free_with(addr: usize, size: usize, kind: u32, ok: impl FnOnce(i32) -> bool, op: impl FnOnce() -> i32) -> i32 {
    if !enabled() || kind & (MEM_RELEASE | MEM_DECOMMIT) == 0 || addr == 0 {
        return op();
    }
    let (lo, hi) = free_range(addr, size);
    let before = committed_private(lo, hi);
    let r = op();
    if ok(r) {
        let gone = kind & MEM_RELEASE != 0 && kind & MEM_PRESERVE_PLACEHOLDER == 0;
        let after = if gone { 0 } else { committed_private(lo, hi) };
        note_virt(after as i64 - before as i64);
    }
    r
}

pub fn track_free(addr: usize, size: usize, kind: u32, op: impl FnOnce() -> i32) -> i32 {
    track_free_with(addr, size, kind, |r| r != 0, op)
}

pub fn track_free_nt(addr: usize, size: usize, kind: u32, op: impl FnOnce() -> i32) -> i32 {
    track_free_with(addr, size, kind, |st| st >= 0, op)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Parts {
    pub virt: u64,
    pub xmem: u64,
    pub pool: u64,
    pub phys: u64,
    pub heap: u64,
    pub gpu: u64,
}

impl Parts {
    pub fn total(&self) -> u64 {
        self.virt.saturating_add(self.xmem).saturating_add(self.pool).saturating_add(self.phys).saturating_add(self.heap).saturating_add(self.gpu)
    }
}

pub fn parts() -> Parts {
    Parts {
        virt: virt_bytes(),
        xmem: crate::alloc::STATS.iter().map(|s| s.bytes.load(Ordering::Relaxed)).fold(0u64, |a, b| a.saturating_add(b)),
        pool: crate::pool::total_cached_bytes(),
        phys: crate::phys::live_bytes(),
        heap: crate::heap_acct::title_bytes(),
        gpu: gpu_bytes(),
    }
}

pub fn title_rule(dll: &[u8], name: Option<&[u8]>, value: usize) -> Option<usize> {
    crate::hooks::heap_rule(dll, name, value)
        .or_else(|| crate::hooks::vfree_rule(dll, name, value))
        .or_else(|| crate::hooks::vex_rule(dll, name, value))
        .or_else(|| crate::hooks::partition_rule(dll, name, value))
        .or_else(|| crate::range_hooks::rule(dll, name, value))
        .or_else(|| crate::crt_hooks::crt_rule(dll, name, value))
}

static INSTALLED: AtomicUsize = AtomicUsize::new(0);
static INSTALL_SLOTS: AtomicUsize = AtomicUsize::new(0);
static INSTALL_NOTE: Mutex<String> = Mutex::new(String::new());
static LATE_SLOTS: AtomicUsize = AtomicUsize::new(0);

pub fn installed() -> bool {
    INSTALLED.load(Ordering::Acquire) == 2
}

pub fn install() {
    if !enabled() || INSTALLED.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    let ph = unsafe { GetProcessHeap() } as usize;
    let counted = crate::heap_acct::register_title(ph, 0, 0).is_some();
    let mods = crate::iat::title_modules();
    let mut n = 0;
    for &b in &mods {
        n += unsafe { crate::iat::patch_module(b, &title_rule) };
    }
    let names: Vec<String> = mods.iter().map(|&b| crate::iat::module_name(b).unwrap_or_else(|| format!("{b:#x}"))).collect();
    INSTALL_SLOTS.store(n, Ordering::Release);
    let watch = unsafe { watch_loads() };
    *INSTALL_NOTE.lock().unwrap_or_else(|e| e.into_inner()) = format!(
        "title memory accounting: {n} import slots in {} title modules [{}]; process heap {ph:#x} {}; late-load watch {}",
        mods.len(),
        names.join(", "),
        if counted { "counted from zero with title-side wrappers only" } else { "not counted (TOMBOX_XMEM_HEAPACCT=0 or no slot)" },
        if watch { "on" } else { "UNAVAILABLE" }
    );
    INSTALLED.store(2, Ordering::Release);
}

unsafe extern "system" fn dll_loaded(reason: u32, data: *const u8, _ctx: *mut c_void) {
    if reason != 1 || data.is_null() {
        return;
    }
    let full = *(data.add(8) as *const *const u16);
    let base = *(data.add(24) as *const usize);
    if full.is_null() || base == 0 {
        return;
    }
    let len = *(full as *const u16) as usize / 2;
    let buf = *(full.add(4) as *const *const u16);
    if buf.is_null() {
        return;
    }
    let path = String::from_utf16_lossy(core::slice::from_raw_parts(buf, len));
    if crate::iat::is_title_path(&path, base) {
        LATE_SLOTS.fetch_add(crate::iat::patch_module(base, &title_rule), Ordering::AcqRel);
    }
}

unsafe fn watch_loads() -> bool {
    #[link(name = "ntdll", kind = "raw-dylib")]
    extern "system" {
        fn LdrRegisterDllNotification(flags: u32, cb: unsafe extern "system" fn(u32, *const u8, *mut c_void), ctx: *mut c_void, cookie: *mut *mut c_void) -> i32;
    }
    let mut cookie = core::ptr::null_mut();
    LdrRegisterDllNotification(0, dll_loaded, core::ptr::null_mut(), &mut cookie) >= 0
}

pub fn announce() {
    static DONE: AtomicUsize = AtomicUsize::new(0);
    if installed() && DONE.swap(1, Ordering::AcqRel) == 0 {
        let note = INSTALL_NOTE.lock().unwrap_or_else(|e| e.into_inner()).clone();
        record(M, "XMemGetWorkingSetStatistics", &note);
    }
}

pub fn log_on() -> bool {
    static ON: AtomicUsize = AtomicUsize::new(2);
    match ON.load(Ordering::Relaxed) {
        0 => false,
        1 => true,
        _ => {
            let on = env_trimmed_is(b"TOMBOX_XMEM_ACCT_LOG\0", b"1") == Some(true);
            ON.store(on as usize, Ordering::Relaxed);
            on
        }
    }
}

fn mb(b: u64) -> f64 {
    b as f64 / (1u64 << 20) as f64
}

pub fn trace(site: &str, extra: &str) {
    if !log_on() {
        return;
    }
    let p = parts();
    let old = process_memory().private_usage as u64 + p.phys;
    let ph = unsafe { GetProcessHeap() } as usize;
    let check_new = crate::heap_acct::slot(ph).map(|a| {
        let (x, o) = a.read();
        x + o
    });
    let mut s = HeapSummaryInfo { cb: core::mem::size_of::<HeapSummaryInfo>() as u32, ..Default::default() };
    let check_old = if unsafe { HeapSummary(ph as Handle, 0, &mut s) } != 0 { Some(s.committed as u64) } else { None };
    let show = |v: Option<u64>| v.map_or_else(|| "n/a".to_string(), |b| format!("{:.1}MB", mb(b)));
    record(
        M,
        "acct",
        &format!(
            "{site}: process-heap check (allocated+overhead) new={} old(whole HeapSummary)={} clamped_frees={} | title_used new={:.1}MB [virt {:.1} xmem {:.1} pool {:.1} phys {:.1} heap {:.1} gpu {:.1} in {} objects, {} clamped] old(private+phys)={:.1}MB slots={}+{} {extra}",
            show(check_new),
            show(check_old),
            crate::heap_acct::clamped_frees(),
            mb(p.total().min(crate::stats::TITLE_LIMIT)),
            mb(p.virt),
            mb(p.xmem),
            mb(p.pool),
            mb(p.phys),
            mb(p.heap),
            mb(p.gpu),
            gpu_objects().0,
            gpu_objects().1,
            mb(old.min(crate::stats::TITLE_LIMIT)),
            INSTALL_SLOTS.load(Ordering::Acquire),
            LATE_SLOTS.load(Ordering::Acquire)
        ),
    );
}

#[cfg(not(test))]
#[used]
#[link_section = ".CRT$XCU"]
static TITLE_ACCT_INIT: extern "C" fn() = title_acct_init;

#[cfg(not(test))]
extern "C" fn title_acct_init() {
    install();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valloc() -> unsafe extern "system" fn(usize, usize, u32, u32) -> usize {
        unsafe { core::mem::transmute(crate::hooks::valloc_wrapper()) }
    }

    fn vfree() -> unsafe extern "system" fn(usize, usize, u32) -> i32 {
        unsafe { core::mem::transmute(crate::hooks::vfree_wrapper()) }
    }

    #[test]
    fn rust_marks_find_our_images_only() {
        assert!(has_rust_marks(b"xx/rustc/90b35a6239c3d8bdabc530a6a0816f7ff89a0aaf/library/core/src/fmt/mod.rs"));
        assert!(has_rust_marks(b"..library\\core\\src\\panicking.rs"));
        assert!(!has_rust_marks(b"msvcp140 basic_string rust is a word"));
        assert!(!has_rust_marks(b""));
        extern "system" {
            fn GetModuleHandleW(name: *const u16) -> usize;
        }
        let exe = unsafe { GetModuleHandleW(core::ptr::null()) };
        assert!(unsafe { built_by_us(exe) });
        let k32: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        assert!(!unsafe { built_by_us(GetModuleHandleW(k32.as_ptr())) });
        assert!(host_module_name("dxcompiler") && host_module_name("dxil") && host_module_name("advapi32_sys") && host_module_name("xbsc_xs_console"));
        assert!(!host_module_name("gbxgame") && !host_module_name("xcurl") && !host_module_name("msvcp140"));
    }

    #[test]
    fn title_virtual_alloc_commit_decommit_release_move_the_counter() {
        let start = local_virt();
        let d = || local_virt() - start;
        let r = unsafe { valloc()(0, 0x10_0000, MEM_RESERVE | MEM_TOP_DOWN, PAGE_READWRITE) };
        assert_ne!(r, 0);
        assert_eq!(d(), 0);
        assert_eq!(unsafe { valloc()(r + 0x1234, 0x2000, MEM_COMMIT, PAGE_READWRITE) }, r + 0x1000);
        assert_eq!(d(), 0x3000);
        assert_eq!(unsafe { valloc()(r + 0x1000, 0x1000, MEM_COMMIT, PAGE_READWRITE) }, r + 0x1000);
        assert_eq!(d(), 0x3000);
        assert_eq!(unsafe { valloc()(r, 0x8000, MEM_COMMIT, PAGE_READWRITE) }, r);
        assert_eq!(d(), 0x8000);
        assert_eq!(unsafe { vfree()(r + 0x2000, 0x2000, MEM_DECOMMIT) }, 1);
        assert_eq!(d(), 0x6000);
        assert_eq!(unsafe { vfree()(r + 0x2000, 0x2000, MEM_DECOMMIT) }, 1);
        assert_eq!(d(), 0x6000);
        assert_eq!(unsafe { vfree()(r + 0x4000, 0x4000, MEM_DECOMMIT) }, 1);
        assert_eq!(d(), 0x2000);
        assert_eq!(unsafe { valloc()(r + 0x9_f000, 0x1000, MEM_COMMIT, PAGE_READWRITE) }, r + 0x9_f000);
        assert_eq!(d(), 0x3000);
        assert_eq!(unsafe { vfree()(r, 0, MEM_DECOMMIT) }, 1);
        assert_eq!(d(), 0);
        assert_eq!(unsafe { valloc()(r + 0x5000, 0x1000, MEM_COMMIT, PAGE_READWRITE) }, r + 0x5000);
        assert_eq!(d(), 0x1000);
        assert_eq!(unsafe { vfree()(r, 0, MEM_RELEASE) }, 1);
        assert_eq!(d(), 0);
        let n =unsafe { valloc()(0, 0x12345, MEM_RESERVE | MEM_COMMIT | MEM_TOP_DOWN, PAGE_READWRITE) };
        assert_ne!(n, 0);
        assert_eq!(d(), 0x13000);
        assert_eq!(unsafe { vfree()(n + 0x1000, 0, MEM_RELEASE) }, 0);
        assert_eq!(d(), 0x13000);
        assert_eq!(unsafe { vfree()(n, 0, MEM_RELEASE) }, 1);
        assert_eq!(d(), 0);
        assert_eq!(unsafe { valloc()(0x10, 0x1000, MEM_COMMIT, PAGE_READWRITE) }, 0);
        assert_eq!(d(), 0);
    }

    #[test]
    fn xmem_virtual_alloc_counts_page_rounded_commit() {
        let start = local_virt();
        let p = crate::virt::XMemVirtualAlloc(core::ptr::null_mut(), 0x30_0000, 0x2000_3000, 0, 0x404) as usize;
        assert_ne!(p, 0);
        assert_eq!(local_virt() - start, 0x40_0000);
        let q = unsafe { VirtualAlloc(core::ptr::null_mut(), 0x5_0000, MEM_RESERVE | MEM_TOP_DOWN, PAGE_READWRITE) } as usize;
        assert_ne!(q, 0);
        assert_eq!(local_virt() - start, 0x40_0000);
        assert_eq!(crate::virt::XMemVirtualAlloc((q + 0x1_2345) as *mut c_void, 0x10, 0x2040_1000, 0, 0x4) as usize, q + 0x1_0000);
        assert_eq!(local_virt() - start, 0x41_0000);
        assert_eq!(unsafe { vfree()(q, 0, MEM_RELEASE) }, 1);
        assert_eq!(unsafe { vfree()(p, 0, MEM_RELEASE) }, 1);
        assert_eq!(local_virt() - start, 0);
    }

    #[test]
    fn emulator_side_allocations_do_not_move_title_counters() {
        extern "system" {
            fn GetModuleHandleA(name: *const u8) -> usize;
            fn GetProcAddress(h: usize, name: *const u8) -> usize;
        }
        let k32 = unsafe { GetModuleHandleA(b"kernel32.dll\0".as_ptr()) };
        let host_alloc: unsafe extern "system" fn(Handle, u32, usize) -> *mut c_void = unsafe { core::mem::transmute(GetProcAddress(k32, b"HeapAlloc\0".as_ptr())) };
        let host_free: unsafe extern "system" fn(Handle, u32, *mut c_void) -> i32 = unsafe { core::mem::transmute(GetProcAddress(k32, b"HeapFree\0".as_ptr())) };
        let ph = unsafe { GetProcessHeap() } as usize;
        crate::heap_acct::register_title(ph, 0, 0).unwrap();
        let (v0, h0) = (local_virt(), crate::heap_acct::local_delta(ph));
        let private0 = process_memory().private_usage;
        let mut big: Vec<u8> = vec![0u8; 0x1000_0000];
        big[0x0fff_ffff] = 1;
        let heap = unsafe { HeapCreate(0, 0, 0) };
        let blk = unsafe { host_alloc(heap, 0, 0x400_0000) };
        assert!(!blk.is_null());
        unsafe { core::ptr::write_bytes(blk as *mut u8, 1, 0x400_0000) };
        let pblk = unsafe { host_alloc(GetProcessHeap(), 0, 0x200_0000) };
        unsafe { core::ptr::write_bytes(pblk as *mut u8, 1, 0x200_0000) };
        let raw = unsafe { VirtualAlloc(core::ptr::null_mut(), 0x400_0000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        unsafe { core::ptr::write_bytes(raw as *mut u8, 1, 0x400_0000) };
        let private1 = process_memory().private_usage;
        assert!(private1 >= private0 + 0x0800_0000,"the old private-bytes formula would have counted these: {private0:#x} -> {private1:#x}");
        assert_eq!(local_virt(), v0);
        assert_eq!(crate::heap_acct::local_delta(ph), h0);
        assert_eq!(big[0x0fff_ffff], 1);
        unsafe {
            VirtualFree(raw, 0, MEM_RELEASE);
            host_free(GetProcessHeap(), 0, pblk);
            host_free(heap, 0, blk);
        }
        assert_eq!(local_virt(), v0);
        assert_eq!(crate::heap_acct::local_delta(ph), h0);
    }

    #[test]
    fn parts_sum_and_title_limit_cap() {
        let p = Parts { virt: 1, xmem: 2, pool: 3, phys: 4, heap: 5, gpu: 6 };
        assert_eq!(p.total(), 21);
        let huge = Parts { virt: u64::MAX, xmem: 9, ..Default::default() };
        assert_eq!(huge.total(), u64::MAX);
        assert_eq!(huge.total().min(crate::stats::TITLE_LIMIT), crate::stats::TITLE_LIMIT);
        let near = Parts { virt: crate::stats::TITLE_LIMIT - 0x1000, heap: 0x2000, ..Default::default() };
        assert_eq!(near.total().min(crate::stats::TITLE_LIMIT), crate::stats::TITLE_LIMIT);
        let under = Parts { virt: 0x1000_0000, heap: 0x2000, ..Default::default() };
        assert_eq!(under.total().min(crate::stats::TITLE_LIMIT), 0x1000_2000);
        let used = crate::stats::title_used();
        assert!(used <= crate::stats::TITLE_LIMIT);
    }

    #[test]
    fn virtual_alloc_ex_counts_only_this_process() {
        let (ax, fx) = crate::hooks::ex_wrappers_for_test();
        let ax: unsafe extern "system" fn(usize, usize, usize, u32, u32) -> usize = unsafe { core::mem::transmute(ax) };
        let fx: unsafe extern "system" fn(usize, usize, usize, u32) -> i32 = unsafe { core::mem::transmute(fx) };
        let start = local_virt();
        let me = unsafe { GetCurrentProcess() } as usize;
        let p = unsafe { ax(me, 0, 0x2_1000, MEM_RESERVE | MEM_COMMIT | MEM_TOP_DOWN, PAGE_READWRITE) };
        assert_ne!(p, 0);
        assert_eq!(local_virt() - start, 0x2_1000);
        assert_eq!(unsafe { fx(me, p, 0x1000, MEM_DECOMMIT) }, 1);
        assert_eq!(local_virt() - start, 0x2_0000);
        assert_eq!(unsafe { fx(me, p, 0, MEM_RELEASE) }, 1);
        assert_eq!(local_virt() - start, 0);
    }

    #[test]
    fn title_module_filter_keeps_title_images_and_drops_ours() {
        extern "system" {
            fn GetModuleHandleW(name: *const u16) -> usize;
        }
        let k32: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        let plain = unsafe { GetModuleHandleW(k32.as_ptr()) };
        let wd = "c:\\windows\\";
        let dir = std::env::current_exe().unwrap().parent().unwrap().to_string_lossy().into_owned();
        let t = |p: &str| crate::iat::title_path_for_test(&p.replace("R:", &dir), plain, wd);
        assert!(t("R:\\GbxGame.exe"));
        assert!(t("R:\\XCurl.dll"));
        assert!(t("R:\\msvcp140.dll"));
        assert!(t("R:\\GameChat2.dll"));
        assert!(!t("Z:\\injected\\overlay64.dll"));
        assert!(!t("R:\\dxcompiler.dll"));
        assert!(!t("R:\\dxil.dll"));
        assert!(!t("R:\\advapi32_sys.DLL"));
        assert!(!t("R:\\xbsc_xs_console.dll"));
        assert!(!t("R:\\api-ms-win-core-memory-l1-1-0.dll"));
        assert!(!t("C:\\Windows\\System32\\ucrtbase.dll"));
        let d = |p: &str, ours: bool| crate::iat::title_decision(p, true, || ours);
        assert!(!d("r:\\xmem.dll", true));
        assert!(!d("r:\\d3d12_xs.dll", true));
        assert!(!d("r:\\xcurl.dll", true));
        assert!(d("r:\\xcurl.dll", false));
        assert!(!d("r:\\minuserx.dll", true));
        assert!(d("r:\\gbxgame.exe", false));
        let old = |p: &str| crate::iat::title_decision(p, false, || unreachable!());
        assert!(!old("r:\\xcurl.dll") && !old("r:\\xmem.dll") && old("r:\\dxcompiler.dll") && old("r:\\gbxgame.exe"));
        let exe = unsafe { GetModuleHandleW(core::ptr::null()) };
        assert!(unsafe { built_by_us(exe) }, "release builds drop any image that carries Rust core paths");
    }

    #[test]
    fn reset_and_reset_undo_leave_commit_unchanged() {
        const MEM_RESET: u32 = 0x8_0000;
        const MEM_RESET_UNDO: u32 = 0x100_0000;
        let start = local_virt();
        let d = || local_virt() - start;
        let r = unsafe { valloc()(0, 0x10_0000, MEM_RESERVE | MEM_TOP_DOWN, PAGE_READWRITE) };
        assert_ne!(r, 0);
        assert_eq!(unsafe { valloc()(r, 0x8000, MEM_COMMIT, PAGE_READWRITE) }, r);
        assert_eq!(d(), 0x8000);
        assert_eq!(unsafe { valloc()(r, 0x4000, MEM_RESET, PAGE_READWRITE) }, r);
        assert_eq!(d(), 0x8000);
        let _ = unsafe { valloc()(r, 0x4000, MEM_RESET_UNDO, PAGE_READWRITE) };
        assert_eq!(d(), 0x8000);
        let _ = unsafe { valloc()(r + 0x2_0000, 0x1000, MEM_RESET, PAGE_READWRITE) };
        assert_eq!(d(), 0x8000);
        assert_eq!(committed_private(r, r + 0x10_0000), 0x8000);
        assert_eq!(unsafe { vfree()(r, 0, MEM_RELEASE) }, 1);
        assert_eq!(d(), 0);
    }

    #[test]
    fn overlapping_and_partial_ranges_count_exact_bytes() {
        let start = local_virt();
        let d = || local_virt() - start;
        let r = unsafe { valloc()(0, 0x10_0000, MEM_RESERVE | MEM_TOP_DOWN, PAGE_READWRITE) };
        assert_ne!(r, 0);
        assert_eq!(unsafe { valloc()(r, 0x8000, MEM_COMMIT, PAGE_READWRITE) }, r);
        assert_eq!(d(), 0x8000);
        assert_eq!(unsafe { valloc()(r + 0x2000, 0x3000, MEM_COMMIT, PAGE_READWRITE) }, r + 0x2000);
        assert_eq!(d(), 0x8000);
        assert_eq!(unsafe { valloc()(r + 0x6000, 0x4000, MEM_COMMIT, PAGE_READWRITE) }, r + 0x6000);
        assert_eq!(d(), 0xa000);
        assert_eq!(unsafe { valloc()(r + 0xbfff, 0x2, MEM_COMMIT, PAGE_READWRITE) }, r + 0xb000);
        assert_eq!(d(), 0xc000);
        assert_eq!(unsafe { vfree()(r + 0x3000, 0x2000, MEM_DECOMMIT) }, 1);
        assert_eq!(d(), 0xa000);
        assert_eq!(unsafe { vfree()(r + 0x2fff, 0x1002, MEM_DECOMMIT) }, 1);
        assert_eq!(d(), 0x9000);
        assert_eq!(unsafe { vfree()(r + 0x8000, 0x8000, MEM_DECOMMIT) }, 1);
        assert_eq!(d(), 0x5000);
        assert_eq!(unsafe { vfree()(r, 0, MEM_RELEASE) }, 1);
        assert_eq!(d(), 0);
    }

    #[test]
    fn phys_pages_mapped_and_committed_count_once() {
        let _g = crate::phys::map_test_guard();
        use crate::virt::{XMemAllocatePhysicalPages, XMemFreePhysicalPages, XMemMapPhysicalPages, XMEM_64K_PAGES};
        const P: usize = crate::phys::PAGE;
        let start = local_virt();
        let d = || local_virt() - start;
        let va = crate::virt::XMemVirtualAlloc(core::ptr::null_mut(), 4 * P, MEM_RESERVE | XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize;
        assert_ne!(va, 0);
        assert_eq!(d(), 0);
        let mut n = 2usize;
        let mut pf = [0u64; 2];
        assert_eq!(unsafe { XMemAllocatePhysicalPages(XMEM_64K_PAGES as u64, &mut n, pf.as_mut_ptr()) }, 1);
        assert_eq!(crate::phys::counted_bytes_of(&pf), 2 * P as u64);
        assert_eq!(unsafe { XMemMapPhysicalPages(va as *mut c_void, 2, pf.as_mut_ptr()) } as usize, va);
        unsafe { *((va + P + 8) as *mut u32) = 5 };
        assert_eq!(d(), 0);
        assert_eq!(crate::phys::counted_bytes_of(&pf), 2 * P as u64);
        assert_eq!(crate::virt::XMemVirtualAlloc(va as *mut c_void, 2 * P, MEM_COMMIT | XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize, 0);
        assert_eq!(d(), 0);
        assert_eq!(crate::virt::XMemVirtualAlloc((va + 2 * P) as *mut c_void, 2 * P, MEM_COMMIT | XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize, va + 2 * P, "{}", crate::phys::LAST_COMMIT_FAIL.lock().unwrap().clone());
        assert_eq!(d(), 2 * P as i64);
        assert_eq!(d() as u64 + crate::phys::counted_bytes_of(&pf), 4 * P as u64);
        assert_eq!(unsafe { vfree()(va, 0, MEM_RELEASE) }, 1);
        assert_eq!(d(), 0);
        assert_eq!(crate::phys::counted_bytes_of(&pf), 2 * P as u64);
        assert_eq!(unsafe { XMemFreePhysicalPages(2, pf.as_mut_ptr()) }, 1);
        assert_eq!(crate::phys::counted_bytes_of(&pf), 0);
    }

    #[test]
    fn title_virtual_alloc2_counts_and_map_view_of_file3_does_not() {
        let _g = crate::phys::map_test_guard();
        extern "system" {
            fn CreateFileMappingW(file: usize, sa: *const c_void, protect: u32, hi: u32, lo: u32, name: *const u16) -> usize;
            fn UnmapViewOfFile(base: *const c_void) -> i32;
            fn CloseHandle(h: usize) -> i32;
        }
        const MEM_RESERVE_PLACEHOLDER: u32 = 0x4_0000;
        const MEM_REPLACE_PLACEHOLDER: u32 = 0x4000;
        const SEC_COMMIT: u32 = 0x800_0000;
        let (a2, m3) = crate::hooks::partition_wrappers_for_test();
        let a2: unsafe extern "system" fn(usize, usize, usize, u32, u32, usize, u32) -> usize = unsafe { core::mem::transmute(a2) };
        let m3: unsafe extern "system" fn(usize, usize, usize, u64, usize, u32, u32, usize, u32) -> usize = unsafe { core::mem::transmute(m3) };
        let me = unsafe { GetCurrentProcess() } as usize;
        let start = local_virt();
        let d = || local_virt() - start;
        let p = unsafe { a2(me, 0, 0x3000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE, 0, 0) };
        assert_ne!(p, 0);
        assert_eq!(d(), 0x3000);
        assert_eq!(unsafe { vfree()(p, 0x1000, MEM_DECOMMIT) }, 1);
        assert_eq!(d(), 0x2000);
        assert_eq!(unsafe { vfree()(p, 0, MEM_RELEASE) }, 1);
        assert_eq!(d(), 0);
        let sec = unsafe { CreateFileMappingW(usize::MAX, core::ptr::null(), PAGE_READWRITE | SEC_COMMIT, 0, 0x2_0000, core::ptr::null()) };
        assert_ne!(sec, 0);
        let ph = unsafe { a2(me, 0, 0x2_0000, MEM_RESERVE | MEM_RESERVE_PLACEHOLDER, PAGE_NOACCESS, 0, 0) };
        assert_ne!(ph, 0);
        assert_eq!(d(), 0);
        let v = unsafe { m3(sec, me, ph, 0, 0x2_0000, MEM_REPLACE_PLACEHOLDER, PAGE_READWRITE, 0, 0) };
        assert_eq!(v, ph);
        unsafe { *((v + 0x1_0008) as *mut u32) = 9 };
        assert_eq!(d(), 0);
        assert_eq!(committed_private(v, v + 0x2_0000), 0);
        unsafe {
            assert_ne!(UnmapViewOfFile(v as *const c_void), 0);
            CloseHandle(sec);
        }
        assert_eq!(d(), 0);
    }

    #[test]
    fn gpu_ledger_rounds_to_64k_and_release_subtracts_what_was_added() {
        assert_eq!(gpu_round(0), 0);
        assert_eq!(gpu_round(1), GPU_PAGE);
        assert_eq!(gpu_round(GPU_PAGE), GPU_PAGE);
        assert_eq!(gpu_round(GPU_PAGE + 1), 2 * GPU_PAGE);
        let (b0, (l0, c0)) = (gpu_bytes(), gpu_objects());
        assert_eq!(gpu_charge(0x30_0000, false), b0 + 0x30_0000);
        assert_eq!(gpu_charge(0x1234, false), b0 + 0x31_0000);
        assert_eq!(gpu_objects().0, l0 + 2);
        assert_eq!(parts().gpu, b0 + 0x31_0000);
        assert_eq!(gpu_charge(0x1234, true), b0 + 0x30_0000);
        assert_eq!(gpu_charge(0x30_0000, true), b0);
        assert_eq!(gpu_objects(), (l0, c0));
        assert_eq!(gpu_charge(0, false), b0);
        let before = gpu_bytes();
        gpu_charge(before + 0x10_0000, true);
        assert_eq!(gpu_bytes(), 0);
        assert_eq!(gpu_objects().1, c0 + 1);
        gpu_charge(0x2_0000, false);
        assert_eq!(gpu_bytes(), 0x2_0000);
        gpu_charge(0x2_0000, true);
        gpu_charge(before, false);
    }

    #[test]
    fn ranges_round_to_pages() {
        assert_eq!(commit_range(0x1_2345, 0x10, 0), (0x1_2000, 0x1_3000));
        assert_eq!(commit_range(0x1_2fff, 0x2, 0), (0x1_2000, 0x1_4000));
        assert_eq!(commit_range(0x40_1000, 0x10, MEM_LARGE_PAGES), (0x40_0000, 0x60_0000));
        assert_eq!(commit_range(0x5000, 0, 0), (0x5000, 0x6000));
        assert_eq!(free_range(0x7001, 0x1000), (0x7000, 0x9000));
    }
}
