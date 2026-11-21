use crate::api::*;
use crate::conn::tests::{init, NET_LOCK};
use crate::dispatcher::IID_XBTP_DISPATCHER;
use crate::dispatcher_com::DispatcherCom;
use crate::handshake::{deserialize_json_message, SYSTEM_NOT_READY};
use crate::service_info::*;
use crate::session_impl::{SessionImplVtbl, IID_XBTP_CLIENT_SESSION};
use crate::start::*;
use crate::sys::*;
use crate::xbtp2_ext::{com_release, new_message, Message};
use crate::xbtp2_json::{ArrVtbl, JsonArray};
use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::Ordering;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: HANDLE) -> HANDLE;
}

unsafe fn json_text(m: *mut c_void) -> String {
    let m = m as *mut Message;
    let mut n = 0u32;
    let mut p: *mut u8 = null_mut();
    assert_eq!(((*(*m).vtbl).get_json_body)(m, &mut n, &mut p), S_OK);
    if p.is_null() {
        return String::new();
    }
    String::from_utf8_lossy(core::slice::from_raw_parts(p, n as usize)).trim_end_matches('\0').to_string()
}

unsafe fn set_json(m: *mut c_void, text: &str) {
    let m = m as *mut Message;
    let w = wide(text);
    assert_eq!(((*(*m).vtbl).set_json_body_w)(m, w.as_ptr()), S_OK);
}

unsafe fn open(port: u32) -> (HRESULT, *mut c_void) {
    let mut o = XBTP_OPEN_SESSION_OPTIONS::default();
    o.quiet = 1;
    o.timeout_ms = 5000;
    let host = wide("localhost");
    let mut s: *mut c_void = null_mut();
    let hr = XbtpOpenSessionEx2(host.as_ptr(), port, &o, &SERVICE_INFO_EXTENSION, &IID_XBTP_CLIENT_SESSION, &mut s);
    (hr, s)
}

unsafe fn call(s: *mut c_void, op: u32, body: &str) -> (i32, String) {
    let v = &**(s as *const *const SessionImplVtbl);
    let m = new_message() as *mut c_void;
    set_json(m, body);
    let mut ex = 0i32;
    assert_eq!((v.send_begin_message)(s as *mut _, op, m, &mut ex), S_OK);
    com_release(m);
    let mut status = 1i32;
    let mut reply: *mut c_void = null_mut();
    assert_eq!((v.receive_end_message)(s as *mut _, 10000, ex, &mut status, &mut reply), S_OK);
    let t = if reply.is_null() { String::new() } else { json_text(reply) };
    if !reply.is_null() {
        com_release(reply);
    }
    (status, t)
}

unsafe fn start(port: u32, system: i32) -> *mut DispatcherCom {
    let mut out: *mut c_void = null_mut();
    assert_eq!(XbtpStartDispatcherEx(port, system, &IID_XBTP_DISPATCHER, &mut out), S_OK);
    assert!(!out.is_null());
    out as *mut DispatcherCom
}

unsafe fn stop(c: *mut DispatcherCom) {
    assert_eq!(((*(*c).vtbl).shutdown)(c), S_OK);
    ((*(*c).vtbl).release)(c);
}

#[test]
fn guid_text_matches_console_format() {
    assert_eq!(guid_text(&SERVICE_INFO_EXTENSION), "206A9D73-95E4-4A5D-93C2-133C3D924600");
}

#[test]
fn start_dispatcher_ex_serves_service_info_operations() {
    let _g = NET_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    unsafe {
        let c = start(47921, 0);
        let (hr, s) = open(47921);
        assert_eq!(hr, S_OK);
        let (st, t) = call(s, VERSION_OPERATION, "{}");
        assert_eq!(st, 0);
        assert!(t.contains("\"Version\":\"Game OS\""), "{}", t);
        assert!(t.contains("\"Platform\":"), "{}", t);
        assert!(t.contains("\"SystemUptimeHDW\":"), "{}", t);
        assert!(t.contains("\"SystemUptimeLDW\":"), "{}", t);
        let (st, t) = call(s, VM_READY_OPERATION, "{}");
        assert_eq!((st, t.as_str()), (0, "{\"VMReady\":1}"));
        SYSTEM_NOT_READY.store(1, Ordering::SeqCst);
        let (st, t) = call(s, VM_READY_OPERATION, "{}");
        SYSTEM_NOT_READY.store(0, Ordering::SeqCst);
        assert_eq!((st, t.as_str()), (0, "{\"VMReady\":0}"));
        let (st, t) = call(s, EXTENSION_VERSION_OPERATION, "{}");
        assert_eq!(st, 0);
        let exe = std::env::current_exe().unwrap().to_string_lossy().replace('\\', "\\\\");
        assert!(t.contains("\"Process\":{"), "{}", t);
        assert!(t.contains(&format!("\"Path\":\"{}\"", exe)) || !t.contains("\"Path\""), "{}", t);
        assert!(t.contains("\"KernelVersion\":"), "{}", t);
        assert!(t.contains("\"Extensions\":["), "{}", t);
        let (st, _) = call(s, 4, "{}");
        assert_eq!(st, -5);
        let (st, _) = call(s, TRANSPORT_TEST_OPERATION, "{\"LoopCount\":1,\"BufferSize\":1048577}");
        assert_eq!(st, -8);
        let (st, _) = call(s, TRANSPORT_TEST_OPERATION, "{\"LoopCount\":134217729,\"BufferSize\":1}");
        assert_eq!(st, -8);
        let (st, _) = call(s, TRANSPORT_TEST_OPERATION, "{\"LoopCount\":0,\"BufferSize\":1048576}");
        assert_eq!(st, 0);
        com_release(s);
        stop(c);
    }
}

#[test]
fn transport_test_operation_echoes_binary_stream() {
    let _g = NET_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    unsafe {
        let c = start(47923, 0);
        let (hr, s) = open(47923);
        assert_eq!(hr, S_OK);
        let v = &**(s as *const *const SessionImplVtbl);
        let m = new_message() as *mut c_void;
        set_json(m, "{\"LoopCount\":3,\"BufferSize\":300}");
        let mut ex = 0i32;
        assert_eq!((v.send_begin_message)(s as *mut _, TRANSPORT_TEST_OPERATION, m, &mut ex), S_OK);
        com_release(m);
        let payload: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
        for _ in 0..3 {
            let mm = new_message();
            assert_eq!(((*(*mm).vtbl).set_binary_body_copy)(mm, 300, payload.as_ptr()), S_OK);
            assert_eq!((v.send_more_message)(s as *mut _, ex, mm as *mut c_void), S_OK);
            com_release(mm as *mut c_void);
        }
        for _ in 0..3 {
            let mut r: *mut c_void = null_mut();
            assert_eq!((v.receive_more_message)(s as *mut _, 10000, ex, &mut r), S_OK);
            let rm = r as *mut Message;
            let mut n = 0u32;
            let mut p: *mut u8 = null_mut();
            assert_eq!(((*(*rm).vtbl).get_binary_body)(rm, &mut n, &mut p), S_OK);
            assert_eq!(n, 300);
            let got = core::slice::from_raw_parts(p, n as usize);
            assert!(got.iter().enumerate().all(|(i, b)| *b == i as u8));
            com_release(r);
        }
        let mut status = 1i32;
        let mut reply: *mut c_void = null_mut();
        assert_eq!((v.receive_end_message)(s as *mut _, 10000, ex, &mut status, &mut reply), S_OK);
        assert_eq!(status, 0);
        if !reply.is_null() {
            com_release(reply);
        }
        let m = new_message() as *mut c_void;
        set_json(m, "{\"LoopCount\":1,\"BufferSize\":8}");
        let mut ex = 0i32;
        assert_eq!((v.send_begin_message)(s as *mut _, TRANSPORT_TEST_OPERATION, m, &mut ex), S_OK);
        com_release(m);
        let mm = new_message();
        assert_eq!(((*(*mm).vtbl).set_binary_body_copy)(mm, 5, payload.as_ptr()), S_OK);
        assert_eq!((v.send_more_message)(s as *mut _, ex, mm as *mut c_void), S_OK);
        com_release(mm as *mut c_void);
        let mut status = 1i32;
        let mut reply: *mut c_void = null_mut();
        assert_eq!((v.receive_end_message)(s as *mut _, 10000, ex, &mut status, &mut reply), S_OK);
        assert_eq!(status, -8);
        if !reply.is_null() {
            com_release(reply);
        }
        com_release(s);
        stop(c);
    }
}

#[test]
fn start_dispatcher_argument_and_port_errors() {
    let _g = NET_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    unsafe {
        assert_eq!(XbtpStartDispatcherEx(47925, 0, &IID_XBTP_DISPATCHER, null_mut()), E_INVALIDARG);
        let mut out: *mut c_void = null_mut();
        assert_eq!(XbtpStartDispatcher(2, 0, &IID_XBTP_DISPATCHER, &mut out), E_INVALIDARG);
        assert!(out.is_null());
        let c = start(47925, 0);
        let mut again: *mut c_void = null_mut();
        let hr = XbtpStartDispatcherEx(47925, 0, &IID_XBTP_DISPATCHER, &mut again);
        assert_eq!(hr, S_OK);
        assert!(!again.is_null());
        stop(again as *mut DispatcherCom);
        let (hr, s) = open(47925);
        assert_eq!(hr, S_OK);
        let (st, _) = call(s, VM_READY_OPERATION, "{}");
        assert_eq!(st, 0);
        com_release(s);
        let bad = GUID::new(1, 2, 3, [4; 8]);
        let mut other: *mut c_void = null_mut();
        assert_eq!(XbtpStartDispatcherEx(47927, 0, &bad, &mut other), E_NOINTERFACE);
        assert!(other.is_null());
        stop(c);
    }
}

#[test]
fn system_server_stops_at_first_missing_vm_pipe() {
    let _g = NET_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    unsafe {
        let c = start(47929, 1);
        let d = crate::dispatcher::dispatcher_impl((*c).dispatcher.ptr).unwrap();
        assert_eq!(d.pipes.lock().unwrap().len(), 0);
        drop(d);
        let name = wide(XVM_PIPE_NAMES[1]);
        let h = CreateFileW(name.as_ptr(), 0xC000_0000, 0, null_mut(), 3, 0, null_mut());
        assert_eq!(h as isize, -1);
        let (hr, s) = open(47929);
        assert_eq!(hr, S_OK);
        let (st, _) = call(s, VM_READY_OPERATION, "{}");
        assert_eq!(st, 0);
        com_release(s);
        stop(c);
    }
}

#[test]
fn fill_module_data_reports_version_strings_for_system_dll() {
    unsafe {
        let o = crate::handshake::Json::new().unwrap();
        let k = wide("C:\\Windows\\System32\\kernel32.dll");
        assert_eq!(fill_module_data(&k[..k.len() - 1], &o), S_OK);
        let t = o.stringify().unwrap().text();
        assert!(t.contains("\"FileVersion\":"), "{}", t);
        assert!(t.contains("\"Path\":"), "{}", t);
        let missing = wide("Z:\\nope\\missing_module.dll");
        let o2 = crate::handshake::Json::new().unwrap();
        assert_eq!(fill_module_data(&missing[..missing.len() - 1], &o2), S_OK);
        assert_eq!(o2.stringify().unwrap().text(), "{}");
        let arr = JsonArr::new().unwrap();
        assert_eq!(arr.add_object(&o), S_OK);
        let vt = &**(arr.0 as *const *const ArrVtbl);
        let mut n = 0u32;
        assert_eq!((vt.get_count)(arr.0 as *mut JsonArray, &mut n), S_OK);
        assert_eq!(n, 1);
        let m = new_message() as *mut c_void;
        set_json(m, "{\"a\":1}");
        assert!(deserialize_json_message(m).is_ok());
        com_release(m);
    }
}

#[test]
fn dispatcher_serves_ipv6_and_ipv4_loopback_on_one_port() {
    let _g = NET_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    unsafe {
        let c = start(47931, 0);
        let mut o = XBTP_OPEN_SESSION_OPTIONS::default();
        o.quiet = 1;
        o.timeout_ms = 3000;
        for address in ["::1", "127.0.0.1", "::1"] {
            let host = wide(address);
            let mut session = null_mut();
            assert_eq!(XbtpOpenSessionEx2(host.as_ptr(), 47931, &o, &SERVICE_INFO_EXTENSION, &IID_XBTP_CLIENT_SESSION, &mut session), S_OK, "{address}");
            assert_eq!(call(session, VM_READY_OPERATION, "{}").0, 0);
            com_release(session);
        }
        stop(c);
    }
}
