use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::sys::*;
use compat_common::record;

const M: &str = "xmem";

pub const XMEM_LARGE_PAGES: u32 = 0x2000_0000;
pub const XMEM_64K_PAGES: u32 = 0x2040_0000;
pub const XMEM_PAGE: usize = 0x1_0000;

pub fn page_granularity(alloc_type: u32) -> usize {
    if alloc_type & XMEM_64K_PAGES == XMEM_64K_PAGES {
        0x1_0000
    } else if alloc_type & XMEM_LARGE_PAGES != 0 {
        0x20_0000
    } else {
        0x1000
    }
}

pub fn round_range(addr: usize, size: usize, gran: usize) -> (usize, usize) {
    if addr == 0 {
        (0, round_up(size, gran))
    } else {
        let base = addr & !(gran - 1);
        (base, round_up(addr + size, gran) - base)
    }
}

pub fn win_protect(protect: u32) -> u32 {
    let base = protect & 0xff;
    let base = match base {
        0x01 | 0x02 | 0x04 | 0x08 | 0x10 | 0x20 | 0x40 | 0x80 => base,
        _ => PAGE_READWRITE,
    };
    base | (protect & PAGE_GUARD)
}

pub fn win_alloc_type(alloc_type: u32) -> u32 {
    let t = alloc_type & WIN_ALLOC_TYPE_MASK;
    if t == 0 {
        MEM_RESERVE | MEM_COMMIT
    } else {
        t
    }
}

#[no_mangle]
pub extern "system" fn XMemVirtualAlloc(
    address: *mut c_void,
    size: usize,
    alloc_type: u32,
    attributes: u64,
    protect: u32,
) -> *mut c_void {
    let gran = page_granularity(alloc_type);
    let (base, len) = round_range(address as usize, size, gran);
    let wt = win_alloc_type(alloc_type);
    let wp = win_protect(protect);
    if base != 0 && wt & MEM_COMMIT != 0 {
        if let Some(r) = crate::phys::commit_owned(base, len, wp) {
            crate::write_note::committed(r, len);
            return r as *mut c_void;
        }
    }
    let p = crate::phys::serialized(|| {
        if base == 0 && wt & MEM_RESERVE != 0 && gran > 0x1_0000 {
            reserve_aligned(len, gran, wt, wp)
        } else {
            unsafe { VirtualAlloc(base as *mut c_void, len, wt, wp) }
        }
    });
    if p.is_null() {
        let err = unsafe { GetLastError() };
        record(
            M,
            "XMemVirtualAlloc",
            &format!("addr={address:p} size={size:#x} type={alloc_type:#x}->{wt:#x} prot={protect:#x}->{wp:#x} attrs={attributes:#x} -> NULL (err={err})"),
        );
    } else if wt & MEM_COMMIT != 0 {
        crate::write_note::committed(p as usize, len);
    }
    p
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct XMemBasicInformation {
    pub base: usize,
    pub alloc_base: usize,
    pub alloc_protect: u32,
    pub reserved0: u32,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
    pub reserved1: u32,
    pub flags: u64,
    pub tag: u32,
    pub tag_text: [u8; 4],
}

#[no_mangle]
pub unsafe extern "system" fn XMemVirtualQuery(process: Handle, address: *const c_void, out: *mut XMemBasicInformation) -> i32 {
    if out.is_null() {
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let mut mbi = MemoryBasicInformation::default();
    let n = VirtualQueryEx(process, address, &mut mbi, core::mem::size_of::<MemoryBasicInformation>());
    if n == 0 {
        return 0;
    }
    *out = XMemBasicInformation {
        base: mbi.base,
        alloc_base: mbi.alloc_base,
        alloc_protect: mbi.alloc_protect,
        region_size: mbi.region_size,
        state: mbi.state,
        protect: mbi.protect,
        kind: mbi.kind,
        flags: if mbi.partition_id == 0 { 1 } else { 0 },
        ..Default::default()
    };
    1
}

pub static WRITE_WATCH_SUSPENDED: AtomicU32 = AtomicU32::new(0);
pub static WRITE_WATCH_OPTION: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub unsafe extern "system" fn XMemGetWriteWatch(
    flags: u32,
    base: *mut c_void,
    size: usize,
    addresses: *mut *mut c_void,
    count: *mut usize,
    granularity: *mut u32,
) -> u32 {
    if WRITE_WATCH_SUSPENDED.load(Ordering::Acquire) != 0 {
        return ERROR_CAN_NOT_COMPLETE;
    }
    GetWriteWatch(flags, base, size, addresses, count, granularity)
}

#[no_mangle]
pub unsafe extern "system" fn XMemResetWriteWatch(base: *mut c_void, size: usize) -> u32 {
    if WRITE_WATCH_SUSPENDED.load(Ordering::Acquire) != 0 {
        return ERROR_CAN_NOT_COMPLETE;
    }
    ResetWriteWatch(base, size)
}

#[no_mangle]
pub extern "system" fn XMemResumeWriteWatch() -> i32 {
    if WRITE_WATCH_OPTION.load(Ordering::Acquire) != 0 {
        return 0;
    }
    WRITE_WATCH_SUSPENDED.store(0, Ordering::Release);
    1
}

#[no_mangle]
pub unsafe extern "system" fn XMemAllocatePhysicalPages(flags: u64, num_pages: *mut usize, page_array: *mut u64) -> i32 {
    if num_pages.is_null() {
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let n = *num_pages;
    if (flags as u32) & XMEM_64K_PAGES == XMEM_LARGE_PAGES && n & 0x1f != 0 {
        *num_pages = 0;
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    }
    if n != 0 && page_array.is_null() {
        *num_pages = 0;
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    }
    match crate::phys::allocate(n) {
        Some(v) => {
            core::ptr::copy_nonoverlapping(v.as_ptr(), page_array, v.len());
            1
        }
        None => {
            let err = GetLastError();
            *num_pages = 0;
            record(M, "XMemAllocatePhysicalPages", &format!("flags={flags:#x} pages={n} -> FAILED (err={err})"));
            SetLastError(err);
            0
        }
    }
}

pub const FREE_CHUNK: usize = 0x1000;
pub const ERROR_NOACCESS: u32 = 998;

pub unsafe fn free_physical_chunked(mut num_pages: usize, mut page_array: *const u64, chunk: usize) -> i32 {
    while num_pages != 0 {
        let n = num_pages.min(chunk);
        num_pages -= n;
        if page_array.is_null() {
            SetLastError(ERROR_NOACCESS);
            return 0;
        }
        if !crate::phys::free(core::slice::from_raw_parts(page_array, n)) {
            SetLastError(ERROR_INVALID_PARAMETER);
            return 0;
        }
        page_array = page_array.add(n);
    }
    1
}

#[no_mangle]
pub unsafe extern "system" fn XMemFreePhysicalPages(num_pages: usize, page_array: *mut u64) -> i32 {
    free_physical_chunked(num_pages, page_array, FREE_CHUNK)
}

#[no_mangle]
pub unsafe extern "system" fn XMemMapPhysicalPages(va: *mut c_void, num_pages: usize, page_array: *mut u64) -> *mut c_void {
    if va.is_null() {
        SetLastError(ERROR_INVALID_PARAMETER);
        return core::ptr::null_mut();
    }
    if num_pages == 0 {
        return va;
    }
    let base = va as usize & !(XMEM_PAGE - 1);
    let pages: Vec<(usize, Option<u64>)> = (0..num_pages)
        .map(|i| (base + i * XMEM_PAGE, if page_array.is_null() { None } else { Some(*page_array.add(i)).filter(|&p| p != 0) }))
        .collect();
    if crate::phys::map(&pages) {
        va
    } else {
        let err = GetLastError();
        record(M, "XMemMapPhysicalPages", &format!("va={va:p} pages={num_pages} map={} -> NULL (err={err})", !page_array.is_null()));
        SetLastError(err);
        core::ptr::null_mut()
    }
}

#[no_mangle]
pub unsafe extern "system" fn XMemMapPhysicalPagesScatter(addresses: *mut *mut c_void, num_pages: usize, page_array: *mut u64) -> i32 {
    if addresses.is_null() {
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let pages: Vec<(usize, Option<u64>)> = (0..num_pages)
        .map(|i| (*addresses.add(i) as usize, if page_array.is_null() { None } else { Some(*page_array.add(i)).filter(|&p| p != 0) }))
        .collect();
    if crate::phys::map(&pages) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granularity_follows_console_page_bits() {
        assert_eq!(page_granularity(0), 0x1000);
        assert_eq!(page_granularity(0x2040_3000), 0x1_0000);
        assert_eq!(page_granularity(0x2000_3000), 0x20_0000);
        assert_eq!(round_range(0, 1, 0x1_0000), (0, 0x1_0000));
        assert_eq!(round_range(0x1_2345, 0x10, 0x1_0000), (0x1_0000, 0x1_0000));
        assert_eq!(round_range(0x1_fff0, 0x20, 0x1_0000), (0x1_0000, 0x2_0000));
    }

    #[test]
    fn protect_and_type_mapping() {
        assert_eq!(win_protect(0x04), 0x04);
        assert_eq!(win_protect(0x404), 0x04);
        assert_eq!(win_protect(0x2000), PAGE_READWRITE);
        assert_eq!(win_protect(0x102), 0x102);
        assert_eq!(win_alloc_type(0x2040_3000), 0x3000);
        assert_eq!(win_alloc_type(0x2000_0000), 0x3000);
        assert_eq!(win_alloc_type(0x0020_2000), 0x0020_2000);
    }

    #[test]
    fn large_page_reservation_is_aligned_and_usable() {
        let p = XMemVirtualAlloc(core::ptr::null_mut(), 0x30_0000, 0x2000_3000, 0, 0x404);
        assert!(!p.is_null());
        assert_eq!(p as usize % 0x20_0000, 0);
        unsafe { *(p as *mut u64).add(0x3f_fff8 / 8) = 7 };
        let mut info = XMemBasicInformation::default();
        assert_eq!(unsafe { XMemVirtualQuery(GetCurrentProcess(), p, &mut info) }, 1);
        assert_eq!(info.alloc_base, p as usize);
        assert_eq!(info.state, MEM_COMMIT);
        assert_eq!(info.region_size, 0x40_0000);
        unsafe { VirtualFree(p, 0, MEM_RELEASE) };
    }

    #[test]
    fn physical_pages_map_and_unmap() {
        let va = XMemVirtualAlloc(core::ptr::null_mut(), 4 * XMEM_PAGE, MEM_RESERVE | XMEM_64K_PAGES, 0, PAGE_READWRITE);
        assert!(!va.is_null());
        let mut n = 4usize;
        let mut pfns = [0u64; 4];
        assert_eq!(unsafe { XMemAllocatePhysicalPages(XMEM_64K_PAGES as u64, &mut n, pfns.as_mut_ptr()) }, 1);
        assert!(pfns.iter().all(|&p| p != 0));
        assert_eq!(unsafe { XMemMapPhysicalPages(va, 4, pfns.as_mut_ptr()) }, va);
        unsafe { *((va as *mut u8).add(XMEM_PAGE) as *mut u32) = 5 };
        assert_eq!(unsafe { XMemMapPhysicalPages(va, 0, pfns.as_mut_ptr()) }, va);
        assert!(unsafe { XMemMapPhysicalPages(core::ptr::null_mut(), 1, pfns.as_mut_ptr()) }.is_null());
        let mut addrs = [unsafe { (va as *mut u8).add(XMEM_PAGE) } as *mut c_void];
        assert_eq!(unsafe { XMemMapPhysicalPagesScatter(addrs.as_mut_ptr(), 1, core::ptr::null_mut()) }, 1);
        let mut info = XMemBasicInformation::default();
        unsafe { XMemVirtualQuery(GetCurrentProcess(), addrs[0], &mut info) };
        assert_eq!(info.state, MEM_RESERVE);
        assert_eq!(unsafe { XMemMapPhysicalPages(va, 4, core::ptr::null_mut()) }, va);
        assert_eq!(unsafe { XMemFreePhysicalPages(4, pfns.as_mut_ptr()) }, 1);
        let mut odd = 3usize;
        assert_eq!(unsafe { XMemAllocatePhysicalPages(XMEM_LARGE_PAGES as u64, &mut odd, pfns.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { VirtualFree(va, 0, MEM_RELEASE) }, 1);
    }

    #[test]
    fn stale_pfn_in_batch_preserves_both_existing_views() {
        let base = XMemVirtualAlloc(core::ptr::null_mut(), 2 * XMEM_PAGE, MEM_RESERVE | XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize;
        assert_ne!(base, 0);
        let mut addrs = [base as *mut c_void, (base + XMEM_PAGE) as *mut c_void];
        let mut n = 3;
        let mut pfns = [0u64; 3];
        unsafe {
            assert_eq!(XMemAllocatePhysicalPages(XMEM_64K_PAGES as u64, &mut n, pfns.as_mut_ptr()), 1);
            assert_eq!(XMemMapPhysicalPages(addrs[0], 2, pfns.as_mut_ptr()), addrs[0]);
            *(addrs[0] as *mut u32) = 11;
            *(addrs[1] as *mut u32) = 22;
            assert_eq!(XMemFreePhysicalPages(1, &mut pfns[2]), 1);
            let mut replacement = [pfns[1], pfns[2]];
            for scatter in [false, true] {
                if scatter {
                    assert_eq!(XMemMapPhysicalPagesScatter(addrs.as_mut_ptr(), 2, replacement.as_mut_ptr()), 0);
                } else {
                    assert!(XMemMapPhysicalPages(addrs[0], 2, replacement.as_mut_ptr()).is_null());
                }
                let error = GetLastError();
                for (address, expected) in addrs.iter().zip([11, 22]) {
                    let mut info = XMemBasicInformation::default();
                    XMemVirtualQuery(GetCurrentProcess(), *address, &mut info);
                    assert_eq!(info.state, MEM_COMMIT);
                    assert_eq!(*(*address as *const u32), expected);
                }
                assert_eq!(error, ERROR_INVALID_PARAMETER);
            }
            assert_eq!(crate::phys::release_region(base, 0), Some(1));
            assert_eq!(XMemFreePhysicalPages(2, pfns.as_mut_ptr()), 1);
        }
    }

    #[test]
    fn owned_region_mixes_commit_and_physical_pages() {
        let va = XMemVirtualAlloc(core::ptr::null_mut(), 4 * XMEM_PAGE, MEM_RESERVE | XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize;
        let mut n = 1usize;
        let mut p = [0u64; 1];
        assert_eq!(unsafe { XMemAllocatePhysicalPages(XMEM_64K_PAGES as u64, &mut n, p.as_mut_ptr()) }, 1);
        assert_eq!(unsafe { XMemMapPhysicalPages(va as *mut c_void, 1, p.as_mut_ptr()) } as usize, va);
        let valloc: unsafe extern "system" fn(usize, usize, u32, u32) -> usize = unsafe { core::mem::transmute(crate::hooks::valloc_wrapper()) };
        let vfree: unsafe extern "system" fn(usize, usize, u32) -> i32 = unsafe { core::mem::transmute(crate::hooks::vfree_wrapper()) };
        assert_eq!(unsafe { valloc(va + XMEM_PAGE + 0x1234, 0x1_8000, MEM_COMMIT, PAGE_READWRITE) }, va + XMEM_PAGE + 0x1000);
        unsafe {
            *((va + XMEM_PAGE + 0x1000) as *mut u32) = 3;
            *((va + 2 * XMEM_PAGE + 0x9000) as *mut u32) = 4;
            *(va as *mut u32) = 9;
        }
        assert_eq!(unsafe { valloc(va + 0x10, 0x10, MEM_COMMIT, PAGE_READWRITE) }, 0);
        let q = XMemVirtualAlloc((va + 3 * XMEM_PAGE) as *mut c_void, 0x1000, MEM_COMMIT, 0, PAGE_READWRITE) as usize;
        assert_eq!(q, va + 3 * XMEM_PAGE);
        unsafe { *(q as *mut u32) = 5 };
        assert_eq!(unsafe { vfree(va + XMEM_PAGE + 0x1000, 0x1000, 0x4000) }, 1);
        let mut info = XMemBasicInformation::default();
        unsafe { XMemVirtualQuery(GetCurrentProcess(), (va + XMEM_PAGE + 0x1000) as *const c_void, &mut info) };
        assert_eq!(info.state, MEM_RESERVE);
        assert_eq!(unsafe { vfree(va + XMEM_PAGE, 0, MEM_RELEASE) }, 0);
        assert_eq!(unsafe { *((va + 2 * XMEM_PAGE + 0x9000) as *const u32) }, 4);
        assert_eq!(unsafe { XMemMapPhysicalPages((va + 2 * XMEM_PAGE) as *mut c_void, 1, p.as_mut_ptr()) } as usize, va + 2 * XMEM_PAGE);
        assert_eq!(unsafe { *((va + 2 * XMEM_PAGE) as *const u32) }, 9);
        assert_eq!(unsafe { vfree(va, 0, MEM_RELEASE) }, 1);
        for i in 0..4 {
            unsafe { XMemVirtualQuery(GetCurrentProcess(), (va + i * XMEM_PAGE) as *const c_void, &mut info) };
            assert_eq!(info.state, 0x1_0000);
        }
        assert_eq!(unsafe { XMemFreePhysicalPages(1, p.as_mut_ptr()) }, 1);
    }

    #[test]
    fn free_physical_pages_matches_console_chunking() {
        assert_eq!(unsafe { XMemFreePhysicalPages(0, core::ptr::null_mut()) }, 1);
        assert_eq!(unsafe { XMemFreePhysicalPages(2, core::ptr::null_mut()) }, 0);
        assert_eq!(unsafe { GetLastError() }, ERROR_NOACCESS);
        let mut n = 6usize;
        let mut p = [0u64; 6];
        assert_eq!(unsafe { XMemAllocatePhysicalPages(XMEM_64K_PAGES as u64, &mut n, p.as_mut_ptr()) }, 1);
        let mut list = [p[0], p[1], p[2], p[3], p[4], p[4]];
        assert_eq!(unsafe { free_physical_chunked(6, list.as_mut_ptr(), 4) }, 0);
        assert_eq!(unsafe { GetLastError() }, ERROR_INVALID_PARAMETER);
        assert_eq!(unsafe { free_physical_chunked(1, list.as_mut_ptr(), 4) }, 0);
        let mut rest = [p[4], p[5]];
        assert_eq!(unsafe { XMemFreePhysicalPages(2, rest.as_mut_ptr()) }, 1);
        assert_eq!(unsafe { XMemFreePhysicalPages(1, rest.as_mut_ptr()) }, 0);
    }

    #[test]
    fn physical_pages_alias_and_persist() {
        let va = XMemVirtualAlloc(core::ptr::null_mut(), 4 * XMEM_PAGE, MEM_RESERVE | XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize;
        let vb = XMemVirtualAlloc(core::ptr::null_mut(), 2 * XMEM_PAGE, MEM_RESERVE | XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize;
        assert!(va != 0 && vb != 0);
        let mut n = 2usize;
        let mut pfns = [0u64; 2];
        assert_eq!(unsafe { XMemAllocatePhysicalPages(XMEM_64K_PAGES as u64, &mut n, pfns.as_mut_ptr()) }, 1);
        assert!(pfns[0] != 0 && pfns[1] != pfns[0]);
        let mut twice = [pfns[0], pfns[0], pfns[1], 0];
        assert_eq!(unsafe { XMemMapPhysicalPages(va as *mut c_void, 4, twice.as_mut_ptr()) } as usize, va);
        unsafe {
            *((va + 0x10) as *mut u32) = 0xdead_beef;
            assert_eq!(*((va + XMEM_PAGE + 0x10) as *const u32), 0xdead_beef);
            *((va + 2 * XMEM_PAGE) as *mut u64) = 0x1122_3344_5566_7788;
        }
        let mut info = XMemBasicInformation::default();
        unsafe { XMemVirtualQuery(GetCurrentProcess(), (va + 3 * XMEM_PAGE) as *const c_void, &mut info) };
        assert_eq!(info.state, MEM_RESERVE);
        assert_eq!(unsafe { XMemMapPhysicalPages(va as *mut c_void, 4, core::ptr::null_mut()) } as usize, va);
        unsafe { XMemVirtualQuery(GetCurrentProcess(), va as *const c_void, &mut info) };
        assert_eq!(info.state, MEM_RESERVE);
        let mut addrs = [(vb + XMEM_PAGE) as *mut c_void, vb as *mut c_void];
        assert_eq!(unsafe { XMemMapPhysicalPagesScatter(addrs.as_mut_ptr(), 2, pfns.as_mut_ptr()) }, 1);
        unsafe {
            assert_eq!(*((vb + XMEM_PAGE + 0x10) as *const u32), 0xdead_beef);
            assert_eq!(*(vb as *const u64), 0x1122_3344_5566_7788);
        }
        assert_eq!(unsafe { XMemFreePhysicalPages(2, pfns.as_mut_ptr()) }, 1);
        assert_eq!(unsafe { *((vb + XMEM_PAGE + 0x10) as *const u32) }, 0xdead_beef);
        assert_eq!(unsafe { XMemFreePhysicalPages(1, pfns.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { GetLastError() }, ERROR_INVALID_PARAMETER);
        let vfree: unsafe extern "system" fn(usize, usize, u32) -> i32 = unsafe { core::mem::transmute(crate::hooks::vfree_wrapper()) };
        assert_eq!(unsafe { vfree(vb, 0, MEM_RELEASE) }, 1);
        for i in 0..2 {
            unsafe { XMemVirtualQuery(GetCurrentProcess(), (vb + i * XMEM_PAGE) as *const c_void, &mut info) };
            assert_eq!(info.state, 0x1_0000);
        }
        assert_eq!(unsafe { VirtualFree(va as *mut c_void, 0, MEM_RELEASE) }, 1);
        for i in 0..4 {
            unsafe { XMemVirtualQuery(GetCurrentProcess(), (va + i * XMEM_PAGE) as *const c_void, &mut info) };
            assert_eq!(info.state, 0x1_0000);
        }
    }

    #[test]
    fn write_watch_tracks_and_suspends() {
        let p = XMemVirtualAlloc(core::ptr::null_mut(), 0x4000, MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH, 0, PAGE_READWRITE);
        assert!(!p.is_null());
        unsafe { *(p as *mut u8).add(0x2000) = 1 };
        let mut addrs = [core::ptr::null_mut::<c_void>(); 4];
        let mut count = 4usize;
        let mut gran = 0u32;
        assert_eq!(unsafe { XMemGetWriteWatch(0, p, 0x4000, addrs.as_mut_ptr(), &mut count, &mut gran) }, 0);
        assert_eq!(count, 1);
        assert_eq!(addrs[0] as usize, p as usize + 0x2000);
        WRITE_WATCH_SUSPENDED.store(1, Ordering::Release);
        assert_eq!(unsafe { XMemResetWriteWatch(p, 0x4000) }, ERROR_CAN_NOT_COMPLETE);
        assert_eq!(XMemResumeWriteWatch(), 1);
        assert_eq!(unsafe { XMemResetWriteWatch(p, 0x4000) }, 0);
        unsafe { VirtualFree(p, 0, MEM_RELEASE) };
    }
}
