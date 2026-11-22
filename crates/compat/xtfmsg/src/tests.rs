use crate::com::{self, json, Com};
use crate::fixture::*;
use crate::msg::*;
use crate::sys::*;
use crate::tlg::*;
use crate::xbtp;

fn reference(dll: &str) -> Vec<Vec<u8>> {
    xtfcommon::pe::title_dlls(dll).into_iter().filter(|(t, _)| t != "Fortnite").map(|(_, p)| std::fs::read(p).unwrap()).collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn event_metadata_matches_console_blobs() {
    let refs = reference("xtfremoterun.dll");
    if refs.is_empty() {
        return;
    }
    for img in &refs {
        for ev in [&HRESULT_FAIL, &HRESULT_OK, &ACTIVITY_START, &ACTIVITY_END] {
            let mut full = vec![11u8, ev.level, ev.opcode];
            full.extend_from_slice(&ev.keyword.to_le_bytes());
            full.extend_from_slice(&ev.metadata());
            assert!(contains(img, &full), "{} level {} not found", ev.name, ev.level);
        }
        let mut prov = guid_bytes(&CORE_DEV_TOOLS_TLG.guid).to_vec();
        prov.extend_from_slice(&CORE_DEV_TOOLS_TLG.traits());
        assert!(contains(img, &prov));
    }
    for img in reference("xtfdebugmonitor.dll") {
        let mut full = vec![11u8, 0, 0];
        full.extend_from_slice(&OPERATION_RESULT.keyword.to_le_bytes());
        full.extend_from_slice(&OPERATION_RESULT.metadata());
        assert!(contains(&img, &full));
        let mut prov = guid_bytes(&CORE_DEV_TOOLS.guid).to_vec();
        prov.extend_from_slice(&CORE_DEV_TOOLS.traits());
        assert!(contains(&img, &prov));
    }
}

#[test]
fn cvtt_matches_x86_truncation() {
    assert_eq!(cvtt64(4294967295.9), 4294967295);
    assert_eq!(cvtt64(f64::NAN), i64::MIN);
    assert_eq!(cvtt64(1e300) as u32, 0);
    assert_eq!(cvtt32(-1.5), -1);
    assert_eq!(cvtt32(3e9), i32::MIN);
}

#[test]
fn json_body_round_trip_and_property_helpers() {
    load_xbtp();
    unsafe {
        let m = message_with_json("{\"dwFlags\":3221225472,\"Name\":\"x\"}");
        let mut body = Com::null();
        assert_eq!(deserialize_body(m.get(), body.put()), S_OK);
        let mut v = 0u32;
        let n = wide("dwFlags");
        assert_eq!(get_json_u32(body.get(), n.as_ptr(), &mut v), S_OK);
        assert_eq!(v, 0xC000_0000);
        let missing = wide("Absent");
        let mut w = 7u32;
        assert_eq!(get_json_u32(body.get(), missing.as_ptr(), &mut w), E_JSON_NOT_FOUND);
        assert_eq!(w, 7);
        assert_eq!(get_json_u32(core::ptr::null_mut(), n.as_ptr(), &mut w), E_INVALIDARG);
        let ec = wide("ExitCode");
        assert_eq!(set_json_u32(body.get(), ec.as_ptr(), 0xFFFF_FFFF), S_OK);
        let out = new_message();
        assert_eq!(serialize_body(body.get(), out.get()), S_OK);
        let t = json_text(out.get()).unwrap();
        assert!(t.contains("\"ExitCode\":4294967295"), "{t}");
        let empty = new_message();
        let mut fresh = Com::null();
        assert_eq!(deserialize_body(empty.get(), fresh.put()), S_OK);
        assert!(!fresh.is_null());
        assert_eq!(deserialize_body(core::ptr::null_mut(), fresh.put()), E_INVALIDARG);
        assert_eq!(serialize_body(core::ptr::null_mut(), out.get()), E_INVALIDARG);
    }
}

#[test]
fn error_context_writes_operation_error_into_end_message() {
    load_xbtp();
    unsafe {
        let reply = end_message();
        {
            let mut ctx = ErrorContext::new(reply.get());
            ctx.append_error(E_INVALIDARG, "xbox\\devex\\xtf\\dll\\remoterun\\run.cpp", 925);
            ctx.hr = E_INVALIDARG;
        }
        let t = end_json(reply.get()).unwrap();
        let mut body = Com::null();
        let u = std::ffi::CString::new(t.clone()).unwrap();
        assert_eq!(xbtp::get_json_object_from_utf8(u.as_ptr() as *const u8, body.put()), S_OK);
        let mut eo = Com::null();
        let k = wide("XtfOperationError");
        assert_eq!(json::get_object(body.get(), k.as_ptr(), eo.put()), S_OK, "{t}");
        let mut rc = 0f64;
        let r = wide("ReturnCode");
        assert_eq!(json::get_number(eo.get(), r.as_ptr(), &mut rc), S_OK);
        assert_eq!(rc, E_INVALIDARG as f64);
        let mut arr = Com::null();
        let e = wide("Errors");
        assert_eq!(json::get_array(eo.get(), e.as_ptr(), arr.put()), S_OK);
        let mut s: BSTR = core::ptr::null_mut();
        assert_eq!(com::array::get_string(arr.get(), 0, &mut s), S_OK);
        assert_eq!(from_wide(s), "Error 0x80070057 File xbox\\devex\\xtf\\dll\\remoterun\\run.cpp : 925");
        SysFreeString(s);
        let ok = end_message();
        let body2 = message_with_json("{\"ExitCode\":0}");
        let mut b2 = Com::null();
        assert_eq!(deserialize_body(body2.get(), b2.put()), S_OK);
        assert_eq!(serialize_end_body(b2.get(), ok.get()), S_OK);
        drop(ErrorContext::new(ok.get()));
        assert_eq!(end_json(ok.get()).unwrap(), "{\"ExitCode\":0}");
    }
}

#[test]
fn send_more_json_message_serializes_through_session_slot_3() {
    load_xbtp();
    unsafe {
        let s = session();
        let mut body = Com::null();
        assert_eq!(xbtp::create_json_object(body.put()), S_OK);
        let k = wide("AttachResult");
        assert_eq!(set_json_i32(body.get(), k.as_ptr(), -2147024809), S_OK);
        assert_eq!(send_more_json_message(as_obj(s), body.get()), S_OK);
        assert_eq!((*s).sent.lock().unwrap()[0], "{\"AttachResult\":-2147024809}");
        assert_eq!(send_more_json_message(core::ptr::null_mut(), body.get()), E_INVALIDARG);
        assert_eq!(send_more_json_message(as_obj(s), core::ptr::null_mut()), E_INVALIDARG);
        assert_eq!((*s).sent.lock().unwrap().len(), 1);
        com::release(as_obj(s));
    }
}

#[test]
fn set_json_property64_splits_low_unsigned_high_signed() {
    load_xbtp();
    unsafe {
        let mut body = Com::null();
        assert_eq!(xbtp::create_json_object(body.put()), S_OK);
        let k = wide("Address");
        assert_eq!(set_json_property64(body.get(), k.as_ptr(), 0xFFFF_FFFF_8000_0001), S_OK);
        let out = new_message();
        assert_eq!(serialize_body(body.get(), out.get()), S_OK);
        assert_eq!(json_text(out.get()).unwrap(), "{\"Address\":{\"H\":-1,\"L\":2147483649}}");
        assert_eq!(set_json_property64(core::ptr::null_mut(), k.as_ptr(), 1), E_INVALIDARG);
    }
}

#[test]
fn xce_log_operation_result_needs_tls_event_data() {
    load_xbtp();
    unsafe {
        let ver = wide("220604");
        let empty = wide("");
        assert_eq!(crate::xce::log_operation_result(ver.as_ptr(), empty.as_ptr(), empty.as_ptr(), E_INVALIDARG), E_OUTOFMEMORY);
        crate::xce::write_operation_result(ver.as_ptr(), &GUID::new(1, 2, 3, [4; 8]), &GUID::new(5, 6, 7, [8; 8]), 3, empty.as_ptr(), empty.as_ptr(), E_INVALIDARG);
        assert_eq!(core::mem::size_of::<crate::xce::XceEventData>(), 0x24);
    }
}

unsafe extern "system" fn nop_handler(_s: usize, _r: usize, _e: usize) -> HRESULT {
    S_OK
}

#[test]
fn entry_point_with_runs_pre_step_after_guid_checks() {
    load_xbtp();
    static SITE: crate::entry::EntrySite = crate::entry::EntrySite { file: "f.cpp", func: "F", bad_extension_line: 1, bad_metadata_line: 2, create_line: 3, handlers: &[(7, nop_handler, "h", 4)], bad_extension_msg: "", bad_metadata_msg: "" };
    let want = GUID::new(9, 9, 9, [9; 8]);
    unsafe {
        let mut calls = 0;
        let mut out: com::Obj = core::ptr::null_mut();
        let bad = GUID::default();
        assert_eq!(crate::entry::extension_entry_point_with(&SITE, &want, &bad, &com::IID_XBTP_EXTENSION_METADATA, &mut out, &mut || { calls += 1; S_OK }), E_UNEXPECTED);
        assert_eq!(crate::entry::extension_entry_point_with(&SITE, &want, &want, &bad, &mut out, &mut || { calls += 1; S_OK }), E_UNEXPECTED);
        assert_eq!(calls, 0);
        assert_eq!(crate::entry::extension_entry_point_with(&SITE, &want, &want, &com::IID_XBTP_EXTENSION_METADATA, &mut out, &mut || { calls += 1; E_OUTOFMEMORY }), E_OUTOFMEMORY);
        assert!(out.is_null());
        assert_eq!(crate::entry::extension_entry_point_with(&SITE, &want, &want, &com::IID_XBTP_EXTENSION_METADATA, &mut out, &mut || { calls += 1; S_OK }), S_OK);
        assert_eq!(calls, 2);
        assert!(!out.is_null());
        com::release(out);
    }
}

#[test]
fn session_fixture_follows_console_slots() {
    load_xbtp();
    unsafe {
        let s = session();
        push_inbound(s, "{\"MsgId\":0}");
        let mut m = Com::null();
        assert_eq!(com::session::receive_more_message(as_obj(s), 0, m.put()), S_OK);
        assert_eq!(com::session::receive_more_message(as_obj(s), 0, m.put()), E_PENDING);
        let out = message_with_json("{\"a\":1}");
        assert_eq!(com::session::send_more_message(as_obj(s), out.get()), S_OK);
        assert_eq!((*s).sent.lock().unwrap()[0], "{\"a\":1}");
        com::release(as_obj(s));
    }
}

#[test]
fn get_json_bool_treats_nonzero_as_true() {
    load_xbtp();
    unsafe {
        let m = message_with_json("{\"Reboot\":1,\"Off\":0,\"Bad\":\"x\"}");
        let mut body = Com::null();
        assert_eq!(deserialize_body(m.get(), body.put()), S_OK);
        let mut b = false;
        let r = wide("Reboot");
        assert_eq!(get_json_bool(body.get(), r.as_ptr(), &mut b), S_OK);
        assert!(b);
        let o = wide("Off");
        assert_eq!(get_json_bool(body.get(), o.as_ptr(), &mut b), S_OK);
        assert!(!b);
        b = true;
        let a = wide("Absent");
        assert_eq!(get_json_bool(body.get(), a.as_ptr(), &mut b), E_JSON_NOT_FOUND);
        assert!(b);
        let x = wide("Bad");
        assert!(get_json_bool(body.get(), x.as_ptr(), &mut b) < 0);
        assert_eq!(get_json_bool(core::ptr::null_mut(), r.as_ptr(), &mut b), E_INVALIDARG);
        assert_eq!(get_json_bool(body.get(), r.as_ptr(), core::ptr::null_mut()), E_INVALIDARG);
    }
}

#[test]
fn send_json_message_follows_console_client_session() {
    load_xbtp();
    unsafe {
        use crate::api::*;
        let s = client_session(S_OK, S_OK, 0, Some("{\"Ack\":1}"));
        let mut reply = Com::null();
        assert_eq!(send_json_message(as_obj(s), core::ptr::null_mut(), 0xe, reply.put()), S_OK);
        assert!(!reply.is_null());
        let mut v = 0f64;
        let k = wide("Ack");
        assert_eq!(json::get_number(reply.get(), k.as_ptr(), &mut v), S_OK);
        assert_eq!(*(*s).calls.lock().unwrap(), vec![(0xe, 0, 0), (u32::MAX, 30000, 0x41)]);
        assert_eq!(send_json_message(as_obj(s), core::ptr::null_mut(), 0xf, core::ptr::null_mut()), S_OK);
        let request = message_with_json(r#"{"ProcessId":1234}"#);
        let mut body = Com::null();
        assert_eq!(deserialize_body(request.get(), body.put()), S_OK);
        assert_eq!(send_json_message(as_obj(s), body.get(), 0x10, core::ptr::null_mut()), S_OK);
        assert_eq!(*(*s).requests.lock().unwrap(), [None, None, Some(r#"{"ProcessId":1234}"#.to_owned())]);
        com::release(as_obj(s));
        let st = client_session(S_OK, S_OK, 0x1234, None);
        assert_eq!(send_json_message(as_obj(st), core::ptr::null_mut(), 0xe, core::ptr::null_mut()), 0x8c11_0134u32 as i32);
        com::release(as_obj(st));
        let pend = client_session(S_OK, E_PENDING, 0, None);
        assert_eq!(send_json_message(as_obj(pend), core::ptr::null_mut(), 0xe, core::ptr::null_mut()), E_TRANSPORT_TIMEOUT);
        com::release(as_obj(pend));
        let bad = client_session(E_FAIL, S_OK, 0, None);
        assert_eq!(send_json_message(as_obj(bad), core::ptr::null_mut(), 0xe, core::ptr::null_mut()), E_FAIL);
        assert_eq!((*bad).calls.lock().unwrap().len(), 1);
        com::release(as_obj(bad));
        assert_eq!(send_json_message(core::ptr::null_mut(), core::ptr::null_mut(), 0xe, core::ptr::null_mut()), E_INVALIDARG);
        assert_eq!(open_pipe_to_system(&GUID::default(), &crate::com::IID_XBTP_CLIENT_SESSION, core::ptr::null_mut()), E_INVALIDARG);
    }
}

#[test]
fn split_uint64_requires_both_words_before_writing_the_result() {
    load_xbtp();
    unsafe {
        let mut body = Com::null();
        let message = message_with_json(r#"{"Partial":{"L":7},"Signed":{"H":-2.5,"L":4294967295}}"#);
        assert_eq!(deserialize_body(message.get(), body.put()), S_OK);
        let mut value = 123;
        assert_eq!(get_json_property64(body.get(), wide("Partial").as_ptr(), &mut value), E_JSON_NOT_FOUND);
        assert_eq!(value, 123);
        assert_eq!(get_json_property64(body.get(), wide("Signed").as_ptr(), &mut value), S_OK);
        assert_eq!(value, 0xffff_fffe_ffff_ffff);
    }
}
