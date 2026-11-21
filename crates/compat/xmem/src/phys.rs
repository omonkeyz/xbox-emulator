use core::ffi::c_void;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

use crate::sys::*;
use compat_common::record;

#[path = "phys_runs.rs"]
mod runs;

const M: &str = "xmem";
pub const PAGE: usize = 0x1_0000;

const MEM_PRIVATE: u32 = 0x2_0000;
const MEM_RESERVE_PLACEHOLDER: u32 = 0x4_0000;
const MEM_REPLACE_PLACEHOLDER: u32 = 0x4000;
const MEM_PRESERVE_PLACEHOLDER: u32 = 0x2;
const MEM_COALESCE_PLACEHOLDERS: u32 = 0x1;
const SEC_COMMIT: u32 = 0x800_0000;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MEM_DECOMMIT: u32 = 0x4000;
const ERROR_INVALID_ADDRESS: u32 = 487;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileMappingW(file: usize, attrs: *const c_void, protect: u32, hi: u32, lo: u32, name: *const u16) -> usize;
    fn CloseHandle(h: usize) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> usize;
    fn Thread32First(snap: usize, entry: *mut ThreadEntry) -> i32;
    fn Thread32Next(snap: usize, entry: *mut ThreadEntry) -> i32;
    fn OpenThread(access: u32, inherit: i32, tid: u32) -> usize;
    fn SuspendThread(h: usize) -> u32;
    fn ResumeThread(h: usize) -> u32;
    fn GetThreadContext(h: usize, ctx: *mut u8) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcessId() -> u32;
}

#[repr(C)]
#[derive(Default)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    tid: u32,
    pid: u32,
    base_pri: i32,
    delta_pri: i32,
    flags: u32,
}

#[repr(C, align(16))]
struct Context([u8; 0x4d0]);

struct Frozen {
    handles: Vec<usize>,
    suspended: Vec<bool>,
    _ctx: Box<Context>,
}

impl Frozen {
    fn new() -> Self {
        let mut ids = Vec::with_capacity(256);
        unsafe {
            let snap = CreateToolhelp32Snapshot(4, 0);
            if snap != usize::MAX {
                let (me, pid) = (GetCurrentThreadId(), GetCurrentProcessId());
                let mut e = ThreadEntry { size: core::mem::size_of::<ThreadEntry>() as u32, ..Default::default() };
                let mut ok = Thread32First(snap, &mut e);
                while ok != 0 {
                    if e.pid == pid && e.tid != me {
                        ids.push(e.tid);
                    }
                    ok = Thread32Next(snap, &mut e);
                }
                CloseHandle(snap);
            }
            let handles: Vec<usize> = ids.iter().map(|&t| OpenThread(0x2 | 0x8, 0, t)).filter(|&h| h != 0).collect();
            let mut f = Frozen { suspended: vec![false; handles.len()], handles, _ctx: Box::new(Context([0u8; 0x4d0])) };
            let ctx = f._ctx.0.as_mut_ptr();
            for i in 0..f.handles.len() {
                if SuspendThread(f.handles[i]) != u32::MAX {
                    f.suspended[i] = true;
                    core::ptr::write_unaligned(ctx.add(0x30) as *mut u32, 0x10_0001);
                    GetThreadContext(f.handles[i], ctx);
                }
            }
            f
        }
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        for i in 0..self.handles.len() {
            unsafe {
                if self.suspended[i] {
                    ResumeThread(self.handles[i]);
                }
                CloseHandle(self.handles[i]);
            }
        }
    }
}

#[link(name = "kernelbase", kind = "raw-dylib")]
extern "system" {
    fn VirtualAlloc2(process: Handle, base: *mut c_void, size: usize, kind: u32, protect: u32, params: *mut c_void, count: u32) -> *mut c_void;
    fn MapViewOfFile3(file: usize, process: Handle, base: *mut c_void, offset: u64, size: usize, kind: u32, protect: u32, params: *mut c_void, count: u32) -> *mut c_void;
    fn UnmapViewOfFile2(process: Handle, base: *mut c_void, flags: u32) -> i32;
}

struct Section {
    handle: usize,
    live: usize,
}

struct Region {
    size: usize,
    protect: u32,
    split: bool,
}

struct PhysicalPage {
    // Cleared when the title releases the PFN. Existing views still own the page.
    backing: Option<(u64, u64)>,
    views: usize,
}

struct State {
    next_pfn: u64,
    next_section: u64,
    sections: HashMap<u64, Section>,
    pfns: HashMap<u64, PhysicalPage>,
    views: BTreeMap<usize, u64>,
    private: BTreeSet<usize>,
    regions: BTreeMap<usize, Region>,
}

impl State {
    fn backing(&self, pfn: u64) -> Option<(u64, u64)> {
        self.pfns.get(&pfn)?.backing
    }

    fn add_view(&mut self, va: usize, pfn: u64) {
        self.views.insert(va, pfn);
        self.pfns.get_mut(&pfn).unwrap().views += 1;
    }

    fn remove_view(&mut self, va: usize) {
        if let Some(pfn) = self.views.remove(&va) {
            let page = self.pfns.get_mut(&pfn).unwrap();
            page.views -= 1;
            if page.views == 0 && page.backing.is_none() {
                self.pfns.remove(&pfn);
            }
        }
    }
}

static S: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = S.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(|| State {
        next_pfn: 0x1000,
        next_section: 1,
        sections: HashMap::new(),
        pfns: HashMap::new(),
        views: BTreeMap::new(),
        private: BTreeSet::new(),
        regions: BTreeMap::new(),
    });
    f(s)
}

pub fn serialized<R>(f: impl FnOnce() -> R) -> R {
    with(|_| f())
}

pub fn live_bytes() -> u64 {
    with(|s| s.pfns.len() as u64 * PAGE as u64)
}

fn query(addr: usize) -> Option<MemoryBasicInformation> {
    let mut m = MemoryBasicInformation::default();
    let n = unsafe { VirtualQueryEx(GetCurrentProcess(), addr as *const c_void, &mut m, core::mem::size_of::<MemoryBasicInformation>()) };
    if n == 0 { None } else { Some(m) }
}

pub fn allocate(n: usize) -> Option<Vec<u64>> {
    if n == 0 {
        return Some(Vec::new());
    }
    let bytes = (n as u64).checked_mul(PAGE as u64)?;
    let h = unsafe { CreateFileMappingW(usize::MAX, core::ptr::null(), PAGE_EXECUTE_READWRITE | SEC_COMMIT, (bytes >> 32) as u32, bytes as u32, core::ptr::null()) };
    if h == 0 {
        return None;
    }
    Some(with(|s| {
        let sid = s.next_section;
        s.next_section += 1;
        s.sections.insert(sid, Section { handle: h, live: n });
        let base = s.next_pfn;
        s.next_pfn += n as u64;
        (0..n as u64).map(|i| {
            s.pfns.insert(base + i, PhysicalPage { backing: Some((sid, i * PAGE as u64)), views: 0 });
            base + i
        }).collect()
    }))
}

pub fn free(pfns: &[u64]) -> bool {
    with(|s| {
        let mut seen = std::collections::HashSet::with_capacity(pfns.len());
        if pfns.iter().any(|p| s.backing(*p).is_none() || !seen.insert(*p)) {
            return false;
        }
        for p in pfns {
            let page = s.pfns.get_mut(p).unwrap();
            if let Some((sid, _)) = page.backing.take() {
                if page.views == 0 {
                    s.pfns.remove(p);
                }
                let gone = match s.sections.get_mut(&sid) {
                    Some(sec) => {
                        sec.live -= 1;
                        sec.live == 0
                    }
                    None => false,
                };
                if gone {
                    if let Some(sec) = s.sections.remove(&sid) {
                        unsafe { CloseHandle(sec.handle) };
                    }
                }
            }
        }
        true
    })
}

fn region_of(s: &State, va: usize) -> Option<usize> {
    s.regions.range(..=va).next_back().filter(|(b, r)| va < **b + r.size).map(|(b, _)| *b)
}

unsafe fn split(base: usize, size: usize) -> bool {
    let mut p = base;
    while p + PAGE < base + size {
        if VirtualFree(p as *mut c_void, PAGE, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER) == 0 {
            return false;
        }
        p += PAGE;
    }
    true
}

unsafe fn coalesce(base: usize, size: usize) {
    VirtualFree(base as *mut c_void, size, MEM_RELEASE | MEM_COALESCE_PLACEHOLDERS);
}

unsafe fn adopt(s: &mut State, va: usize) -> Option<usize> {
    if let Some(b) = region_of(s, va) {
        return Some(b);
    }
    let m = query(va)?;
    if m.state != MEM_RESERVE || m.kind != MEM_PRIVATE {
        return None;
    }
    let base = m.alloc_base;
    let mut size = 0usize;
    let mut p = base;
    loop {
        let r = match query(p) {
            Some(r) if r.alloc_base == base && r.state != 0x1_0000 => r,
            _ => break,
        };
        if r.state != MEM_RESERVE {
            return None;
        }
        size += r.region_size;
        p = r.base + r.region_size;
    }
    let protect = match m.alloc_protect & 0xff {
        0 | 0x01 => PAGE_READWRITE,
        v => v,
    };
    let (got, err) = {
        let _world = Frozen::new();
        if VirtualFree(base as *mut c_void, 0, MEM_RELEASE) == 0 {
            return None;
        }
        let got = VirtualAlloc2(GetCurrentProcess(), base as *mut c_void, size, MEM_RESERVE | MEM_RESERVE_PLACEHOLDER, PAGE_NOACCESS, core::ptr::null_mut(), 0);
        let err = GetLastError();
        if got as usize != base {
            if !got.is_null() {
                VirtualFree(got, 0, MEM_RELEASE);
            }
            VirtualAlloc(base as *mut c_void, size, MEM_RESERVE, m.alloc_protect.max(PAGE_NOACCESS));
        }
        (got, err)
    };
    if got as usize != base {
        record(M, "XMemMapPhysicalPages", &format!("could not turn reservation {base:#x}+{size:#x} into a placeholder (err={err})"));
        return None;
    }
    s.regions.insert(base, Region { size, protect, split: false });
    crate::hooks::install_vfree_hook();
    Some(base)
}

fn idle(s: &State, base: usize, size: usize) -> bool {
    !s.views.range(base..base + size).any(|_| true) && !s.private.range(base..base + size).any(|_| true)
}

unsafe fn ensure_split(s: &mut State, base: usize) -> bool {
    let (size, was) = (s.regions[&base].size, s.regions[&base].split);
    if was {
        return true;
    }
    if !split(base, size) {
        return false;
    }
    if let Some(r) = s.regions.get_mut(&base) {
        r.split = true;
    }
    true
}

unsafe fn to_placeholder(s: &mut State, piece: usize) -> bool {
    if s.private.remove(&piece) {
        let len = region_of(s, piece).map(|b| PAGE.min(b + s.regions[&b].size - piece)).unwrap_or(PAGE);
        VirtualFree(piece as *mut c_void, len, MEM_DECOMMIT);
        if VirtualFree(piece as *mut c_void, len, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER) == 0 {
            record(M, "XMemMapPhysicalPages", &format!("piece {piece:#x} back to placeholder failed (err={})", GetLastError()));
            return false;
        }
    }
    true
}

unsafe fn unmap_view(s: &mut State, va: usize) -> bool {
    if runs::run_range(va).is_some() && !runs::break_run(s, va) {
        return false;
    }
    if s.views.contains_key(&va) {
        if UnmapViewOfFile2(GetCurrentProcess(), va as *mut c_void, MEM_PRESERVE_PLACEHOLDER) == 0 {
            return false;
        }
        s.remove_view(va);
    }
    true
}

unsafe fn map_page(s: &mut State, va: usize, pfn: Option<u64>) -> bool {
    if try_map_page(s, va, pfn) {
        return true;
    }
    let stale = region_of(s, va).filter(|b| idle(s, *b, s.regions[b].size));
    match stale {
        Some(b) => {
            s.regions.remove(&b);
            try_map_page(s, va, pfn)
        }
        None => false,
    }
}

unsafe fn try_map_page(s: &mut State, va: usize, pfn: Option<u64>) -> bool {
    let base = match adopt(s, va) {
        Some(b) => b,
        None => return false,
    };
    if !unmap_view(s, va) {
        return false;
    }
    let (size, protect, was_split) = {
        let r = &s.regions[&base];
        (r.size, r.protect, r.split)
    };
    let pfn = match pfn {
        Some(p) => p,
        None => {
            if was_split && idle(s, base, size) {
                coalesce(base, size);
                if let Some(r) = s.regions.get_mut(&base) {
                    r.split = false;
                }
            }
            return true;
        }
    };
    let (sid, off) = match s.backing(pfn) {
        Some(v) => v,
        None => {
            SetLastError(ERROR_INVALID_PARAMETER);
            return false;
        }
    };
    if !was_split && !ensure_split(s, base) {
        return false;
    }
    if !to_placeholder(s, va) {
        return false;
    }
    let h = s.sections[&sid].handle;
    let p = MapViewOfFile3(h, GetCurrentProcess(), va as *mut c_void, off, PAGE, MEM_REPLACE_PLACEHOLDER, protect, core::ptr::null_mut(), 0);
    if p as usize != va {
        return false;
    }
    s.add_view(va, pfn);
    true
}

pub fn map(pages: &[(usize, Option<u64>)]) -> bool {
    if let (Some(lo), Some(hi)) = (pages.iter().map(|p| p.0 & !(PAGE - 1)).min(), pages.iter().map(|p| p.0 & !(PAGE - 1)).max()) {
        crate::epoch::bump_range(lo, hi + PAGE);
    }
    let ok = with(|s| {
        let mut ok = true;
        unsafe { runs::release_covered(s, pages) };
        for (i, j) in runs::groups(s, pages) {
            if j - i > 1 && unsafe { runs::try_map_run(s, &pages[i..j]) } {
                continue;
            }
            for &(va, pfn) in &pages[i..j] {
                if !unsafe { map_page(s, va & !(PAGE - 1), pfn) } {
                    ok = false;
                }
            }
        }
        ok
    });
    if let (Some(lo), Some(hi)) = (pages.iter().map(|p| p.0 & !(PAGE - 1)).min(), pages.iter().map(|p| p.0 & !(PAGE - 1)).max()) {
        crate::epoch::bump_range(lo, hi + PAGE);
    }
    for &(va, pfn) in pages {
        if pfn.is_some() {
            crate::write_note::committed(va & !(PAGE - 1), PAGE);
        }
    }
    ok
}

pub fn release_region(addr: usize, size: usize) -> Option<i32> {
    with(|s| {
        let r = s.regions.get(&addr)?;
        if size != 0 {
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            return Some(0);
        }
        let total = r.size;
        unsafe { runs::release_in(s, addr, addr + total) };
        let views: Vec<usize> = s.views.range(addr..addr + total).map(|(v, _)| *v).collect();
        for v in views {
            unsafe { unmap_view(s, v) };
        }
        let pieces: Vec<usize> = s.private.range(addr..addr + total).copied().collect();
        for p in pieces {
            unsafe { to_placeholder(s, p) };
        }
        unsafe {
            coalesce(addr, total);
            s.regions.remove(&addr);
            Some(VirtualFree(addr as *mut c_void, 0, MEM_RELEASE))
        }
    })
}


fn pieces(base: usize, size: usize, addr: usize, len: usize) -> Vec<(usize, usize, usize)> {
    let end = (addr + len).min(base + size);
    let mut out = Vec::new();
    let mut p = (addr.max(base) - base) / PAGE * PAGE + base;
    while p < end {
        let piece_end = (p + PAGE).min(base + size);
        let lo = addr.max(p);
        let hi = end.min(piece_end);
        out.push((p, lo, hi - lo));
        p = piece_end;
    }
    out
}

pub fn commit_owned(addr: usize, size: usize, protect: u32) -> Option<usize> {
    with(|s| {
        let base = region_of(s, addr)?;
        let (rsize, rprot) = (s.regions[&base].size, s.regions[&base].protect);
        let lo = addr & !0xfff;
        let len = ((addr + size.max(1) + 0xfff) & !0xfff) - lo;
        if lo + len > base + rsize {
            unsafe { SetLastError(ERROR_INVALID_ADDRESS) };
            return Some(0);
        }
        let ps = pieces(base, rsize, lo, len);
        if ps.iter().any(|(p, _, _)| s.views.contains_key(p)) {
            unsafe { SetLastError(ERROR_INVALID_ADDRESS) };
            return Some(0);
        }
        unsafe {
            if !ensure_split(s, base) {
                return Some(0);
            }
            for (p, a, n) in ps {
                if !s.private.contains(&p) {
                    let plen = PAGE.min(base + rsize - p);
                    let got = VirtualAlloc2(GetCurrentProcess(), p as *mut c_void, plen, MEM_RESERVE | MEM_REPLACE_PLACEHOLDER, rprot, core::ptr::null_mut(), 0);
                    if got as usize != p {
                        return Some(0);
                    }
                    s.private.insert(p);
                }
                if VirtualAlloc(a as *mut c_void, n, MEM_COMMIT, protect).is_null() {
                    return Some(0);
                }
            }
        }
        Some(lo)
    })
}

pub fn decommit_owned(addr: usize, size: usize) -> Option<i32> {
    with(|s| {
        let base = region_of(s, addr)?;
        let rsize = s.regions[&base].size;
        let len = if size == 0 { base + rsize - addr } else { size };
        let lo = addr & !0xfff;
        let hi = (addr + len + 0xfff) & !0xfff;
        for (p, a, n) in pieces(base, rsize, lo, hi - lo) {
            if s.private.contains(&p) && unsafe { VirtualFree(a as *mut c_void, n, MEM_DECOMMIT) } == 0 {
                return Some(0);
            }
        }
        Some(1)
    })
}

pub fn region_extent(addr: usize) -> Option<usize> {
    with(|s| s.regions.get(&addr).map(|r| r.size))
}

pub fn owns(addr: usize) -> bool {
    with(|s| region_of(s, addr).is_some())
}

#[cfg(test)]
mod accounting_tests {
    use super::*;

    #[test]
    fn released_pages_remain_charged_until_their_last_view_is_unmapped() {
        // Other xmem tests allocate from the process-wide physical page pool.
        const CHILD: &str = "TOMBOX_TEST_XMEM_PAGE_ACCOUNTING_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "phys::accounting_tests::released_pages_remain_charged_until_their_last_view_is_unmapped"])
                .env(CHILD, "1").status().unwrap();
            assert!(status.success());
            return;
        }
        let base = unsafe { VirtualAlloc(core::ptr::null_mut(), PAGE * 4, MEM_RESERVE, PAGE_READWRITE) } as usize;
        assert_ne!(base, 0);
        let pages = allocate(2).unwrap();
        let pair = 2 * PAGE as u64;
        assert_eq!(live_bytes(), pair);
        assert!(map(&[(base, Some(pages[0])), (base + PAGE, Some(pages[1])),
                      (base + 3 * PAGE, Some(pages[0]))]));
        assert_eq!(live_bytes(), pair);
        assert!(free(&pages));
        assert_eq!(live_bytes(), pair);
        assert!(!free(&pages));
        assert!(map(&[(base, None), (base + PAGE, None)]));
        assert_eq!(live_bytes(), PAGE as u64);
        assert_eq!(release_region(base, 0), Some(1));
        assert_eq!(live_bytes(), 0);
    }
}
