use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::iat;
use crate::sys::*;
use compat_common::record;

const M: &str = "xmem";

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleA(name: *const u8) -> usize;
    fn GetProcAddress(h: usize, name: *const u8) -> usize;
    fn TlsAlloc() -> u32;
    fn TlsGetValue(index: u32) -> usize;
    fn TlsSetValue(index: u32, value: usize) -> i32;
    fn GetProcessId(process: usize) -> u32;
    fn GetCurrentProcessId() -> u32;
}

const TLS_OUT_OF_INDEXES: u32 = 0xffff_ffff;
const SLOTS: usize = 4;
pub(crate) const MEM_SLOTS: usize = 8;
pub(crate) const WIN32_DLLS: &[&[u8]] = &[b"kernel32.dll\0", b"kernelbase.dll\0"];
pub(crate) const NTDLL_DLLS: &[&[u8]] = &[b"ntdll.dll\0"];

static TLS: AtomicU32 = AtomicU32::new(TLS_OUT_OF_INDEXES);

fn tls_index() -> u32 {
    let i = TLS.load(Ordering::Acquire);
    if i != TLS_OUT_OF_INDEXES {
        return i;
    }
    let n = unsafe { TlsAlloc() };
    match TLS.compare_exchange(TLS_OUT_OF_INDEXES, n, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => n,
        Err(prev) => prev,
    }
}

unsafe fn guarded(f: impl FnOnce()) {
    let err = GetLastError();
    let i = tls_index();
    if i != TLS_OUT_OF_INDEXES && TlsGetValue(i) == 0 {
        TlsSetValue(i, 1);
        f();
        TlsSetValue(i, 0);
    }
    SetLastError(err);
}

pub static TRACKING_HOOKS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
pub static REALLOC_REPORTS_FREE: AtomicU32 = AtomicU32::new(0);

static ALLOC_ORIG: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static REALLOC_ORIG: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static FREE_ORIG: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static SIZE_ORIG: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static VALIDATE_ORIG: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static WALK_ORIG: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];

type AllocFn = unsafe extern "system" fn(usize, u32, usize) -> usize;
type ReallocFn = unsafe extern "system" fn(usize, u32, usize, usize) -> usize;
type FreeFn = unsafe extern "system" fn(usize, u32, usize) -> usize;
type AllocHook = unsafe extern "system" fn(usize, u32, usize, usize);
type ReallocHook = unsafe extern "system" fn(usize, u32, usize, usize, usize);
type FreeHook = unsafe extern "system" fn(usize, u32, usize);

unsafe fn report_free(heap: usize, flags: u32, ptr: usize) {
    let h = TRACKING_HOOKS[2].load(Ordering::Acquire);
    if h != 0 {
        guarded(|| core::mem::transmute::<usize, FreeHook>(h)(heap, flags, ptr));
    }
}

unsafe extern "system" fn alloc_w<const I: usize>(heap: usize, flags: u32, size: usize) -> usize {
    let ww = if crate::ww_heap::wants(heap, size) { crate::ww_heap::alloc(heap, size) } else { None };
    let r = match ww {
        Some(p) => p,
        None => core::mem::transmute::<usize, AllocFn>(ALLOC_ORIG[I].load(Ordering::Acquire))(heap, flags, size),
    };
    if r != 0 {
        if let Some(a) = crate::heap_acct::slot(heap) {
            a.add(size);
        }
    }
    let h = TRACKING_HOOKS[0].load(Ordering::Acquire);
    if h != 0 {
        guarded(|| core::mem::transmute::<usize, AllocHook>(h)(heap, flags, size, r));
    }
    r
}

pub(crate) unsafe fn ww_realloc(heap: usize, flags: u32, old: usize, size: usize) -> Option<usize> {
    use crate::ww_heap::{Resize, HEAP_GENERATE_EXCEPTIONS, HEAP_REALLOC_IN_PLACE_ONLY, HEAP_ZERO_MEMORY};
    let acct = crate::heap_acct::slot(heap);
    if let Some(b) = crate::ww_heap::lookup(old) {
        let plain = flags & !(HEAP_REALLOC_IN_PLACE_ONLY | HEAP_GENERATE_EXCEPTIONS);
        let r = match crate::ww_heap::resize(old, size, flags & HEAP_ZERO_MEMORY != 0, flags & HEAP_REALLOC_IN_PLACE_ONLY != 0, |m| HeapAlloc(heap as Handle, plain, m) as usize) {
            Resize::InPlace(q) | Resize::Moved(q) => q,
            Resize::Failed => 0,
        };
        if let (Some(a), true) = (acct, r != 0) {
            a.sub(b.size);
            a.add(size);
        }
        return Some(r);
    }
    if old == 0 || flags & HEAP_REALLOC_IN_PLACE_ONLY != 0 || !crate::ww_heap::wants(heap, size) {
        return None;
    }
    let o = crate::heap_acct::block_size(heap, flags, old)?;
    let q = crate::ww_heap::migrate(heap, old, o, size)?;
    HeapFree(heap as Handle, 0, old as *mut core::ffi::c_void);
    if let Some(a) = acct {
        a.sub(o);
        a.add(size);
    }
    Some(q)
}

unsafe extern "system" fn realloc_w<const I: usize>(heap: usize, flags: u32, old: usize, size: usize) -> usize {
    if REALLOC_REPORTS_FREE.load(Ordering::Acquire) != 0 {
        report_free(heap, flags, old);
    }
    if let Some(r) = ww_realloc(heap, flags, old, size) {
        let h = TRACKING_HOOKS[1].load(Ordering::Acquire);
        if h != 0 {
            guarded(|| core::mem::transmute::<usize, ReallocHook>(h)(heap, flags, size, old, r));
        }
        return r;
    }
    let acct = crate::heap_acct::slot(heap);
    let old_size = if acct.is_some() { crate::heap_acct::block_size(heap, flags, old) } else { None };
    let r = core::mem::transmute::<usize, ReallocFn>(REALLOC_ORIG[I].load(Ordering::Acquire))(heap, flags, old, size);
    if let (Some(a), true) = (acct, r != 0) {
        if let Some(o) = old_size {
            a.sub(o);
        }
        a.add(size);
    }
    let h = TRACKING_HOOKS[1].load(Ordering::Acquire);
    if h != 0 {
        guarded(|| core::mem::transmute::<usize, ReallocHook>(h)(heap, flags, size, old, r));
    }
    r
}

unsafe extern "system" fn free_w<const I: usize>(heap: usize, flags: u32, ptr: usize) -> usize {
    if let Some(s) = crate::ww_heap::free(ptr) {
        if let Some(a) = crate::heap_acct::slot(heap) {
            a.sub(s);
        }
        report_free(heap, flags, ptr);
        return 1;
    }
    let acct = crate::heap_acct::slot(heap);
    let old_size = if acct.is_some() { crate::heap_acct::block_size(heap, flags, ptr) } else { None };
    let r = core::mem::transmute::<usize, FreeFn>(FREE_ORIG[I].load(Ordering::Acquire))(heap, flags, ptr);
    if let (Some(a), Some(o), true) = (acct, old_size, r != 0) {
        a.sub(o);
    }
    report_free(heap, flags, ptr);
    r
}

unsafe extern "system" fn size_w<const I: usize>(heap: usize, flags: u32, ptr: usize) -> usize {
    if let Some(s) = crate::ww_heap::size_of(ptr) {
        return s;
    }
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, u32, usize) -> usize>(SIZE_ORIG[I].load(Ordering::Acquire))(heap, flags, ptr)
}

unsafe extern "system" fn validate_w<const I: usize>(heap: usize, flags: u32, ptr: usize) -> i32 {
    if crate::ww_heap::is_block(ptr) {
        return 1;
    }
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, u32, usize) -> i32>(VALIDATE_ORIG[I].load(Ordering::Acquire))(heap, flags, ptr)
}

unsafe extern "system" fn walk_w<const I: usize>(heap: usize, entry: *mut crate::ww_heap::HeapEntry) -> i32 {
    crate::ww_heap::walk(heap, entry, || core::mem::transmute::<usize, unsafe extern "system" fn(usize, *mut crate::ww_heap::HeapEntry) -> i32>(WALK_ORIG[I].load(Ordering::Acquire))(heap, entry))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Alloc,
    Realloc,
    Free,
    Size,
    Validate,
    Walk,
}

const KINDS: [Kind; 6] = [Kind::Alloc, Kind::Realloc, Kind::Free, Kind::Size, Kind::Validate, Kind::Walk];

fn wrappers(k: Kind) -> [usize; SLOTS] {
    match k {
        Kind::Alloc => [alloc_w::<0> as *const () as usize, alloc_w::<1> as *const () as usize, alloc_w::<2> as *const () as usize, alloc_w::<3> as *const () as usize],
        Kind::Realloc => [realloc_w::<0> as *const () as usize, realloc_w::<1> as *const () as usize, realloc_w::<2> as *const () as usize, realloc_w::<3> as *const () as usize],
        Kind::Free => [free_w::<0> as *const () as usize, free_w::<1> as *const () as usize, free_w::<2> as *const () as usize, free_w::<3> as *const () as usize],
        Kind::Size => [size_w::<0> as *const () as usize, size_w::<1> as *const () as usize, size_w::<2> as *const () as usize, size_w::<3> as *const () as usize],
        Kind::Validate => [validate_w::<0> as *const () as usize, validate_w::<1> as *const () as usize, validate_w::<2> as *const () as usize, validate_w::<3> as *const () as usize],
        Kind::Walk => [walk_w::<0> as *const () as usize, walk_w::<1> as *const () as usize, walk_w::<2> as *const () as usize, walk_w::<3> as *const () as usize],
    }
}

fn origs(k: Kind) -> &'static [AtomicUsize; SLOTS] {
    match k {
        Kind::Alloc => &ALLOC_ORIG,
        Kind::Realloc => &REALLOC_ORIG,
        Kind::Free => &FREE_ORIG,
        Kind::Size => &SIZE_ORIG,
        Kind::Validate => &VALIDATE_ORIG,
        Kind::Walk => &WALK_ORIG,
    }
}

fn claim(table: &[AtomicUsize; SLOTS], value: usize) -> Option<usize> {
    for (i, s) in table.iter().enumerate() {
        match s.compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(i),
            Err(v) if v == value => return Some(i),
            Err(_) => {}
        }
    }
    None
}

fn heap_kind_by_name(n: &[u8]) -> Option<Kind> {
    match n {
        b"HeapAlloc" | b"RtlAllocateHeap" => Some(Kind::Alloc),
        b"HeapReAlloc" | b"RtlReAllocateHeap" => Some(Kind::Realloc),
        b"HeapFree" | b"RtlFreeHeap" => Some(Kind::Free),
        b"HeapSize" | b"RtlSizeHeap" => Some(Kind::Size),
        b"HeapValidate" | b"RtlValidateHeap" => Some(Kind::Validate),
        b"HeapWalk" => Some(Kind::Walk),
        _ => None,
    }
}

fn resolve(dlls: &[&[u8]], names: &[&[u8]]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for d in dlls {
        let h = unsafe { GetModuleHandleA(d.as_ptr()) };
        if h == 0 {
            continue;
        }
        for (i, n) in names.iter().enumerate() {
            let f = unsafe { GetProcAddress(h, n.as_ptr()) };
            if f != 0 {
                out.push((f, i));
            }
        }
    }
    out
}

fn heap_known() -> &'static Vec<(usize, Kind)> {
    static K: OnceLock<Vec<(usize, Kind)>> = OnceLock::new();
    K.get_or_init(|| {
        let names: [&[u8]; 11] = [b"HeapAlloc\0", b"RtlAllocateHeap\0", b"HeapReAlloc\0", b"RtlReAllocateHeap\0", b"HeapFree\0", b"RtlFreeHeap\0", b"HeapSize\0", b"RtlSizeHeap\0", b"HeapValidate\0", b"RtlValidateHeap\0", b"HeapWalk\0"];
        let kinds = [Kind::Alloc, Kind::Alloc, Kind::Realloc, Kind::Realloc, Kind::Free, Kind::Free, Kind::Size, Kind::Size, Kind::Validate, Kind::Validate, Kind::Walk];
        resolve(&[b"ntdll.dll\0", b"kernel32.dll\0", b"kernelbase.dll\0"], &names).into_iter().map(|(f, i)| (f, kinds[i])).collect()
    })
}

pub(crate) fn is_ours(v: usize) -> bool {
    KINDS.iter().any(|&k| wrappers(k).contains(&v))
        || winhttp_wrappers().contains(&v)
        || vfree_wrappers().contains(&v)
        || valloc_wrappers().contains(&v)
        || valloc2_wrappers().contains(&v)
        || map3_wrappers().contains(&v)
        || vallocex_wrappers().contains(&v)
        || vfreeex_wrappers().contains(&v)
        || crate::range_hooks::wrappers().contains(&v)
        || crate::crt_hooks::wrappers().contains(&v)
}

pub fn heap_rule(_dll: &[u8], name: Option<&[u8]>, value: usize) -> Option<usize> {
    if is_ours(value) {
        return None;
    }
    let k = name.and_then(heap_kind_by_name).or_else(|| heap_known().iter().find(|(f, _)| *f == value).map(|(_, k)| *k))?;
    let i = claim_or_warn(origs(k), value, k, _dll, name)?;
    Some(wrappers(k)[i])
}

static UNWRAPPED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

fn slots_full_message(k: Kind, dll: &[u8], name: Option<&[u8]>, value: usize) -> String {
    let dll = String::from_utf8_lossy(dll.strip_suffix(b"\0").unwrap_or(dll)).to_string();
    let name = name.map(|n| String::from_utf8_lossy(n).to_string()).unwrap_or_else(|| "<by ordinal>".into());
    format!("WARNING: all {SLOTS} {k:?} wrapper slots are taken; import {dll}!{name} -> {value:#x} stays UNWRAPPED and its heap traffic is not tracked")
}

fn claim_or_warn(table: &[AtomicUsize; SLOTS], value: usize, k: Kind, dll: &[u8], name: Option<&[u8]>) -> Option<usize> {
    let r = claim(table, value);
    if r.is_none() {
        let first = UNWRAPPED.lock().map(|mut v| if v.contains(&value) { false } else { v.push(value); true }).unwrap_or(true);
        if first {
            record(M, "XMemSetWin32HeapTrackingHooks", &slots_full_message(k, dll, name, value));
        }
    }
    r
}

pub fn install_heap_accounting() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| iat::patch_all(&heap_rule))
}

pub fn install_heap_hooks() {
    let n = iat::patch_all(&heap_rule);
    record(M, "XMemSetWin32HeapTrackingHooks", &format!("heap imports redirected through tracking wrappers: {n} slots"));
}

pub const WINHTTP_NAMES: [&[u8]; 7] = [
    b"WinHttpOpen",
    b"WinHttpConnect",
    b"WinHttpOpenRequest",
    b"WinHttpWebSocketCompleteUpgrade",
    b"WinHttpCloseHandle",
    b"WinHttpReceiveResponse",
    b"WinHttpSetOption",
];

pub static WINHTTP_HOOKS: [AtomicUsize; 7] = [const { AtomicUsize::new(0) }; 7];
static WINHTTP_ORIG: [AtomicUsize; 7] = [const { AtomicUsize::new(0) }; 7];

unsafe fn pick(i: usize) -> usize {
    let h = WINHTTP_HOOKS[i].load(Ordering::Acquire);
    if h != 0 { h } else { WINHTTP_ORIG[i].load(Ordering::Acquire) }
}

unsafe extern "system" fn wh_open(a: usize, b: u32, c: usize, d: usize, e: u32) -> usize {
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, u32, usize, usize, u32) -> usize>(pick(0))(a, b, c, d, e)
}

unsafe extern "system" fn wh_connect(a: usize, b: usize, c: u16, d: u32) -> usize {
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, u16, u32) -> usize>(pick(1))(a, b, c, d)
}

unsafe extern "system" fn wh_open_request(a: usize, b: usize, c: usize, d: usize, e: usize, f: usize, g: u32) -> usize {
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, usize, usize, usize, usize, u32) -> usize>(pick(2))(a, b, c, d, e, f, g)
}

unsafe extern "system" fn wh_ws_upgrade(a: usize, b: usize) -> usize {
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize) -> usize>(pick(3))(a, b)
}

unsafe extern "system" fn wh_close(a: usize) -> usize {
    core::mem::transmute::<usize, unsafe extern "system" fn(usize) -> usize>(pick(4))(a)
}

unsafe extern "system" fn wh_receive(a: usize, b: usize) -> usize {
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize) -> usize>(pick(5))(a, b)
}

unsafe extern "system" fn wh_set_option(a: usize, b: u32, c: usize, d: u32) -> usize {
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, u32, usize, u32) -> usize>(pick(6))(a, b, c, d)
}

fn winhttp_wrappers() -> [usize; 7] {
    [
        wh_open as *const () as usize,
        wh_connect as *const () as usize,
        wh_open_request as *const () as usize,
        wh_ws_upgrade as *const () as usize,
        wh_close as *const () as usize,
        wh_receive as *const () as usize,
        wh_set_option as *const () as usize,
    ]
}

pub fn winhttp_rule(dll: &[u8], name: Option<&[u8]>, value: usize) -> Option<usize> {
    if is_ours(value) {
        return None;
    }
    let i = match name {
        Some(n) if dll.to_ascii_lowercase().starts_with(b"winhttp") => WINHTTP_NAMES.iter().position(|w| *w == n)?,
        _ => {
            let names: Vec<Vec<u8>> = WINHTTP_NAMES.iter().map(|n| [*n, b"\0"].concat()).collect();
            let refs: Vec<&[u8]> = names.iter().map(|v| v.as_slice()).collect();
            resolve(&[b"winhttp.dll\0"], &refs).into_iter().find(|(f, _)| *f == value)?.1
        }
    };
    match WINHTTP_ORIG[i].compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Some(winhttp_wrappers()[i]),
        Err(v) if v == value => Some(winhttp_wrappers()[i]),
        Err(_) => None,
    }
}

pub fn install_winhttp_hooks() {
    let n = iat::patch_all(&winhttp_rule);
    record(M, "XMemSetWinHttpHooks", &format!("WinHttp imports redirected through hook dispatch: {n} slots"));
}

macro_rules! mem_slot_array {
    ($f:ident) => {
        [
            $f::<0> as *const () as usize,
            $f::<1> as *const () as usize,
            $f::<2> as *const () as usize,
            $f::<3> as *const () as usize,
            $f::<4> as *const () as usize,
            $f::<5> as *const () as usize,
            $f::<6> as *const () as usize,
            $f::<7> as *const () as usize,
        ]
    };
}
pub(crate) use mem_slot_array;

static VFREE_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];
static VALLOC2_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];
static MAP3_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];

unsafe extern "system" fn vfree_w<const I: usize>(addr: usize, size: usize, kind: u32) -> i32 {
    let (lo, hi) = crate::epoch::bump_free(addr, size);
    let r = crate::title_acct::track_free(addr, size, kind, || vfree_inner(I, addr, size, kind));
    if r != 0 && kind & MEM_RELEASE != 0 {
        crate::write_watch::forget_range(addr, size);
    }
    crate::epoch::bump_range(lo, hi);
    r
}

unsafe fn vfree_inner(slot: usize, addr: usize, size: usize, kind: u32) -> i32 {
    if kind & MEM_RELEASE != 0 {
        if let Some(r) = crate::phys::release_region(addr, size) {
            return r;
        }
        if crate::phys::owns(addr) {
            SetLastError(487);
            return 0;
        }
    } else if kind & 0x4000 != 0 {
        if let Some(r) = crate::phys::decommit_owned(addr, size) {
            return r;
        }
    }
    let f = VFREE_ORIG[slot].load(Ordering::Acquire);
    let f = if f != 0 { f } else { VirtualFree as *const () as usize };
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, u32) -> i32>(f)(addr, size, kind)
}

static VALLOC_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];

unsafe extern "system" fn valloc_w<const I: usize>(addr: usize, size: usize, kind: u32, protect: u32) -> usize {
    crate::title_acct::track_alloc(addr, size, kind, || valloc_inner(I, addr, size, kind, protect))
}

unsafe fn valloc_inner(slot: usize, addr: usize, size: usize, kind: u32, protect: u32) -> usize {
    if addr != 0 && kind & MEM_COMMIT != 0 {
        if let Some(r) = crate::phys::commit_owned(addr, size, protect) {
            crate::write_note::committed(r, size);
            return r;
        }
    }
    if crate::ww_valloc::wants(addr, size, kind, protect) {
        let r = crate::ww_valloc::alloc(size, kind, protect);
        if r != 0 {
            if kind & MEM_COMMIT != 0 {
                crate::write_note::committed(r, size);
            }
            return r;
        }
    }
    let f = VALLOC_ORIG[slot].load(Ordering::Acquire);
    let f = if f != 0 { f } else { VirtualAlloc as *const () as usize };
    let r = core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, u32, u32) -> usize>(f)(addr, size, kind, protect);
    if r != 0 && kind & MEM_RESERVE != 0 {
        if kind & MEM_WRITE_WATCH != 0 {
            crate::write_watch::note_title(r, size);
        } else {
            crate::write_watch::invalidate_range(r, size);
        }
    }
    if r != 0 && kind & MEM_COMMIT != 0 {
        crate::write_note::committed(r, size);
    }
    r
}

pub fn valloc_wrapper() -> usize {
    vfree_rule(b"kernel32.dll", Some(b"VirtualAlloc"), VirtualAlloc as *const () as usize).unwrap_or(valloc_w::<0> as *const () as usize)
}

const PARAM_PARTITION_HANDLE: u64 = 3;

fn strip_partition(params: usize, count: u32) -> Vec<[u64; 2]> {
    let mut out = Vec::new();
    if params == 0 {
        return out;
    }
    let me = unsafe { GetCurrentProcessId() };
    for i in 0..count as usize {
        let p = unsafe { core::ptr::read_unaligned((params + i * 16) as *const [u64; 2]) };
        if p[0] & 0xff == PARAM_PARTITION_HANDLE && unsafe { GetProcessId(p[1] as usize) } == me {
            continue;
        }
        out.push(p);
    }
    out
}

pub(crate) fn is_self(process: usize) -> bool {
    process == usize::MAX || process == 0 || unsafe { GetProcessId(process) == GetCurrentProcessId() }
}

unsafe extern "system" fn valloc2_w<const I: usize>(process: usize, base: usize, size: usize, kind: u32, protect: u32, params: usize, count: u32) -> usize {
    let v = strip_partition(params, count);
    let (pp, pc) = if v.is_empty() { (0, 0) } else { (v.as_ptr() as usize, v.len() as u32) };
    let call = || core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, usize, u32, u32, usize, u32) -> usize>(VALLOC2_ORIG[I].load(Ordering::Acquire))(process, base, size, kind, protect, pp, pc);
    if is_self(process) {
        crate::title_acct::track_alloc(base, size, kind, call)
    } else {
        call()
    }
}

static VALLOCEX_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];
static VFREEEX_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];

unsafe extern "system" fn vallocex_w<const I: usize>(process: usize, addr: usize, size: usize, kind: u32, protect: u32) -> usize {
    let call = || core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, usize, u32, u32) -> usize>(VALLOCEX_ORIG[I].load(Ordering::Acquire))(process, addr, size, kind, protect);
    if is_self(process) {
        let r = crate::title_acct::track_alloc(addr, size, kind, call);
        if r != 0 && kind & MEM_RESERVE != 0 {
            crate::write_watch::invalidate_range(r, size);
        }
        r
    } else {
        call()
    }
}

unsafe extern "system" fn vfreeex_w<const I: usize>(process: usize, addr: usize, size: usize, kind: u32) -> i32 {
    let call = || core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, usize, u32) -> i32>(VFREEEX_ORIG[I].load(Ordering::Acquire))(process, addr, size, kind);
    if is_self(process) {
        let (lo, hi) = crate::epoch::bump_free(addr, size);
        let r = crate::title_acct::track_free(addr, size, kind, call);
        crate::epoch::bump_range(lo, hi);
        if r != 0 && kind & MEM_RELEASE != 0 {
            crate::write_watch::forget_range(addr, size);
        }
        r
    } else {
        call()
    }
}

fn vfree_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(vfree_w)
}

fn valloc_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(valloc_w)
}

fn valloc2_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(valloc2_w)
}

fn map3_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(map3_w)
}

fn vallocex_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(vallocex_w)
}

fn vfreeex_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(vfreeex_w)
}

pub fn vex_rule(_dll: &[u8], name: Option<&[u8]>, value: usize) -> Option<usize> {
    multi_rule(WIN32_DLLS, &[b"VirtualAllocEx"], &VALLOCEX_ORIG, &vallocex_wrappers(), name, value)
        .or_else(|| multi_rule(WIN32_DLLS, &[b"VirtualFreeEx"], &VFREEEX_ORIG, &vfreeex_wrappers(), name, value))
}

#[cfg(test)]
pub fn ex_wrappers_for_test() -> (usize, usize) {
    let a = vex_rule(b"KERNEL32.dll", Some(b"VirtualAllocEx"), VirtualAllocEx as *const () as usize).expect("VirtualAllocEx hook");
    let f = vex_rule(b"KERNEL32.dll", Some(b"VirtualFreeEx"), VirtualFreeEx as *const () as usize).expect("VirtualFreeEx hook");
    (a, f)
}

#[cfg(test)]
pub fn partition_wrappers_for_test() -> (usize, usize) {
    extern "system" {
        fn GetModuleHandleA(name: *const u8) -> usize;
        fn GetProcAddress(h: usize, name: *const u8) -> usize;
    }
    let kb = unsafe { GetModuleHandleA(b"kernelbase.dll\0".as_ptr()) };
    let a2 = partition_rule(b"api-ms-win-core-memory-l1-1-6.dll", Some(b"VirtualAlloc2"), unsafe { GetProcAddress(kb, b"VirtualAlloc2\0".as_ptr()) }).expect("VirtualAlloc2 hook");
    let m3 = partition_rule(b"api-ms-win-core-memory-l1-1-6.dll", Some(b"MapViewOfFile3"), unsafe { GetProcAddress(kb, b"MapViewOfFile3\0".as_ptr()) }).expect("MapViewOfFile3 hook");
    (a2, m3)
}

#[cfg(test)]
#[link(name = "kernel32")]
extern "system" {
    fn VirtualAllocEx(process: usize, addr: usize, size: usize, kind: u32, protect: u32) -> usize;
    fn VirtualFreeEx(process: usize, addr: usize, size: usize, kind: u32) -> i32;
}

unsafe extern "system" fn map3_w<const I: usize>(file: usize, process: usize, base: usize, offset: u64, size: usize, kind: u32, protect: u32, params: usize, count: u32) -> usize {
    let v = strip_partition(params, count);
    let (pp, pc) = if v.is_empty() { (0, 0) } else { (v.as_ptr() as usize, v.len() as u32) };
    core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, usize, u64, usize, u32, u32, usize, u32) -> usize>(MAP3_ORIG[I].load(Ordering::Acquire))(file, process, base, offset, size, kind, protect, pp, pc)
}

pub(crate) fn multi_rule<const N: usize>(dlls: &[&[u8]], names: &[&[u8]], origs: &[AtomicUsize; N], wrappers: &[usize; N], name: Option<&[u8]>, value: usize) -> Option<usize> {
    if value == 0 || is_ours(value) {
        return None;
    }
    let by_name = name.map(|n| names.contains(&n)).unwrap_or(false);
    let by_value = !by_name && {
        let z: Vec<Vec<u8>> = names.iter().map(|n| [*n, b"\0"].concat()).collect();
        let refs: Vec<&[u8]> = z.iter().map(|v| v.as_slice()).collect();
        resolve(dlls, &refs).iter().any(|(f, _)| *f == value)
    };
    if !by_name && !by_value {
        return None;
    }
    for (i, s) in origs.iter().enumerate() {
        match s.compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(wrappers[i]),
            Err(v) if v == value => return Some(wrappers[i]),
            Err(_) => {}
        }
    }
    None
}

pub fn vfree_rule(_dll: &[u8], name: Option<&[u8]>, value: usize) -> Option<usize> {
    multi_rule(WIN32_DLLS, &[b"VirtualFree"], &VFREE_ORIG, &vfree_wrappers(), name, value)
        .or_else(|| multi_rule(WIN32_DLLS, &[b"VirtualAlloc"], &VALLOC_ORIG, &valloc_wrappers(), name, value))
}

pub fn partition_rule(_dll: &[u8], name: Option<&[u8]>, value: usize) -> Option<usize> {
    multi_rule(WIN32_DLLS, &[b"VirtualAlloc2"], &VALLOC2_ORIG, &valloc2_wrappers(), name, value)
        .or_else(|| multi_rule(WIN32_DLLS, &[b"MapViewOfFile3"], &MAP3_ORIG, &map3_wrappers(), name, value))
}

pub fn install_vfree_hook() {
    static DONE: AtomicU32 = AtomicU32::new(0);
    if DONE.swap(1, Ordering::AcqRel) == 0 {
        let n = iat::patch_all(&vfree_rule);
        record(M, "XMemMapPhysicalPages", &format!("VirtualAlloc/VirtualFree imports redirected for xmem-owned physical-page regions: {n} slots"));
    }
}

pub fn install_partition_hooks() {
    static DONE: AtomicU32 = AtomicU32::new(0);
    if DONE.swap(1, Ordering::AcqRel) == 0 {
        let n = iat::patch_all(&partition_rule);
        record(M, "XMemGetPartitionHandleAndId", &format!("VirtualAlloc2/MapViewOfFile3 imports strip the stand-in partition handle: {n} slots"));
    }
}

pub fn vfree_wrapper() -> usize {
    vfree_rule(b"kernel32.dll", Some(b"VirtualFree"), VirtualFree as *const () as usize).unwrap_or(vfree_w::<0> as *const () as usize)
}

#[cfg(test)]
pub fn strip_for_test(params: &[[u64; 2]]) -> Vec<[u64; 2]> {
    strip_partition(params.as_ptr() as usize, params.len() as u32)
}

#[cfg(test)]
mod ww_heap_tests {
    use super::*;

    #[link(name = "kernel32")]
    extern "system" {
        fn HeapReAlloc(heap: Handle, flags: u32, mem: *mut core::ffi::c_void, bytes: usize) -> *mut core::ffi::c_void;
        fn HeapSize(heap: Handle, flags: u32, mem: *const core::ffi::c_void) -> usize;
        fn HeapValidate(heap: Handle, flags: u32, mem: *const core::ffi::c_void) -> i32;
        fn HeapWalk(heap: Handle, entry: *mut crate::ww_heap::HeapEntry) -> i32;
    }

    fn export(name: &[u8]) -> usize {
        let z = [name, b"\0"].concat();
        resolve(&[b"kernel32.dll\0"], &[z.as_slice()]).first().map(|r| r.0).expect("kernel32 export")
    }

    fn hooked<T: Copy>(name: &[u8]) -> T {
        let real = export(name);
        assert!(!is_ours(real), "kernel32 export resolves to a wrapper");
        let w = heap_rule(b"api-ms-win-core-heap-l1-1-0.dll", Some(name), real).expect("heap slot claims");
        unsafe { core::mem::transmute_copy::<usize, T>(&w) }
    }

    #[test]
    fn a_full_slot_table_refuses_and_names_the_unwrapped_import() {
        let t: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
        for v in 1..=SLOTS {
            assert_eq!(claim_or_warn(&t, v * 0x10, Kind::Alloc, b"kernel32.dll", Some(b"HeapAlloc")), Some(v - 1));
        }
        assert_eq!(claim_or_warn(&t, 0x10, Kind::Alloc, b"kernel32.dll", Some(b"HeapAlloc")), Some(0));
        assert_eq!(claim_or_warn(&t, 0x999, Kind::Alloc, b"kernel32.dll", Some(b"HeapAlloc")), None);
        let m = slots_full_message(Kind::Alloc, b"kernel32.dll\0", Some(b"HeapAlloc"), 0x999);
        assert!(m.contains("kernel32.dll!HeapAlloc -> 0x999") && m.contains("UNWRAPPED") && m.contains("Alloc"), "{m}");
    }

    #[test]
    fn heap_rule_refuses_its_own_wrappers_but_claims_the_real_export() {
        let real = export(b"HeapAlloc");
        let w = heap_rule(b"kernel32.dll", Some(b"HeapAlloc"), real).expect("real export claims");
        assert!(is_ours(w));
        assert_eq!(heap_rule(b"kernel32.dll", Some(b"HeapAlloc"), w), None);
        assert_eq!(heap_rule(b"kernel32.dll", Some(b"HeapAlloc"), real), Some(w));
    }

    #[test]
    fn title_heap_imports_serve_large_blocks_from_watched_memory() {
        let _g = crate::ww_heap::test_lock();
        crate::ww_heap::set_for_test(true, 0x10_0000);
        let h = unsafe { GetProcessHeap() } as usize;
        let alloc: AllocFn = hooked(b"HeapAlloc");
        let realloc: ReallocFn = hooked(b"HeapReAlloc");
        let free: FreeFn = hooked(b"HeapFree");
        let size: unsafe extern "system" fn(usize, u32, usize) -> usize = hooked(b"HeapSize");
        let validate: unsafe extern "system" fn(usize, u32, usize) -> i32 = hooked(b"HeapValidate");
        let walk: unsafe extern "system" fn(usize, *mut crate::ww_heap::HeapEntry) -> i32 = hooked(b"HeapWalk");
        unsafe {
            let p = alloc(h, 0x8, 0x10_0000);
            assert!(crate::ww_heap::is_block(p));
            assert_eq!(size(h, 0, p), 0x10_0000);
            assert_eq!(validate(h, 0, p), 1);
            assert_eq!(realloc(h, 0x10, p, 0x40_0000), 0);
            assert!(crate::ww_heap::is_block(p));
            let q = realloc(h, 0x8, p, 0x40_0000);
            assert!(crate::ww_heap::is_block(q) && !crate::ww_heap::is_block(p));
            assert!(core::slice::from_raw_parts(q as *const u8, 0x40_0000).iter().all(|&x| x == 0));
            let mut e = crate::ww_heap::HeapEntry::default();
            let mut found = false;
            let mut guard = 0;
            while walk(h, &mut e) != 0 {
                found |= e.data == q && e.cb_data == 0x40_0000;
                guard += 1;
                assert!(guard < 10_000_000);
            }
            assert!(found, "HeapWalk must report the watched block");
            assert_eq!(free(h, 0, q), 1);
            assert!(!crate::ww_heap::is_block(q));
            let s = alloc(h, 0, 0x40);
            assert!(!crate::ww_heap::is_block(s));
            let g = realloc(h, 0, s, 0x20_0000);
            assert!(crate::ww_heap::is_block(g));
            assert_eq!(free(h, 0, g), 1);
            let other = HeapCreate(0, 0, 0) as usize;
            let o = alloc(other, 0, 0x20_0000);
            assert!(!crate::ww_heap::is_block(o));
            assert_eq!(free(other, 0, o), 1);
        }
    }

    #[test]
    fn multi_rule_claims_per_target_slots_and_reuses_or_rejects_full() {
        static ORIG: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
        let wraps = [0x1111_0001usize, 0x1111_0002usize];
        assert_eq!(multi_rule(WIN32_DLLS, &[b"VirtualFree"], &ORIG, &wraps, Some(b"VirtualFree"), 0), None);
        assert_eq!(multi_rule(WIN32_DLLS, &[b"VirtualFree"], &ORIG, &wraps, Some(b"VirtualFree"), 0x2222_0001), Some(wraps[0]));
        assert_eq!(multi_rule(WIN32_DLLS, &[b"VirtualFree"], &ORIG, &wraps, Some(b"VirtualFree"), 0x2222_0002), Some(wraps[1]));
        assert_eq!(multi_rule(WIN32_DLLS, &[b"VirtualFree"], &ORIG, &wraps, Some(b"VirtualFree"), 0x2222_0001), Some(wraps[0]));
        assert_eq!(multi_rule(WIN32_DLLS, &[b"VirtualFree"], &ORIG, &wraps, Some(b"VirtualFree"), 0x2222_0002), Some(wraps[1]));
        assert_eq!(multi_rule(WIN32_DLLS, &[b"VirtualFree"], &ORIG, &wraps, Some(b"VirtualFree"), 0x2222_0003), None);
    }

    #[test]
    fn multi_target_vfree_dispatches_to_distinct_originals_and_bumps_epoch() {
        static HITS_A: AtomicUsize = AtomicUsize::new(0);
        static HITS_B: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "system" fn fake_free_a(_addr: usize, _size: usize, _kind: u32) -> i32 {
            HITS_A.fetch_add(1, Ordering::AcqRel);
            1
        }
        unsafe extern "system" fn fake_free_b(_addr: usize, _size: usize, _kind: u32) -> i32 {
            HITS_B.fetch_add(1, Ordering::AcqRel);
            0
        }
        let wa = vfree_rule(b"kernel32.dll", Some(b"VirtualFree"), fake_free_a as *const () as usize).expect("first target slot");
        let wb = vfree_rule(b"kernelbase.dll", Some(b"VirtualFree"), fake_free_b as *const () as usize).expect("second target slot");
        assert_ne!(wa, wb);
        assert!(is_ours(wa) && is_ours(wb));
        assert_eq!(vfree_rule(b"kernel32.dll", Some(b"VirtualFree"), fake_free_a as *const () as usize), Some(wa));
        assert_eq!(vfree_rule(b"kernel32.dll", Some(b"VirtualFree"), wa), None);

        let fa: unsafe extern "system" fn(usize, usize, u32) -> i32 = unsafe { core::mem::transmute(wa) };
        let fb: unsafe extern "system" fn(usize, usize, u32) -> i32 = unsafe { core::mem::transmute(wb) };
        let s0 = crate::epoch::TomboxAddressSpaceEpoch();
        assert_eq!(unsafe { fa(0x5555_1000, 0x1000, MEM_RESET) }, 1);
        assert!(crate::epoch::TomboxAddressSpaceChangedSince(s0, 0x5555_1000, 0x5555_2000) != 0);
        assert_eq!((HITS_A.load(Ordering::Acquire), HITS_B.load(Ordering::Acquire)), (1, 0));
        let s1 = crate::epoch::TomboxAddressSpaceEpoch();
        assert_eq!(unsafe { fb(0x6666_1000, 0x1000, MEM_RESET) }, 0);
        assert!(crate::epoch::TomboxAddressSpaceChangedSince(s1, 0x6666_1000, 0x6666_2000) != 0);
        assert_eq!((HITS_A.load(Ordering::Acquire), HITS_B.load(Ordering::Acquire)), (1, 1));
    }
}
