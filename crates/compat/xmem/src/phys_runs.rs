use super::*;
use core::sync::atomic::{AtomicU64, Ordering};
use std::collections::BTreeSet;

struct Run {
    hi: usize,
    sid: u64,
    off: u64,
}

struct Runs {
    runs: BTreeMap<usize, Run>,
    fragmented: BTreeSet<usize>,
}

static R: Mutex<Runs> = Mutex::new(Runs { runs: BTreeMap::new(), fragmented: BTreeSet::new() });
static MERGED: AtomicU64 = AtomicU64::new(0);
static MERGED_PAGES: AtomicU64 = AtomicU64::new(0);
static BROKEN: AtomicU64 = AtomicU64::new(0);
static BROKEN_NS: AtomicU64 = AtomicU64::new(0);
static LAST_LOG: AtomicU64 = AtomicU64::new(0);

fn enabled() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var("TOMBOX_PHYS_RUNS").map(|v| v != "0").unwrap_or(true))
}

fn runs() -> std::sync::MutexGuard<'static, Runs> {
    R.lock().unwrap_or_else(|p| p.into_inner())
}

fn run_of(r: &Runs, va: usize) -> Option<usize> {
    r.runs.range(..=va).next_back().filter(|(_, x)| va < x.hi).map(|(lo, _)| *lo)
}

pub(super) fn groups(s: &State, pages: &[(usize, Option<u64>)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let mut j = i + 1;
        if let Some(p0) = pages[i].1.and_then(|p| s.backing(p)) {
            while j < pages.len() {
                let ok = pages[j].0 == pages[j - 1].0 + PAGE
                    && pages[j].0 & (PAGE - 1) == 0
                    && matches!(pages[j].1.and_then(|p| s.backing(p)), Some((sid, off)) if sid == p0.0 && off == p0.1 + ((j - i) * PAGE) as u64);
                if !ok {
                    break;
                }
                j += 1;
            }
        }
        out.push((i, j));
        i = j;
    }
    out
}

pub(super) unsafe fn release_covered(s: &mut State, pages: &[(usize, Option<u64>)]) {
    let Some(lo) = pages.iter().map(|p| p.0 & !(PAGE - 1)).min() else { return };
    let hi = pages.iter().map(|p| p.0 & !(PAGE - 1)).max().unwrap() + PAGE;
    let mut r = runs();
    let candidates: Vec<(usize, usize)> = r.runs.range(..hi).filter(|(a, x)| x.hi > lo && **a < hi).map(|(a, x)| (*a, x.hi)).collect();
    if candidates.is_empty() {
        return;
    }
    let set: BTreeSet<usize> = pages.iter().map(|p| p.0 & !(PAGE - 1)).collect();
    for (a, b) in candidates {
        if (a..b).step_by(PAGE).all(|p| set.contains(&p)) {
            drop_run(s, &mut r, a);
        }
    }
}

unsafe fn drop_run(s: &mut State, r: &mut Runs, lo: usize) {
    let Some(run) = r.runs.remove(&lo) else { return };
    UnmapViewOfFile2(GetCurrentProcess(), lo as *mut c_void, MEM_PRESERVE_PLACEHOLDER);
    split(lo, run.hi - lo);
    let mut p = lo;
    while p < run.hi {
        s.remove_view(p);
        p += PAGE;
    }
    unref(s, run.sid);
}

fn unref(s: &mut State, sid: u64) {
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

pub(super) unsafe fn try_map_run(s: &mut State, run: &[(usize, Option<u64>)]) -> bool {
    if !enabled() || run.len() < 2 {
        return false;
    }
    if BROKEN.load(Ordering::Relaxed) >= 8 || BROKEN_NS.load(Ordering::Relaxed) >= 20_000_000 {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| record(M, "XMemMapPhysicalPages", &format!("stopped merging contiguous runs: {} partial remaps froze threads for {:.1} ms; new maps stay per page",
            BROKEN.load(Ordering::Relaxed), BROKEN_NS.load(Ordering::Relaxed) as f64 / 1e6)));
        return false;
    }
    let lo = run[0].0;
    let hi = lo + run.len() * PAGE;
    let Some((sid, off)) = run[0].1.and_then(|p| s.backing(p)) else { return false };
    let Some(base) = adopt(s, lo) else { return false };
    let (rsize, protect) = (s.regions[&base].size, s.regions[&base].protect);
    if hi > base + rsize {
        return false;
    }
    {
        let r = runs();
        if r.fragmented.contains(&base) || lo & (PAGE - 1) != 0 || run_of(&r, lo).is_some() || run_of(&r, hi - PAGE).is_some() || r.runs.range(lo..hi).next().is_some() {
            return false;
        }
    }
    if !ensure_split(s, base) {
        return false;
    }
    for p in (lo..hi).step_by(PAGE) {
        if !unmap_view(s, p) || !to_placeholder(s, p) {
            return false;
        }
    }
    if VirtualFree(lo as *mut c_void, hi - lo, MEM_RELEASE | MEM_COALESCE_PLACEHOLDERS) == 0 {
        return false;
    }
    let h = s.sections[&sid].handle;
    let p = MapViewOfFile3(h, GetCurrentProcess(), lo as *mut c_void, off, hi - lo, MEM_REPLACE_PLACEHOLDER, protect, core::ptr::null_mut(), 0);
    if p as usize != lo {
        if !p.is_null() {
            UnmapViewOfFile2(GetCurrentProcess(), p, MEM_PRESERVE_PLACEHOLDER);
        }
        split(lo, hi - lo);
        return false;
    }
    for &(va, pfn) in run {
        s.add_view(va, pfn.unwrap());
    }
    if let Some(sec) = s.sections.get_mut(&sid) {
        sec.live += 1;
    }
    runs().runs.insert(lo, Run { hi, sid, off });
    MERGED.fetch_add(1, Ordering::Relaxed);
    MERGED_PAGES.fetch_add(run.len() as u64, Ordering::Relaxed);
    maybe_log();
    true
}

pub(super) fn run_range(va: usize) -> Option<(usize, usize)> {
    let r = runs();
    run_of(&r, va).map(|lo| (lo, r.runs[&lo].hi))
}

pub(super) unsafe fn break_run(s: &mut State, va: usize) -> bool {
    let mut r = runs();
    let Some(lo) = run_of(&r, va) else { return true };
    let run = r.runs.remove(&lo).unwrap();
    if let Some(b) = region_of(s, lo) {
        r.fragmented.insert(b);
    }
    drop(r);
    let h = match s.sections.get(&run.sid) {
        Some(sec) => sec.handle,
        None => return false,
    };
    let protect = region_of(s, lo).map(|b| s.regions[&b].protect).unwrap_or(PAGE_READWRITE);
    let n = (run.hi - lo) / PAGE;
    let mut ok = true;
    let t0 = std::time::Instant::now();
    {
        let _world = Frozen::new();
        if UnmapViewOfFile2(GetCurrentProcess(), lo as *mut c_void, MEM_PRESERVE_PLACEHOLDER) == 0 {
            ok = false;
        } else if !split(lo, run.hi - lo) {
            ok = false;
        } else {
            for i in 0..n {
                let p = lo + i * PAGE;
                let got = MapViewOfFile3(h, GetCurrentProcess(), p as *mut c_void, run.off + (i * PAGE) as u64, PAGE, MEM_REPLACE_PLACEHOLDER, protect, core::ptr::null_mut(), 0);
                if got as usize != p {
                    ok = false;
                }
            }
        }
    }
    let dt = t0.elapsed().as_nanos() as u64;
    unref(s, run.sid);
    let k = BROKEN.fetch_add(1, Ordering::Relaxed);
    BROKEN_NS.fetch_add(dt, Ordering::Relaxed);
    if k < 32 {
        record(M, "XMemMapPhysicalPages", &format!("split merged view {lo:#x}+{:#x} ({n} pages) to remap {va:#x}: threads frozen {:.2} ms ok={ok}; region now maps per page", run.hi - lo, dt as f64 / 1e6));
    }
    ok
}

pub(super) unsafe fn release_in(s: &mut State, lo: usize, hi: usize) {
    let mut r = runs();
    let inside: Vec<usize> = r.runs.range(lo..hi).map(|(a, _)| *a).collect();
    for a in inside {
        let run = r.runs.remove(&a).unwrap();
        UnmapViewOfFile2(GetCurrentProcess(), a as *mut c_void, MEM_PRESERVE_PLACEHOLDER);
        let mut p = a;
        while p < run.hi {
            s.remove_view(p);
            p += PAGE;
        }
        unref(s, run.sid);
    }
    r.fragmented.remove(&lo);
}

fn maybe_log() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let last = LAST_LOG.load(Ordering::Relaxed);
    if now < last + 5 || LAST_LOG.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    record(M, "XMemMapPhysicalPages", &format!("contiguous runs mapped as single views: {} runs / {} pages total, {} split back for partial remaps ({:.1} ms threads frozen)",
        MERGED.load(Ordering::Relaxed), MERGED_PAGES.load(Ordering::Relaxed), BROKEN.load(Ordering::Relaxed), BROKEN_NS.load(Ordering::Relaxed) as f64 / 1e6));
}

#[cfg(test)]
mod tests {
    use crate::sys::*;
    use crate::virt::*;
    use core::ffi::c_void;

    fn regions(va: usize, len: usize) -> usize {
        let mut n = 0;
        let mut p = va;
        while p < va + len {
            let mut info = XMemBasicInformation::default();
            unsafe { XMemVirtualQuery(GetCurrentProcess(), p as *const c_void, &mut info) };
            n += 1;
            p = info.base + info.region_size;
        }
        n
    }

    #[test]
    fn contiguous_map_is_one_view_and_partial_remap_keeps_neighbours() {
        let va = XMemVirtualAlloc(core::ptr::null_mut(), 34 * XMEM_PAGE, MEM_RESERVE | XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize;
        let mut n = 32usize;
        let mut p = [0u64; 32];
        assert_eq!(unsafe { XMemAllocatePhysicalPages(XMEM_LARGE_PAGES as u64, &mut n, p.as_mut_ptr()) }, 1);
        let mut one = 1usize;
        let mut q = [0u64; 1];
        assert_eq!(unsafe { XMemAllocatePhysicalPages(XMEM_64K_PAGES as u64, &mut one, q.as_mut_ptr()) }, 1);
        assert_eq!(unsafe { XMemMapPhysicalPages((va + XMEM_PAGE) as *mut c_void, 32, p.as_mut_ptr()) } as usize, va + XMEM_PAGE);
        assert_eq!(regions(va + XMEM_PAGE, 32 * XMEM_PAGE), 1);
        for i in 0..32 {
            unsafe { *((va + (i + 1) * XMEM_PAGE + 8) as *mut u32) = 100 + i as u32 };
        }
        assert_eq!(unsafe { XMemMapPhysicalPages((va + 17 * XMEM_PAGE) as *mut c_void, 1, q.as_mut_ptr()) } as usize, va + 17 * XMEM_PAGE);
        unsafe { *((va + 17 * XMEM_PAGE + 8) as *mut u32) = 7 };
        for i in 0..32 {
            let want = if i == 16 { 7 } else { 100 + i as u32 };
            assert_eq!(unsafe { *((va + (i + 1) * XMEM_PAGE + 8) as *const u32) }, want);
        }
        assert_eq!(unsafe { XMemMapPhysicalPages((va + XMEM_PAGE) as *mut c_void, 32, p.as_mut_ptr()) } as usize, va + XMEM_PAGE);
        assert_eq!(unsafe { *((va + 17 * XMEM_PAGE + 8) as *const u32) }, 116);
        assert_eq!(unsafe { XMemMapPhysicalPages((va + XMEM_PAGE) as *mut c_void, 32, core::ptr::null_mut()) } as usize, va + XMEM_PAGE);
        let vfree: unsafe extern "system" fn(usize, usize, u32) -> i32 = unsafe { core::mem::transmute(crate::hooks::vfree_wrapper()) };
        assert_eq!(unsafe { vfree(va, 0, MEM_RELEASE) }, 1);
        assert_eq!(unsafe { XMemFreePhysicalPages(32, p.as_mut_ptr()) }, 1);
        assert_eq!(unsafe { XMemFreePhysicalPages(1, q.as_mut_ptr()) }, 1);
    }

    #[test]
    fn release_with_merged_view_frees_region() {
        let va = XMemVirtualAlloc(core::ptr::null_mut(), 4 * XMEM_PAGE, MEM_RESERVE | XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize;
        let mut n = 4usize;
        let mut p = [0u64; 4];
        assert_eq!(unsafe { XMemAllocatePhysicalPages(XMEM_64K_PAGES as u64, &mut n, p.as_mut_ptr()) }, 1);
        assert_eq!(unsafe { XMemMapPhysicalPages(va as *mut c_void, 4, p.as_mut_ptr()) } as usize, va);
        unsafe { *((va + 3 * XMEM_PAGE) as *mut u32) = 1 };
        let vfree: unsafe extern "system" fn(usize, usize, u32) -> i32 = unsafe { core::mem::transmute(crate::hooks::vfree_wrapper()) };
        assert_eq!(unsafe { vfree(va, 0, MEM_RELEASE) }, 1);
        let mut info = XMemBasicInformation::default();
        unsafe { XMemVirtualQuery(GetCurrentProcess(), va as *const c_void, &mut info) };
        assert_eq!(info.state, 0x1_0000);
        assert_eq!(unsafe { XMemFreePhysicalPages(4, p.as_mut_ptr()) }, 1);
    }
}
