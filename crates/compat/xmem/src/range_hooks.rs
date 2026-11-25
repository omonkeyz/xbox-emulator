use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::hooks::{is_self, mem_slot_array, multi_rule, MEM_SLOTS, NTDLL_DLLS, WIN32_DLLS};
use crate::iat;
use crate::sys::MEM_RELEASE;
use compat_common::record;

const M: &str = "xmem";
const MEM_DECOMMIT: u32 = 0x4000;
const STATUS_SUCCESS: i32 = 0;
const STATUS_UNSUCCESSFUL: i32 = 0xc000_0001u32 as i32;
const STATUS_INVALID_PARAMETER: i32 = 0xc000_000du32 as i32;
const STATUS_FREE_VM_NOT_AT_BASE: i32 = 0xc000_009fu32 as i32;

static UNMAP_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];
static UNMAPEX_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];
static UNMAP2_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];
static PROTECT_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];
static PROTECTEX_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];
static NTFREE_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];
static NTPROTECT_ORIG: [AtomicUsize; MEM_SLOTS] = [const { AtomicUsize::new(0) }; MEM_SLOTS];

fn page_bounds(addr: usize, size: usize) -> (usize, usize) {
    let lo = addr & !0xfff;
    let hi = addr.saturating_add(size.max(1)).saturating_add(0xfff) & !0xfff;
    (lo, hi.max(lo.saturating_add(1)))
}

unsafe extern "system" fn unmap_w<const I: usize>(base: usize) -> i32 {
    let (lo, hi) = crate::epoch::bump_free(base, 0);
    let r = core::mem::transmute::<usize, unsafe extern "system" fn(usize) -> i32>(UNMAP_ORIG[I].load(Ordering::Acquire))(base);
    crate::epoch::bump_range(lo, hi);
    r
}

unsafe extern "system" fn unmapex_w<const I: usize>(base: usize, flags: u32) -> i32 {
    let (lo, hi) = crate::epoch::bump_free(base, 0);
    let r = core::mem::transmute::<usize, unsafe extern "system" fn(usize, u32) -> i32>(UNMAPEX_ORIG[I].load(Ordering::Acquire))(base, flags);
    crate::epoch::bump_range(lo, hi);
    r
}

unsafe extern "system" fn unmap2_w<const I: usize>(process: usize, base: usize, flags: u32) -> i32 {
    let f = core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, u32) -> i32>(UNMAP2_ORIG[I].load(Ordering::Acquire));
    if process != 0 && is_self(process) {
        let (lo, hi) = crate::epoch::bump_free(base, 0);
        let r = f(process, base, flags);
        crate::epoch::bump_range(lo, hi);
        r
    } else {
        f(process, base, flags)
    }
}

unsafe extern "system" fn protect_w<const I: usize>(addr: usize, size: usize, new: u32, old: *mut u32) -> i32 {
    let (lo, hi) = page_bounds(addr, size);
    crate::epoch::bump_range(lo, hi);
    let r = core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, u32, *mut u32) -> i32>(PROTECT_ORIG[I].load(Ordering::Acquire))(addr, size, new, old);
    crate::epoch::bump_range(lo, hi);
    r
}

unsafe extern "system" fn protectex_w<const I: usize>(process: usize, addr: usize, size: usize, new: u32, old: *mut u32) -> i32 {
    let f = core::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, usize, u32, *mut u32) -> i32>(PROTECTEX_ORIG[I].load(Ordering::Acquire));
    if process != 0 && is_self(process) {
        let (lo, hi) = page_bounds(addr, size);
        crate::epoch::bump_range(lo, hi);
        let r = f(process, addr, size, new, old);
        crate::epoch::bump_range(lo, hi);
        r
    } else {
        f(process, addr, size, new, old)
    }
}

unsafe extern "system" fn ntprotect_w<const I: usize>(process: usize, base: *mut usize, size: *mut usize, new: u32, old: *mut u32) -> i32 {
    let f = core::mem::transmute::<usize, unsafe extern "system" fn(usize, *mut usize, *mut usize, u32, *mut u32) -> i32>(NTPROTECT_ORIG[I].load(Ordering::Acquire));
    if process == 0 || !is_self(process) || base.is_null() || size.is_null() {
        return f(process, base, size, new, old);
    }
    let addr = *base;
    let len = *size;
    let (lo, hi) = page_bounds(addr, len);
    crate::epoch::bump_range(lo, hi);
    let st = f(process, base, size, new, old);
    if st >= 0 {
        let (olo, ohi) = page_bounds(*base, *size);
        crate::epoch::bump_range(lo.min(olo), hi.max(ohi));
    } else {
        crate::epoch::bump_range(lo, hi);
    }
    st
}

unsafe fn ntfree_inner(slot: usize, process: usize, base: *mut usize, size: *mut usize, kind: u32) -> i32 {
    let addr = *base;
    let len = *size;
    if kind & MEM_RELEASE != 0 {
        let extent = crate::phys::region_extent(addr);
        if let Some(r) = crate::phys::release_region(addr, len) {
            if r != 0 {
                if let Some(total) = extent {
                    *base = addr;
                    *size = total;
                }
                return STATUS_SUCCESS;
            }
            return if len != 0 { STATUS_INVALID_PARAMETER } else { STATUS_UNSUCCESSFUL };
        }
        if crate::phys::owns(addr) {
            return STATUS_FREE_VM_NOT_AT_BASE;
        }
    } else if kind & MEM_DECOMMIT != 0 {
        if let Some(r) = crate::phys::decommit_owned(addr, len) {
            if r != 0 {
                if let Some((rbase, rend)) = crate::phys::owned_extent(addr) {
                    let eff = if len == 0 { rend.saturating_sub(addr) } else { len };
                    let lo = addr & !0xfff;
                    let hi = addr.saturating_add(eff).saturating_add(0xfff) & !0xfff;
                    *base = lo.max(rbase);
                    *size = hi.min(rend).saturating_sub(*base);
                }
                return STATUS_SUCCESS;
            }
            return STATUS_UNSUCCESSFUL;
        }
    }
    let f = core::mem::transmute::<usize, unsafe extern "system" fn(usize, *mut usize, *mut usize, u32) -> i32>(NTFREE_ORIG[slot].load(Ordering::Acquire));
    f(process, base, size, kind)
}

unsafe extern "system" fn ntfree_w<const I: usize>(process: usize, base: *mut usize, size: *mut usize, kind: u32) -> i32 {
    let f = core::mem::transmute::<usize, unsafe extern "system" fn(usize, *mut usize, *mut usize, u32) -> i32>(NTFREE_ORIG[I].load(Ordering::Acquire));
    if process == 0 || !is_self(process) || base.is_null() || size.is_null() {
        return f(process, base, size, kind);
    }
    let addr = *base;
    let len = *size;
    let (lo, hi) = crate::epoch::bump_free(addr, len);
    let st = crate::title_acct::track_free_nt(addr, len, kind, || ntfree_inner(I, process, base, size, kind));
    if st >= 0 {
        let out_addr = *base;
        let out_len = *size;
        let lo2 = lo.min(out_addr);
        let hi2 = hi.max(out_addr.saturating_add(out_len));
        crate::epoch::bump_range(lo2, hi2);
        if kind & MEM_RELEASE != 0 {
            crate::write_watch::forget_range(lo2, hi2.saturating_sub(lo2));
        }
    } else {
        crate::epoch::bump_range(lo, hi);
    }
    st
}

fn unmap_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(unmap_w)
}

fn unmapex_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(unmapex_w)
}

fn unmap2_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(unmap2_w)
}

fn protect_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(protect_w)
}

fn protectex_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(protectex_w)
}

fn ntfree_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(ntfree_w)
}

fn ntprotect_wrappers() -> [usize; MEM_SLOTS] {
    mem_slot_array!(ntprotect_w)
}

pub(crate) fn wrappers() -> Vec<usize> {
    [
        unmap_wrappers(),
        unmapex_wrappers(),
        unmap2_wrappers(),
        protect_wrappers(),
        protectex_wrappers(),
        ntfree_wrappers(),
        ntprotect_wrappers(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(crate) fn rule(_dll: &[u8], name: Option<&[u8]>, value: usize) -> Option<usize> {
    multi_rule(WIN32_DLLS, &[b"UnmapViewOfFile"], &UNMAP_ORIG, &unmap_wrappers(), name, value)
        .or_else(|| multi_rule(WIN32_DLLS, &[b"UnmapViewOfFileEx"], &UNMAPEX_ORIG, &unmapex_wrappers(), name, value))
        .or_else(|| multi_rule(WIN32_DLLS, &[b"UnmapViewOfFile2"], &UNMAP2_ORIG, &unmap2_wrappers(), name, value))
        .or_else(|| multi_rule(WIN32_DLLS, &[b"VirtualProtect"], &PROTECT_ORIG, &protect_wrappers(), name, value))
        .or_else(|| multi_rule(WIN32_DLLS, &[b"VirtualProtectEx"], &PROTECTEX_ORIG, &protectex_wrappers(), name, value))
        .or_else(|| multi_rule(NTDLL_DLLS, &[b"NtFreeVirtualMemory", b"ZwFreeVirtualMemory"], &NTFREE_ORIG, &ntfree_wrappers(), name, value))
        .or_else(|| multi_rule(NTDLL_DLLS, &[b"NtProtectVirtualMemory", b"ZwProtectVirtualMemory"], &NTPROTECT_ORIG, &ntprotect_wrappers(), name, value))
}

pub fn install() {
    static DONE: AtomicU32 = AtomicU32::new(0);
    if DONE.swap(1, Ordering::AcqRel) == 0 {
        let n = iat::patch_all(&rule);
        record(M, "TomboxAddressSpaceEpoch", &format!("UnmapViewOfFile/UnmapViewOfFileEx/UnmapViewOfFile2/VirtualProtect/VirtualProtectEx/NtFreeVirtualMemory/NtProtectVirtualMemory imports log their ranges to the address-space change log: {n} slots"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoch::{TomboxAddressSpaceChangedSince, TomboxAddressSpaceEpoch};
    use crate::sys::{GetCurrentProcess, VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};

    const PAGE_READONLY: u32 = 0x02;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleA(name: *const u8) -> usize;
        fn GetProcAddress(h: usize, name: *const u8) -> usize;
    }

    fn resolve_proc(dll: &[u8], name: &[u8]) -> usize {
        let h = unsafe { GetModuleHandleA(dll.as_ptr()) };
        assert_ne!(h, 0);
        let p = unsafe { GetProcAddress(h, name.as_ptr()) };
        assert_ne!(p, 0);
        p
    }

    #[test]
    fn virtual_protect_ex_bumps_epoch_only_for_this_process() {
        let real = resolve_proc(b"kernel32.dll\0", b"VirtualProtectEx\0");
        let w = rule(b"kernel32.dll", Some(b"VirtualProtectEx"), real).expect("VirtualProtectEx hook");
        assert!(crate::hooks::is_ours(w));
        assert_eq!(rule(b"kernel32.dll", Some(b"VirtualProtectEx"), w), None);
        let pex: unsafe extern "system" fn(usize, usize, usize, u32, *mut u32) -> i32 = unsafe { core::mem::transmute(w) };

        let p = unsafe { VirtualAlloc(core::ptr::null_mut(), 0x2000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        let other = unsafe { VirtualAlloc(core::ptr::null_mut(), 0x1000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        assert_ne!(p, 0);
        assert_ne!(other, 0);

        let me = unsafe { GetCurrentProcess() } as usize;
        let s0 = TomboxAddressSpaceEpoch();
        let mut old = 0u32;
        assert_ne!(unsafe { pex(me, p + 0x100, 0x1000, PAGE_READONLY, &mut old) }, 0);
        assert_ne!(TomboxAddressSpaceChangedSince(s0, p, p + 0x1000), 0);
        assert_ne!(TomboxAddressSpaceChangedSince(s0, p + 0x1000, p + 0x2000), 0);
        assert_eq!(TomboxAddressSpaceChangedSince(s0, other, other + 0x1000), 0);

        let s1 = TomboxAddressSpaceEpoch();
        assert_eq!(unsafe { pex(0xDEAD_BEEFusize, p, 0x1000, PAGE_READWRITE, &mut old) }, 0);
        assert_eq!(TomboxAddressSpaceChangedSince(s1, p, p + 0x1000), 0);

        unsafe {
            assert_ne!(VirtualFree(other as *mut _, 0, MEM_RELEASE), 0);
            assert_ne!(VirtualFree(p as *mut _, 0, MEM_RELEASE), 0);
        }
    }

    #[test]
    fn nt_protect_virtual_memory_bumps_epoch_and_handles_invalid_args() {
        let nt_real = resolve_proc(b"ntdll.dll\0", b"NtProtectVirtualMemory\0");
        let zw_real = resolve_proc(b"ntdll.dll\0", b"ZwProtectVirtualMemory\0");
        let w = rule(b"ntdll.dll", Some(b"NtProtectVirtualMemory"), nt_real).expect("NtProtectVirtualMemory hook");
        assert!(crate::hooks::is_ours(w));
        assert_eq!(rule(b"ntdll.dll", Some(b"ZwProtectVirtualMemory"), nt_real), Some(w));
        let w_zw = rule(b"ntdll.dll", None, zw_real).expect("ZwProtectVirtualMemory hook by address");
        assert_eq!(rule(b"ntdll.dll", Some(b"ZwProtectVirtualMemory"), zw_real), Some(w_zw));
        assert_eq!(rule(b"ntdll.dll", Some(b"NtProtectVirtualMemory"), w), None);
        let np: unsafe extern "system" fn(usize, *mut usize, *mut usize, u32, *mut u32) -> i32 = unsafe { core::mem::transmute(w) };

        let p = unsafe { VirtualAlloc(core::ptr::null_mut(), 0x2000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        let other = unsafe { VirtualAlloc(core::ptr::null_mut(), 0x1000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        assert_ne!(p, 0);
        assert_ne!(other, 0);

        let me = unsafe { GetCurrentProcess() } as usize;
        let mut base = p + 0x100;
        let mut size = 0x1000usize;
        let mut old = 0u32;
        let s0 = TomboxAddressSpaceEpoch();
        assert!(unsafe { np(me, &mut base, &mut size, PAGE_READONLY, &mut old) } >= 0);
        assert_eq!(base, p);
        assert_eq!(size, 0x2000);
        assert_ne!(TomboxAddressSpaceChangedSince(s0, p, p + 0x1000), 0);
        assert_ne!(TomboxAddressSpaceChangedSince(s0, p + 0x1000, p + 0x2000), 0);
        assert_eq!(TomboxAddressSpaceChangedSince(s0, other, other + 0x1000), 0);

        let s1 = TomboxAddressSpaceEpoch();
        assert!(unsafe { np(me, core::ptr::null_mut(), &mut size, PAGE_READWRITE, &mut old) } < 0);
        assert!(unsafe { np(0xDEAD_BEEFusize, &mut base, &mut size, PAGE_READWRITE, &mut old) } < 0);
        assert_eq!(TomboxAddressSpaceChangedSince(s1, p, p + 0x2000), 0);

        unsafe {
            assert_ne!(VirtualFree(other as *mut _, 0, MEM_RELEASE), 0);
            assert_ne!(VirtualFree(p as *mut _, 0, MEM_RELEASE), 0);
        }
    }

    #[test]
    fn nt_free_virtual_memory_updates_epoch_accounting_and_write_watch() {
        let nt_real = resolve_proc(b"ntdll.dll\0", b"NtFreeVirtualMemory\0");
        let w = rule(b"ntdll.dll", Some(b"NtFreeVirtualMemory"), nt_real).expect("NtFreeVirtualMemory hook");
        assert!(crate::hooks::is_ours(w));
        assert_eq!(rule(b"ntdll.dll", Some(b"ZwFreeVirtualMemory"), nt_real), Some(w));
        assert_eq!(rule(b"ntdll.dll", Some(b"NtFreeVirtualMemory"), w), None);
        let ntf: unsafe extern "system" fn(usize, *mut usize, *mut usize, u32) -> i32 = unsafe { core::mem::transmute(w) };
        let valloc: unsafe extern "system" fn(usize, usize, u32, u32) -> usize = unsafe { core::mem::transmute(crate::hooks::valloc_wrapper()) };

        let start = crate::title_acct::local_virt();
        let p = unsafe { valloc(0, 0x4000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        let other = unsafe { VirtualAlloc(core::ptr::null_mut(), 0x1000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        assert_ne!(p, 0);
        assert_ne!(other, 0);
        assert_eq!(crate::title_acct::local_virt() - start, 0x4000);

        let me = unsafe { GetCurrentProcess() } as usize;
        let mut base = p + 0x1000;
        let mut size = 0x2000usize;
        let s0 = TomboxAddressSpaceEpoch();
        assert!(unsafe { ntf(me, &mut base, &mut size, MEM_DECOMMIT) } >= 0);
        assert_eq!(crate::title_acct::local_virt() - start, 0x2000);
        assert_ne!(TomboxAddressSpaceChangedSince(s0, p + 0x1000, p + 0x3000), 0);
        assert_eq!(TomboxAddressSpaceChangedSince(s0, other, other + 0x1000), 0);

        let mut bad_base = p + 0x1000;
        let mut bad_size = 0usize;
        assert!(unsafe { ntf(me, &mut bad_base, &mut bad_size, MEM_RELEASE) } < 0);
        assert_eq!(crate::title_acct::local_virt() - start, 0x2000);
        assert!(unsafe { ntf(me, core::ptr::null_mut(), &mut bad_size, MEM_RELEASE) } < 0);
        assert!(unsafe { ntf(0xDEAD_BEEFusize, &mut base, &mut bad_size, MEM_RELEASE) } < 0);

        base = p;
        size = 0;
        let s1 = TomboxAddressSpaceEpoch();
        assert!(unsafe { ntf(me, &mut base, &mut size, MEM_RELEASE) } >= 0);
        assert_eq!(crate::title_acct::local_virt() - start, 0);
        assert_ne!(TomboxAddressSpaceChangedSince(s1, p, p + 0x4000), 0);
        assert_eq!(TomboxAddressSpaceChangedSince(s1, other, other + 0x1000), 0);

        let wsize = 0x2_0000;
        let wp = crate::write_watch::reserve(wsize, 0x1_0000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) as usize;
        assert_ne!(wp, 0);
        if crate::write_watch::wanted() {
            assert_eq!(crate::write_watch::owner(wp).0, crate::write_watch::OWNER_EMULATOR);
        }
        let mut wbase = wp;
        let mut wlen = 0usize;
        assert!(unsafe { ntf(me, &mut wbase, &mut wlen, MEM_RELEASE) } >= 0);
        assert_eq!(crate::write_watch::owner(wp).0, crate::write_watch::OWNER_NONE);

        unsafe {
            assert_ne!(VirtualFree(other as *mut _, 0, MEM_RELEASE), 0);
        }
    }

    #[test]
    fn nt_free_virtual_memory_handles_xmem_physical_region() {
        let _g = crate::phys::map_test_guard();
        let nt_real = resolve_proc(b"ntdll.dll\0", b"NtFreeVirtualMemory\0");
        let w = rule(b"ntdll.dll", Some(b"NtFreeVirtualMemory"), nt_real).expect("NtFreeVirtualMemory hook");
        let ntf: unsafe extern "system" fn(usize, *mut usize, *mut usize, u32) -> i32 = unsafe { core::mem::transmute(w) };
        let valloc: unsafe extern "system" fn(usize, usize, u32, u32) -> usize = unsafe { core::mem::transmute(crate::hooks::valloc_wrapper()) };

        let page = crate::phys::PAGE;
        let va = crate::virt::XMemVirtualAlloc(core::ptr::null_mut(), 4 * page, MEM_RESERVE | crate::virt::XMEM_64K_PAGES, 0, PAGE_READWRITE) as usize;
        assert_ne!(va, 0);
        let mut n = 1usize;
        let mut pfn = [0u64; 1];
        assert_eq!(unsafe { crate::virt::XMemAllocatePhysicalPages(crate::virt::XMEM_64K_PAGES as u64, &mut n, pfn.as_mut_ptr()) }, 1);
        assert_eq!(unsafe { crate::virt::XMemMapPhysicalPages(va as *mut _, 1, pfn.as_mut_ptr()) } as usize, va);
        assert_eq!(unsafe { valloc(va + page + 0x1000, 0x2000, MEM_COMMIT, PAGE_READWRITE) }, va + page + 0x1000);

        let me = unsafe { GetCurrentProcess() } as usize;
        let mut base = va + page + 0x1000;
        let mut size = 0x1000usize;
        let s0 = TomboxAddressSpaceEpoch();
        assert_eq!(unsafe { ntf(me, &mut base, &mut size, MEM_DECOMMIT) }, STATUS_SUCCESS);
        assert_eq!(base, va + page + 0x1000);
        assert_eq!(size, 0x1000);
        assert_ne!(TomboxAddressSpaceChangedSince(s0, va + page + 0x1000, va + page + 0x2000), 0);
        let mut info = crate::virt::XMemBasicInformation::default();
        unsafe { crate::virt::XMemVirtualQuery(GetCurrentProcess(), (va + page + 0x1000) as *const _, &mut info) };
        assert_eq!(info.state, MEM_RESERVE);

        let mut bad_base = va + page;
        let mut bad_size = 0usize;
        assert_eq!(unsafe { ntf(me, &mut bad_base, &mut bad_size, MEM_RELEASE) }, STATUS_FREE_VM_NOT_AT_BASE);
        assert_eq!(crate::phys::owned_extent(va), Some((va, va + 4 * page)));

        let mut nz_base = va;
        let mut nz_size = 0x1000usize;
        assert_eq!(unsafe { ntf(me, &mut nz_base, &mut nz_size, MEM_RELEASE) }, STATUS_INVALID_PARAMETER);
        assert_eq!(crate::phys::owned_extent(va), Some((va, va + 4 * page)));

        base = va;
        size = 0;
        let s1 = TomboxAddressSpaceEpoch();
        assert_eq!(unsafe { ntf(me, &mut base, &mut size, MEM_RELEASE) }, STATUS_SUCCESS);
        assert_eq!(base, va);
        assert_eq!(size, 4 * page);
        assert_ne!(TomboxAddressSpaceChangedSince(s1, va, va + 4 * page), 0);
        assert_eq!(crate::phys::owned_extent(va), None);
        for i in 0..4 {
            assert_eq!(crate::phys::released(va + i * page), Ok(()));
        }
        assert_eq!(unsafe { crate::virt::XMemFreePhysicalPages(1, pfn.as_mut_ptr()) }, 1);
    }
}
