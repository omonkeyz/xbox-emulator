mod common;

use common::*;
use core::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn SetDllDirectoryW(p: *const u16) -> i32;
    fn GetModuleFileNameW(m: *mut c_void, buf: *mut u16, n: u32) -> u32;
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[test]
fn elf_calls_work_when_the_compat_dll_is_loaded_as_advapi32() {
    let dir = std::env::temp_dir().join(format!("tombox-a32-real-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(built("advapi32.dll"), dir.join("advapi32.dll")).unwrap();
    std::fs::copy(built("advapi32legacy.dll"), dir.join("advapi32legacy.dll")).unwrap();
    std::fs::copy(std::path::PathBuf::from(std::env::var("WINDIR").unwrap()).join("System32").join("advapi32.dll"), dir.join("advapi32_sys.dll")).unwrap();
    unsafe {
        assert_ne!(SetDllDirectoryW(wide(dir.to_str().unwrap()).as_ptr()), 0);
        let h = LoadLibraryW(wide(dir.join("advapi32.dll").to_str().unwrap()).as_ptr());
        assert!(!h.is_null());
        let mut b = [0u16; 520];
        let n = GetModuleFileNameW(h, b.as_mut_ptr(), 520) as usize;
        assert!(String::from_utf16_lossy(&b[..n]).to_ascii_lowercase().starts_with(&dir.to_string_lossy().to_ascii_lowercase()));
        let reg = proc(h, "ElfRegisterEventSourceW");
        assert!(!reg.is_null(), "ElfRegisterEventSourceW must resolve through advapi32legacy while the compat DLL owns the advapi32 name");
        let reg: unsafe extern "system" fn(*const UnicodeString, *const UnicodeString, *mut *mut c_void) -> i32 = core::mem::transmute(reg);
        let dereg: unsafe extern "system" fn(*mut c_void) -> i32 = core::mem::transmute(proc(h, "ElfDeregisterEventSource"));
        let report: unsafe extern "system" fn(*mut c_void, u16, u16, u32, *mut c_void, u16, u32, *const *const UnicodeString, *const c_void, u16, *mut u32, *mut u32) -> i32 = core::mem::transmute(proc(h, "ElfReportEventW"));
        let name = wide("Application");
        let us = UnicodeString { length: ((name.len() - 1) * 2) as u16, maximum_length: (name.len() * 2) as u16, buffer: name.as_ptr() };
        let mut ev = core::ptr::null_mut();
        assert_eq!(reg(core::ptr::null(), &us, &mut ev), 0);
        assert!(!ev.is_null());
        assert_eq!(report(core::ptr::null_mut(), 4, 0, 1, core::ptr::null_mut(), 0, 0, core::ptr::null(), core::ptr::null(), 0, core::ptr::null_mut(), core::ptr::null_mut()), 0xC000_0008u32 as i32);
        assert_eq!(dereg(ev), 0);
        let enable: unsafe extern "system" fn() -> i32 = core::mem::transmute(proc(h, "LsaEnablePasswordLessCurrentUser"));
        let disable: unsafe extern "system" fn() -> i32 = core::mem::transmute(proc(h, "LsaDisablePasswordLessCurrentUser"));
        assert_eq!(enable(), 0);
        assert_eq!(disable(), 0);
        let staged = proc(system("advapi32_sys.dll"), "RegisterEventSourceW");
        assert!(!staged.is_null());
    }
}
