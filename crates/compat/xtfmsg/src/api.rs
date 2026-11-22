use crate::com::{self, Com, Obj, IID_XBTP_MESSAGE};
use crate::msg::{deserialize_body, log_api, serialize_body, API_CPP, S_E_INVALIDARG};
use crate::sys::*;
use crate::xbtp;

pub const DEFAULT_RECEIVE_TIMEOUT: u32 = 30000;
pub const E_TRANSPORT_TIMEOUT: HRESULT = 0x8c11_0410u32 as i32;

pub fn make_transport_status_error(status: u32) -> HRESULT {
    ((status & 0xff) | 0x8c11_0100) as i32
}

pub unsafe fn open_pipe_to_system(ext: *const GUID, iid: *const GUID, out: *mut Obj) -> HRESULT {
    if out.is_null() {
        log_api(S_E_INVALIDARG, E_INVALIDARG, API_CPP, 0x39f);
        return E_INVALIDARG;
    }
    *out = core::ptr::null_mut();
    let mut pipe: BSTR = core::ptr::null_mut();
    let mut hr = S_OK;
    let r = xbtp::get_pipe_name_to_system(&mut pipe);
    if r < 0 {
        log_api(b"XbtpGetPipeNameToSystem(&pipeName)\0", r, API_CPP, 0x3a4);
        hr = r;
    } else {
        let r = xbtp::open_pipe_session(pipe, ext, iid, out);
        if r < 0 {
            log_api(b"XbtpOpenPipeSession( pipeName, extensionId, iid, ppvObject)\0", r, API_CPP, 0x3aa);
            hr = r;
        }
    }
    if !pipe.is_null() {
        SysFreeString(pipe);
    }
    hr
}

pub unsafe fn send_json_message_timeout(session: Obj, op: u32, timeout: u32, reply: *mut Obj) -> HRESULT {
    send_json_message_with_timeout(session, core::ptr::null_mut(), op, timeout, reply)
}

pub unsafe fn send_json_message_with_timeout(session: Obj, request_body: Obj, op: u32, timeout: u32, reply: *mut Obj) -> HRESULT {
    if session.is_null() {
        log_api(S_E_INVALIDARG, E_INVALIDARG, API_CPP, 0x26c);
        return E_INVALIDARG;
    }
    let mut req = Com::null();
    let mut resp = Com::null();
    let mut body = Com::null();
    let fail = |hr: HRESULT, expr: &[u8], line: u32| -> HRESULT {
        log_api(expr, hr, API_CPP, line);
        if hr == E_PENDING {
            E_TRANSPORT_TIMEOUT
        } else {
            hr
        }
    };
    let hr = xbtp::create_message(&IID_XBTP_MESSAGE, req.put());
    if hr < 0 {
        return fail(hr, b"XbtpCreateMessage(IID_PPV_ARGS(&reqMsg))\0", 0x270);
    }
    if !request_body.is_null() {
        let hr = serialize_body(request_body, req.get());
        if hr < 0 {
            return fail(hr, b"XtfSerializeJsonBody(body, reqMsg)\0", 0x274);
        }
    }
    let mut exchange = 0u32;
    let hr = com::client_session::send_begin_message(session, op, req.get(), &mut exchange);
    if hr < 0 {
        return fail(hr, b"session->SendBeginMessage( operationId, reqMsg, &exchangeId)\0", 0x27c);
    }
    let mut status = 0u32;
    let hr = com::client_session::receive_end_message(session, timeout, exchange, &mut status, resp.put());
    if hr < 0 {
        return fail(hr, b"session->ReceiveEndMessage( timeoutMS, exchangeId, &status, &respMsg)\0", 0x284);
    }
    if status != 0 {
        return fail(make_transport_status_error(status), b"MakeTransportStatusError(status)\0", 0x291);
    }
    if !reply.is_null() {
        let hr = deserialize_body(resp.get(), body.put());
        if hr < 0 {
            return fail(hr, b"XtfDeserializeJsonBody(respMsg, &respBody)\0", 0x28b);
        }
        *reply = body.detach();
    }
    S_OK
}

pub unsafe fn send_json_message(session: Obj, body: Obj, op: u32, reply: *mut Obj) -> HRESULT {
    let hr = send_json_message_with_timeout(session, body, op, DEFAULT_RECEIVE_TIMEOUT, reply);
    if hr < 0 {
        log_api(b"XtfSendJsonMessage( session, body, operationId, XbtpDefaultReceiveTimeout, reply)\0", hr, API_CPP, 0x255);
        return hr;
    }
    S_OK
}
