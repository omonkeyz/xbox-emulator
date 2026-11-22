use crate::com::{self, json, Com, Obj};
use crate::sys::*;
use crate::xbtp;

pub const API_CPP: &[u8] = b"xbox\\devex\\xtf\\transport\\xtfmessaging\\api.cpp\0";
pub const ERRCTX_CPP: &[u8] = b"xbox\\devex\\xtf\\transport\\xtfmessaging\\xtfoperationerrorcontext.cpp\0";
pub const LOG_FMT: &[u8] = b"%ws 0x%X (%s %d)\0";
pub const S_E_INVALIDARG: &[u8] = b"E_INVALIDARG\0";

pub fn log_api(expr: &[u8], hr: HRESULT, file: &[u8], line: u32) {
    unsafe { xbtp::log_info(LOG_FMT.as_ptr(), &[expr.as_ptr() as usize, hr as u32 as usize, file.as_ptr() as usize, line as usize]) };
}

pub unsafe fn setup_json_body(unparsed: *const u8, out: *mut Obj) -> HRESULT {
    if unparsed.is_null() {
        let hr = xbtp::create_json_object(out);
        if hr < 0 {
            log_api(b"XbtpCreateJsonObject(body)\0", hr, API_CPP, 0x7c);
            return hr;
        }
        return S_OK;
    }
    let hr = xbtp::get_json_object_from_utf8(unparsed, out);
    if hr < 0 {
        log_api(b"XbtpGetJsonObjectFromUtf8(unparsedBody, body)\0", hr, API_CPP, 0x80);
        return hr;
    }
    S_OK
}

unsafe fn deserialize(get: unsafe fn(Obj, *mut u32, *mut *const u8) -> HRESULT, message: Obj, out: *mut Obj, lines: [u32; 3]) -> HRESULT {
    if message.is_null() || out.is_null() {
        log_api(S_E_INVALIDARG, E_INVALIDARG, API_CPP, lines[0]);
        return E_INVALIDARG;
    }
    let mut len = 0u32;
    let mut body: *const u8 = core::ptr::null();
    let hr = get(message, &mut len, &mut body);
    if hr < 0 {
        log_api(b"message->GetJsonBody(&length, &unparsedBody)\0", hr, API_CPP, lines[1]);
        return hr;
    }
    let hr = setup_json_body(body, out);
    if hr < 0 {
        log_api(b"SetupJsonBody(unparsedBody, body)\0", hr, API_CPP, lines[2]);
        return hr;
    }
    S_OK
}

pub unsafe fn deserialize_body(message: Obj, out: *mut Obj) -> HRESULT {
    deserialize(com::message::get_json_body, message, out, [0x1f5, 0x1fa, 0x1fb])
}

pub unsafe fn deserialize_end_body(message: Obj, out: *mut Obj) -> HRESULT {
    deserialize(com::end_message::get_json_body, message, out, [0x20d, 0x212, 0x213])
}

unsafe fn serialize(set: unsafe fn(Obj, *const u16) -> HRESULT, body: Obj, message: Obj, lines: [u32; 3]) -> HRESULT {
    if body.is_null() || message.is_null() {
        log_api(S_E_INVALIDARG, E_INVALIDARG, API_CPP, lines[0]);
        return E_INVALIDARG;
    }
    let mut w: BSTR = core::ptr::null_mut();
    let mut hr = json::stringify(body, &mut w);
    if hr < 0 {
        log_api(b"body->Stringify(&wbody)\0", hr, API_CPP, lines[1]);
    } else {
        hr = set(message, w);
        if hr < 0 {
            log_api(b"message->SetJsonBodyW(wbody)\0", hr, API_CPP, lines[2]);
        } else {
            hr = S_OK;
        }
    }
    if !w.is_null() {
        SysFreeString(w);
    }
    hr
}

pub unsafe fn serialize_end_body(body: Obj, message: Obj) -> HRESULT {
    serialize(com::end_message::set_json_body_w, body, message, [0x225, 0x228, 0x229])
}

pub unsafe fn serialize_body(body: Obj, message: Obj) -> HRESULT {
    serialize(com::message::set_json_body_w, body, message, [0x23b, 0x23e, 0x23f])
}

pub unsafe fn send_more_json_message(session: Obj, body: Obj) -> HRESULT {
    if session.is_null() || body.is_null() {
        log_api(S_E_INVALIDARG, E_INVALIDARG, API_CPP, 0x2e6);
        return E_INVALIDARG;
    }
    let mut m = Com::null();
    let hr = xbtp::create_message(&com::IID_XBTP_MESSAGE, m.put());
    if hr < 0 {
        log_api(b"XbtpCreateMessage(IID_PPV_ARGS(&message))\0", hr, API_CPP, 0x2e9);
        return hr;
    }
    let hr = serialize_body(body, m.get());
    if hr < 0 {
        log_api(b"XtfSerializeJsonBody(body, message)\0", hr, API_CPP, 0x2ea);
        return hr;
    }
    let hr = com::session::send_more_message(session, m.get());
    if hr < 0 {
        log_api(b"session->SendMoreMessage(message)\0", hr, API_CPP, 0x2eb);
        return hr;
    }
    S_OK
}

pub unsafe fn set_json_property64(body: Obj, name: *const u16, v: u64) -> HRESULT {
    if body.is_null() {
        crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfSetJsonProperty64", 19, "E_INVALIDARG", E_INVALIDARG, "");
        return E_INVALIDARG;
    }
    let mut num = Com::null();
    let hr = xbtp::create_json_object(num.put());
    crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfSetJsonProperty64", 22, "XbtpCreateJsonObject(&num)", hr, "");
    if hr < 0 {
        return hr;
    }
    let l = wide("L");
    let hr = json::set_number(num.get(), l.as_ptr(), (v as u32) as f64);
    crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfSetJsonProperty64", 26, "num->SetNumber(L\"L\", li.LowPart)", hr, "");
    if hr < 0 {
        return hr;
    }
    let h = wide("H");
    let hr = json::set_number(num.get(), h.as_ptr(), ((v >> 32) as u32 as i32) as f64);
    crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfSetJsonProperty64", 27, "num->SetNumber(L\"H\", li.HighPart)", hr, "");
    if hr < 0 {
        return hr;
    }
    let hr = json::set_object(body, name, num.get());
    crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfSetJsonProperty64", 28, "body->SetObject(propertyName, num)", hr, "");
    hr
}

// Build 19041 XtfGetJsonProperty64 (console-control RVA 0x91dc).
pub unsafe fn get_json_property64(body: Obj, name: *const u16, out: *mut u64) -> HRESULT {
    if body.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let mut number = Com::null();
    let hr = json::get_object(body, name, number.put());
    if hr < 0 {
        return hr;
    }
    let mut low = 0.0;
    let hr = json::get_number(number.get(), wide("L").as_ptr(), &mut low);
    if hr < 0 {
        return hr;
    }
    let mut high = 0.0;
    let hr = json::get_number(number.get(), wide("H").as_ptr(), &mut high);
    if hr >= 0 {
        *out = ((cvtt64(high) as u64) << 32) | u64::from(cvtt64(low) as u32);
    }
    hr
}

pub fn cvtt64(v: f64) -> i64 {
    if v.is_nan() || v >= 9.223372036854775808e18 || v < -9.223372036854775808e18 {
        i64::MIN
    } else {
        v as i64
    }
}

pub fn cvtt32(v: f64) -> i32 {
    if v.is_nan() || v >= 2147483648.0 || v <= -2147483649.0 {
        i32::MIN
    } else {
        v as i32
    }
}

pub const JSON_HELPERS_H: &str = "xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h";

pub unsafe fn get_number(body: Obj, name: *const u16, out: &mut f64) -> HRESULT {
    if body.is_null() {
        crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfGetJsonProperty", 111, "E_INVALIDARG", E_INVALIDARG, "");
        return E_INVALIDARG;
    }
    let mut v = 0f64;
    let hr = json::get_number(body, name, &mut v);
    if hr == E_JSON_NOT_FOUND {
        return hr;
    }
    crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfGetJsonProperty", 118, "hr", hr, "");
    if hr >= 0 {
        *out = v;
    }
    hr
}

pub unsafe fn get_json_u32(body: Obj, name: *const u16, out: &mut u32) -> HRESULT {
    let mut v = 0f64;
    let hr = get_number(body, name, &mut v);
    if hr >= 0 {
        *out = cvtt64(v) as u32;
    }
    hr
}

pub unsafe fn get_json_i32(body: Obj, name: *const u16, out: &mut i32) -> HRESULT {
    let mut v = 0f64;
    let hr = get_number(body, name, &mut v);
    if hr >= 0 {
        *out = cvtt32(v);
    }
    hr
}

pub unsafe fn get_json_bool(body: Obj, name: *const u16, out: *mut bool) -> HRESULT {
    if body.is_null() || out.is_null() {
        crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfGetJsonProperty", 136, "E_INVALIDARG", E_INVALIDARG, "");
        return E_INVALIDARG;
    }
    let mut v = 0f64;
    let hr = json::get_number(body, name, &mut v);
    if hr == E_JSON_NOT_FOUND {
        return hr;
    }
    crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfGetJsonProperty", 143, "hr", hr, "");
    if hr >= 0 {
        *out = v != 0.0;
    }
    hr
}

pub unsafe fn set_number(body: Obj, name: *const u16, v: f64) -> HRESULT {
    if body.is_null() {
        crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfSetJsonProperty", 92, "E_INVALIDARG", E_INVALIDARG, "");
        return E_INVALIDARG;
    }
    let hr = json::set_number(body, name, v);
    crate::xtfchk!("xbox\\devex\\xtf\\transport\\XtfMessaging\\XtfJsonHelpers.h", "XtfSetJsonProperty", 96, "body->SetNumber(propertyName, dval)", hr, "");
    hr
}

pub unsafe fn set_json_u32(body: Obj, name: *const u16, v: u32) -> HRESULT {
    set_number(body, name, v as f64)
}

pub unsafe fn set_json_i32(body: Obj, name: *const u16, v: i32) -> HRESULT {
    set_number(body, name, v as f64)
}

pub unsafe fn set_json_bool(body: Obj, name: *const u16, v: bool) -> HRESULT {
    set_number(body, name, if v { 1.0 } else { 0.0 })
}

#[repr(C)]
pub struct ErrorContext {
    pub errors: Obj,
    pub body: Obj,
    pub end_message: Obj,
    pub checkpoint: u32,
    pub hr: HRESULT,
    pub done: bool,
}

impl ErrorContext {
    pub fn new(end_message: Obj) -> ErrorContext {
        ErrorContext { errors: core::ptr::null_mut(), body: core::ptr::null_mut(), end_message, checkpoint: 0, hr: S_OK, done: false }
    }

    pub unsafe fn create_errors_array(&mut self) -> HRESULT {
        if !self.errors.is_null() {
            return S_OK;
        }
        let hr = xbtp::create_json_array(&mut self.errors);
        if hr < 0 {
            log_api(b"XbtpCreateJsonArray(&m_errors)\0", hr, ERRCTX_CPP, 0x94);
            return hr;
        }
        S_OK
    }

    pub unsafe fn append(&mut self, text: &str) -> HRESULT {
        if text.is_empty() {
            return E_INVALIDARG;
        }
        let w = wide(text);
        let scratch = SysAllocStringLen(w.as_ptr(), (w.len() - 1) as u32);
        if scratch.is_null() {
            return E_OUTOFMEMORY;
        }
        let mut hr = self.create_errors_array();
        if hr < 0 {
            log_api(b"CreateErrorsArray()\0", hr, ERRCTX_CPP, 0x84);
        } else {
            hr = com::array::append_string(self.errors, scratch);
            if hr < 0 {
                log_api(b"m_errors->AppendString(scratch.Ptr())\0", hr, ERRCTX_CPP, 0x85);
            }
        }
        SysFreeString(scratch);
        hr
    }

    pub unsafe fn append_error(&mut self, hr: HRESULT, file: &str, line: i32) -> HRESULT {
        self.append(&format!("Error 0x{:X} File {} : {}", hr as u32, file, line))
    }

    pub unsafe fn finalize(&mut self, hr: &mut HRESULT) {
        if self.end_message.is_null() {
            return;
        }
        if self.hr >= 0 && *hr >= 0 && self.body.is_null() {
            return;
        }
        let mut tmp = Com::null();
        let body = if self.body.is_null() {
            let r = deserialize_end_body(self.end_message, tmp.put());
            if r < 0 {
                log_api(b"XtfDeserializeJsonBody(m_endMessage, &bodyTmp)\0", r, ERRCTX_CPP, 0x47);
                return;
            }
            tmp.get()
        } else {
            self.body
        };
        let mut eb = Com::null();
        let r = xbtp::create_json_object(eb.put());
        if r < 0 {
            log_api(b"XbtpCreateJsonObject(&errorBody)\0", r, ERRCTX_CPP, 0x4a);
            return;
        }
        let code = if self.hr < 0 { self.hr } else { *hr };
        let rc = wide("ReturnCode");
        let r = json::set_number(eb.get(), rc.as_ptr(), code as f64);
        if r < 0 {
            log_api(b"errorBody->SetNumber(XbtpEndErrorReturnCodeName, error)\0", r, ERRCTX_CPP, 0x54);
            return;
        }
        let cp = wide("Checkpoint");
        let r = json::set_number(eb.get(), cp.as_ptr(), self.checkpoint as f64);
        if r < 0 {
            log_api(b"errorBody->SetNumber(XbtpEndErrorCheckpointName, m_checkpoint)\0", r, ERRCTX_CPP, 0x57);
            return;
        }
        let r = self.create_errors_array();
        if r < 0 {
            log_api(b"CreateErrorsArray()\0", r, ERRCTX_CPP, 0x5a);
            return;
        }
        let er = wide("Errors");
        let r = json::set_array(eb.get(), er.as_ptr(), self.errors);
        if r < 0 {
            log_api(b"errorBody->SetArray(XbtpEndErrorErrorsName, m_errors)\0", r, ERRCTX_CPP, 0x5d);
            return;
        }
        let oe = wide("XtfOperationError");
        let r = json::set_object(body, oe.as_ptr(), eb.get());
        if r < 0 {
            log_api(b"body->SetObject(XbtpEndErrorObjectName, errorBody)\0", r, ERRCTX_CPP, 0x60);
            return;
        }
        let r = serialize_end_body(body, self.end_message);
        if r < 0 {
            log_api(b"XtfSerializeJsonBody(body, m_endMessage)\0", r, ERRCTX_CPP, 0x61);
            return;
        }
        *hr = 0;
        self.done = true;
    }
}

impl Drop for ErrorContext {
    fn drop(&mut self) {
        unsafe {
            if !self.done {
                let mut h = self.hr;
                self.finalize(&mut h);
            }
            let b = core::mem::replace(&mut self.body, core::ptr::null_mut());
            if !b.is_null() {
                com::release(b);
            }
            let e = core::mem::replace(&mut self.errors, core::ptr::null_mut());
            if !e.is_null() {
                com::release(e);
            }
        }
    }
}
