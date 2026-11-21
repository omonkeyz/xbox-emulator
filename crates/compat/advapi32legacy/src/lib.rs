#![allow(non_snake_case)]

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type NtStatus = i32;

pub const STATUS_SUCCESS: NtStatus = 0;
pub const STATUS_INVALID_HANDLE: NtStatus = 0xC000_0008u32 as i32;
pub const STATUS_INVALID_PARAMETER: NtStatus = 0xC000_000Du32 as i32;
pub const STATUS_ACCESS_DENIED: NtStatus = 0xC000_0022u32 as i32;

const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;

#[repr(C)]
pub struct UnicodeString {
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: *const u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
}

pub mod passwordless;
pub mod pc;

#[link(name = "ntdll")]
extern "system" {
    fn RtlGetLastNtStatus() -> NtStatus;
    fn RtlNtStatusToDosError(status: NtStatus) -> u32;
}

struct TebState {
    error: u32,
    status: NtStatus,
}

impl TebState {
    unsafe fn save_and_arm() -> Self {
        let s = TebState { error: GetLastError(), status: RtlGetLastNtStatus() };
        RtlNtStatusToDosError(STATUS_SUCCESS);
        s
    }

    unsafe fn failure_status(&self, early: &[(u32, NtStatus)]) -> NtStatus {
        let st = RtlGetLastNtStatus();
        if st < 0 {
            return st;
        }
        let err = GetLastError();
        early.iter().find(|(e, _)| *e == err).map(|(_, s)| *s).unwrap_or(STATUS_INVALID_PARAMETER)
    }

    unsafe fn restore(self) {
        RtlNtStatusToDosError(self.status);
        SetLastError(self.error);
    }
}

pub unsafe fn counted_to_wide(s: *const UnicodeString) -> Option<Vec<u16>> {
    if s.is_null() || (*s).buffer.is_null() {
        return None;
    }
    let n = (*s).length as usize / 2;
    let mut v = Vec::with_capacity(n + 1);
    v.extend_from_slice(core::slice::from_raw_parts((*s).buffer, n));
    v.push(0);
    Some(v)
}

#[no_mangle]
pub unsafe extern "system" fn ElfRegisterEventSourceW(unc_server: *const UnicodeString, module: *const UnicodeString, out: *mut Handle) -> NtStatus {
    if out.is_null() || module.is_null() || (*module).length == 0 {
        return STATUS_INVALID_PARAMETER;
    }
    let server = if !unc_server.is_null() && (*unc_server).length > 1 { counted_to_wide(unc_server) } else { None };
    let name = counted_to_wide(module).unwrap_or_else(|| vec![0]);
    let Some(register) = pc::pc().register else { return pc::missing_status() };
    let teb = TebState::save_and_arm();
    let h = register(server.as_ref().map_or(core::ptr::null(), |v| v.as_ptr()), name.as_ptr());
    let st = if h.is_null() { teb.failure_status(&[]) } else { STATUS_SUCCESS };
    teb.restore();
    *out = h;
    st
}

#[no_mangle]
pub unsafe extern "system" fn ElfDeregisterEventSource(h: Handle) -> NtStatus {
    let Some(deregister) = pc::pc().deregister else { return pc::missing_status() };
    let teb = TebState::save_and_arm();
    let st = if deregister(h) != 0 { STATUS_SUCCESS } else { teb.failure_status(&[(ERROR_INVALID_HANDLE, STATUS_INVALID_HANDLE)]) };
    teb.restore();
    st
}

#[no_mangle]
pub unsafe extern "system" fn ElfReportEventW(
    h: Handle,
    event_type: u16,
    category: u16,
    event_id: u32,
    user_sid: *mut c_void,
    num_strings: u16,
    data_size: u32,
    strings: *const *const UnicodeString,
    data: *const c_void,
    _flags: u16,
    _record_number: *mut u32,
    _time_written: *mut u32,
) -> NtStatus {
    if h.is_null() {
        return STATUS_INVALID_HANDLE;
    }
    let owned: Vec<Option<Vec<u16>>> = if strings.is_null() {
        Vec::new()
    } else {
        (0..num_strings as usize).map(|i| counted_to_wide(*strings.add(i))).collect()
    };
    let ptrs: Vec<*const u16> = owned.iter().map(|s| s.as_ref().map_or(core::ptr::null(), |v| v.as_ptr())).collect();
    let arr = if strings.is_null() { core::ptr::null() } else { ptrs.as_ptr() };
    let Some(report) = pc::pc().report else { return pc::missing_status() };
    let teb = TebState::save_and_arm();
    let ok = report(h, event_type, category, event_id, user_sid, num_strings, data_size, arr, data);
    let st = if ok != 0 {
        STATUS_SUCCESS
    } else {
        teb.failure_status(&[(ERROR_INVALID_HANDLE, STATUS_INVALID_HANDLE), (ERROR_INVALID_PARAMETER, STATUS_INVALID_PARAMETER)])
    };
    teb.restore();
    st
}

#[no_mangle]
pub unsafe extern "system" fn ElfFlushEventLog(_h: Handle) -> NtStatus {
    STATUS_ACCESS_DENIED
}

#[cfg(test)]
mod tests {
    use super::*;

    fn us(v: &[u16]) -> UnicodeString {
        UnicodeString { length: (v.len() * 2) as u16, maximum_length: (v.len() * 2) as u16, buffer: v.as_ptr() }
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn pc_functions_come_from_the_system32_path_or_the_staged_copy() {
        let p = String::from_utf16_lossy(&pc::system32_advapi32().unwrap());
        assert!(p.to_ascii_lowercase().ends_with("\\system32\\advapi32.dll\0"));
        let r = pc::pc();
        assert_ne!(r.module, 0);
        assert!(r.register.is_some() && r.deregister.is_some() && r.report.is_some());
        assert_eq!(pc::STATUS_DLL_NOT_FOUND, 0xC000_0135u32 as i32);
        assert_eq!(pc::STATUS_ENTRYPOINT_NOT_FOUND, 0xC000_0139u32 as i32);
        assert_eq!(pc::missing_status(), pc::STATUS_ENTRYPOINT_NOT_FOUND);
    }

    #[test]
    fn flush_is_access_denied() {
        unsafe {
            assert_eq!(ElfFlushEventLog(core::ptr::null_mut()), STATUS_ACCESS_DENIED);
            assert_eq!(ElfFlushEventLog(1 as Handle), STATUS_ACCESS_DENIED);
        }
    }

    #[test]
    fn register_rejects_bad_parameters() {
        unsafe {
            let name = w("TomBoxA32l");
            let m = us(&name);
            let empty = UnicodeString { length: 0, maximum_length: 0, buffer: name.as_ptr() };
            let mut h: Handle = 7 as Handle;
            assert_eq!(ElfRegisterEventSourceW(core::ptr::null(), &m, core::ptr::null_mut()), STATUS_INVALID_PARAMETER);
            assert_eq!(ElfRegisterEventSourceW(core::ptr::null(), core::ptr::null(), &mut h), STATUS_INVALID_PARAMETER);
            assert_eq!(ElfRegisterEventSourceW(core::ptr::null(), &empty, &mut h), STATUS_INVALID_PARAMETER);
            assert_eq!(h, 7 as Handle);
        }
    }

    #[test]
    fn register_and_deregister_round_trip_keeps_last_error() {
        unsafe {
            let name = w("TomBoxA32lUnitTestSourceName");
            let m = UnicodeString { length: 20, maximum_length: 20, buffer: name.as_ptr() };
            let one = w("x");
            let short_server = UnicodeString { length: 1, maximum_length: 2, buffer: one.as_ptr() };
            let mut h: Handle = core::ptr::null_mut();
            SetLastError(0x1234);
            assert_eq!(ElfRegisterEventSourceW(&short_server, &m, &mut h), STATUS_SUCCESS);
            assert!(!h.is_null());
            assert_eq!(GetLastError(), 0x1234);
            assert_eq!(ElfDeregisterEventSource(h), STATUS_SUCCESS);
            assert_eq!(GetLastError(), 0x1234);
        }
    }

    #[test]
    fn deregister_bad_handles_are_invalid_handle() {
        unsafe {
            assert_eq!(ElfDeregisterEventSource(core::ptr::null_mut()), STATUS_INVALID_HANDLE);
            let mut fake = [0u64; 16];
            assert_eq!(ElfDeregisterEventSource(fake.as_mut_ptr() as Handle), STATUS_INVALID_HANDLE);
        }
    }

    #[test]
    fn report_handle_and_parameter_errors() {
        unsafe {
            let n = core::ptr::null_mut();
            assert_eq!(ElfReportEventW(n, 4, 0, 1, n, 0, 0, core::ptr::null(), core::ptr::null(), 0, n as _, n as _), STATUS_INVALID_HANDLE);
            let mut fake = [0u64; 16];
            assert_eq!(ElfReportEventW(fake.as_mut_ptr() as Handle, 4, 0, 1, n, 0, 0, core::ptr::null(), core::ptr::null(), 0, n as _, n as _), STATUS_INVALID_HANDLE);
            let name = w("TomBoxA32lUnitTestSourceName");
            let m = us(&name);
            let mut h: Handle = core::ptr::null_mut();
            assert_eq!(ElfRegisterEventSourceW(core::ptr::null(), &m, &mut h), STATUS_SUCCESS);
            SetLastError(0x55);
            assert_eq!(ElfReportEventW(h, 4, 0, 1, n, 2, 0, core::ptr::null(), core::ptr::null(), 0, n as _, n as _), STATUS_INVALID_PARAMETER);
            assert_eq!(GetLastError(), 0x55);
            assert_eq!(ElfDeregisterEventSource(h), STATUS_SUCCESS);
        }
    }

    #[test]
    fn counted_strings_copy_by_length() {
        unsafe {
            let v = w("abcdef");
            let s = UnicodeString { length: 6, maximum_length: 12, buffer: v.as_ptr() };
            assert_eq!(counted_to_wide(&s).unwrap(), w("abc\0"));
            let e = UnicodeString { length: 0, maximum_length: 0, buffer: v.as_ptr() };
            assert_eq!(counted_to_wide(&e).unwrap(), vec![0]);
            let z = UnicodeString { length: 4, maximum_length: 4, buffer: core::ptr::null() };
            assert!(counted_to_wide(&z).is_none());
            assert!(counted_to_wide(core::ptr::null()).is_none());
        }
    }
}

#[cfg(test)]
mod tests_passwordless;
