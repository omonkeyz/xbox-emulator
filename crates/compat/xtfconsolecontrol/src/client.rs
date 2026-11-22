use xtfmsg::api::send_json_message;
use xtfmsg::com::{json, Com, Obj};
use crate::system::SystemTime;
use xtfmsg::msg::{get_json_bool, get_json_property64};
use xtfmsg::sys::*;
use xtfmsg::xbtp;

const E_POINTER: HRESULT = 0x8000_4003u32 as i32;

fn check(hr: HRESULT) -> Result<(), HRESULT> {
    if hr < 0 { Err(hr) } else { Ok(()) }
}

// ConsoleControlClient::CheckTaskErrors (19041 RVA 0x77c0) and its
// XtfOperationError reader (0xcc60). A successful reply can omit the envelope.
unsafe fn check_task_errors(reply: Obj) -> Result<(), HRESULT> {
    let mut envelope = Com::null();
    if json::get_object(reply, wide("XtfOperationError").as_ptr(), envelope.put()) < 0 {
        return Ok(());
    }
    let mut result = 0.0;
    let mut checkpoint = 0.0;
    let mut errors = Com::null();
    check(json::get_number(envelope.get(), wide("ReturnCode").as_ptr(), &mut result))?;
    check(json::get_number(envelope.get(), wide("Checkpoint").as_ptr(), &mut checkpoint))?;
    check(json::get_array(envelope.get(), wide("Errors").as_ptr(), errors.put()))?;
    // The native double-to-i32 conversion yields INT_MIN outside the signed range.
    let code = result.trunc();
    check(if code >= i32::MIN as f64 && code <= i32::MAX as f64 { code as i32 } else { i32::MIN })
}

unsafe fn request_status(session: Obj, op: u32) -> Result<Com, HRESULT> {
    let mut request = Com::null();
    let mut reply = Com::null();
    check(xbtp::create_json_object(request.put()))?;
    check(send_json_message(session, request.get(), op, reply.put()))?;
    check_task_errors(reply.get())?;
    Ok(reply)
}

#[link(name = "kernel32")]
extern "system" {
    fn FileTimeToSystemTime(file_time: *const u64, system_time: *mut SystemTime) -> i32;
}

// Session exchange used by GetSystemTime (19041 RVA 0x8c20).
pub unsafe fn get_system_time(session: Obj, out: *mut SystemTime) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let mut time = SystemTime::default();
    let result = request_status(session, crate::OP_GET_SYSTEM_TIME).map(|reply| {
        let mut ticks = 0;
        let hr = get_json_property64(reply.get(), wide("ConsoleTime").as_ptr(), &mut ticks);
        if hr >= 0 {
            // The console returns the property read status even if conversion fails.
            FileTimeToSystemTime(&ticks, &mut time);
        }
        hr
    });
    *out = time;
    result.unwrap_or_else(|hr| hr)
}

// Session exchange used by UpdatesPending (19041 RVA 0x94d0).
pub unsafe fn updates_pending(session: Obj, out: *mut i32) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let mut pending = false;
    let hr = match request_status(session, crate::OP_UPDATES_PENDING) {
        Ok(reply) => get_json_bool(reply.get(), wide("UpdatesPending").as_ptr(), &mut pending),
        Err(hr) => hr,
    };
    *out = pending as i32;
    hr
}

// Session exchange used by QueryConnectedStandbyEx (19041 RVA 0x7140).
pub unsafe fn query_connected_standby_ex(session: Obj, in_standby: *mut i32, instant_on: *mut i32, app_connections: *mut i32) -> HRESULT {
    if in_standby.is_null() || instant_on.is_null() || app_connections.is_null() {
        return E_POINTER;
    }
    let mut flags = [false; 3];
    let result = (|| {
        let reply = request_status(session, crate::OP_IN_CONNECTED_STANDBY)?;
        check(get_json_bool(reply.get(), wide("InConnectedStandby").as_ptr(), &mut flags[0]))?;
        check(get_json_bool(reply.get(), wide("AllowsInstantOn").as_ptr(), &mut flags[1]))?;
        let hr = get_json_bool(reply.get(), wide("AllowXboxAppConnections").as_ptr(), &mut flags[2]);
        Ok(if hr == E_JSON_NOT_FOUND { S_OK } else { hr })
    })();
    *in_standby = flags[0] as i32;
    *instant_on = flags[1] as i32;
    *app_connections = flags[2] as i32;
    result.unwrap_or_else(|hr| hr)
}

pub unsafe fn query_connected_standby(session: Obj, in_standby: *mut i32, instant_on: *mut i32) -> HRESULT {
    query_connected_standby_ex(session, in_standby, instant_on, &mut 0)
}
