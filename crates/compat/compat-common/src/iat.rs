
use core::ffi::c_void;

extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old_protect: *mut u32) -> i32;
}

pub unsafe fn slots_bound_to(base: *const u8, target: usize) -> Vec<usize> {
    let rd32 = |off: usize| core::ptr::read_unaligned(base.add(off) as *const u32) as usize;
    let mut out = Vec::new();
    if core::ptr::read_unaligned(base as *const u16) != 0x5a4d {
        return out;
    }
    let nt = rd32(0x3c);
    if rd32(nt) != 0x4550 || core::ptr::read_unaligned(base.add(nt + 24) as *const u16) != 0x20b {
        return out;
    }
    let (dir, dir_size) = (rd32(nt + 24 + 112 + 8), rd32(nt + 24 + 112 + 12));
    if dir == 0 || dir_size == 0 {
        return out;
    }
    let mut desc = dir;
    while desc + 20 <= dir + dir_size && rd32(desc + 12) != 0 {
        let iat = rd32(desc + 16);
        let mut slot = iat;
        loop {
            let value = core::ptr::read_unaligned(base.add(slot) as *const usize);
            if value == 0 {
                break;
            }
            if value == target {
                out.push(slot);
            }
            slot += 8;
        }
        desc += 20;
    }
    out
}

pub unsafe fn redirect_main_module(target: usize, replacement: usize) -> usize {
    let base = GetModuleHandleW(core::ptr::null()) as *mut u8;
    if base.is_null() || target == 0 {
        return 0;
    }
    let mut n = 0;
    for off in slots_bound_to(base, target) {
        let slot = base.add(off) as *mut usize;
        let mut old = 0u32;
        if VirtualProtect(slot as *mut c_void, 8, 4, &mut old) == 0 {
            continue;
        }
        core::ptr::write_volatile(slot, replacement);
        let mut tmp = 0u32;
        VirtualProtect(slot as *mut c_void, 8, old, &mut tmp);
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "system" {
        fn GetCurrentProcessorNumber() -> u32;
    }
    extern "system" fn fake_processor_number() -> u32 {
        0x5eed
    }

    fn real_processor_number() -> usize {
        unsafe { crate::net_guard::resolve_export(b"kernel32.dll\0", b"GetCurrentProcessorNumber\0") as usize }
    }

    #[test]
    fn finds_the_bound_slot_of_an_import_of_this_binary() {
        let base = unsafe { GetModuleHandleW(core::ptr::null()) } as *const u8;
        let slots = unsafe { slots_bound_to(base, real_processor_number()) };
        assert!(!slots.is_empty(), "the test binary imports GetCurrentProcessorNumber through its IAT");
        assert!(unsafe { slots_bound_to(base, 1) }.is_empty(), "no slot is bound to address 1");
    }

    #[test]
    fn redirected_calls_reach_the_replacement_and_restore() {
        let real = real_processor_number();
        assert_ne!(unsafe { GetCurrentProcessorNumber() }, 0x5eed);
        let n = unsafe { redirect_main_module(real, fake_processor_number as *const () as usize) };
        assert!(n >= 1);
        let seen = unsafe { GetCurrentProcessorNumber() };
        assert_eq!(unsafe { redirect_main_module(fake_processor_number as *const () as usize, real) }, n);
        assert_eq!(seen, 0x5eed, "the call went through the rewritten slot");
        assert_ne!(unsafe { GetCurrentProcessorNumber() }, 0x5eed, "restored");
    }

    #[test]
    fn rejects_images_that_are_not_pe32_plus() {
        let junk = [0u8; 256];
        assert!(unsafe { slots_bound_to(junk.as_ptr(), 0) }.is_empty());
    }
}
