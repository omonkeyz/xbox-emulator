use crate::op::*;
use crate::privilege;
use console_sandbox::registry::{reg_close_key, reg_open_key_ex_w, HKEY_LOCAL_MACHINE, REG_MULTI_SZ};
use console_sandbox::regw::reg_query_value_ex_w;
use xtfmsg::chk::Activity;
use xtfmsg::com::{Com, Obj};
use xtfmsg::msg::{deserialize_body, get_json_bool, get_json_u32, serialize_end_body, set_json_bool, set_json_property64};
use xtfmsg::sys::*;
use xtfmsg::xbtp;

pub const SHUTDOWN_CPP: &str = "xbox\\devex\\xtf\\dll\\console\\shutdown.cpp";
pub const SYSTEMTIME_CPP: &str = "xbox\\devex\\xtf\\dll\\console\\systemtime.cpp";
pub const UPDATESPENDING_CPP: &str = "xbox\\devex\\xtf\\dll\\console\\updatespending.cpp";
pub const INCONNECTEDSTANDBY_CPP: &str = "xbox\\devex\\xtf\\dll\\console\\inconnectedstandby.cpp";

pub const SESSION_MANAGER_KEY: &str = "OSDATA\\CurrentControlSet\\Control\\Session Manager";
pub const PENDING_FILE_RENAME_OPERATIONS: &str = "PendingFileRenameOperations";
pub const OPEN_SAMS: [u32; 3] = [0xf003f, 0x2001f, 0x20019];

#[repr(C)]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SystemTime {
    pub year: u16,
    pub month: u16,
    pub day_of_week: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub milliseconds: u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemTime(st: *mut SystemTime);
    fn SystemTimeToFileTime(st: *const SystemTime, ft: *mut u64) -> i32;
}

pub fn console_time() -> u64 {
    let mut st = SystemTime::default();
    let mut ft = 0u64;
    unsafe {
        GetSystemTime(&mut st);
        SystemTimeToFileTime(&st, &mut ft);
    }
    ft
}

pub fn system_time_fields() -> (u16, u16, u16, u16, u16) {
    let mut st = SystemTime::default();
    unsafe { GetSystemTime(&mut st) };
    (st.year, st.month, st.day, st.hour, st.minute)
}

pub unsafe extern "system" fn shutdown_handler(_session: usize, request: usize, reply: usize) -> HRESULT {
    let activity = Activity::start(b"XtfConsoleControl: ShutdownHandler\0");
    let mut op = Op::new(SHUTDOWN_CPP, "ShutdownHandler", reply);
    let mut req = Com::null();
    'done: {
        let hr = deserialize_body(request as Obj, req.put());
        if hr < 0 {
            op.fail(hr, 224, "XtfDeserializeJsonBody(received, &reqBody)");
            break 'done;
        }
        let mut reboot = false;
        let n = w("Reboot");
        let hr = get_json_bool(req.get(), n.as_ptr(), &mut reboot);
        if hr < 0 {
            op.fail(hr, 225, "XtfGetJsonProperty(reqBody, RebootBoolProperty, &reboot)");
            break 'done;
        }
        let mut power_mode_aware = false;
        let n = w("PowerModeAware");
        let r = get_json_bool(req.get(), n.as_ptr(), &mut power_mode_aware);
        let hr = if r == E_JSON_NOT_FOUND { S_OK } else { r };
        op.check(hr, 226, "XtfTryGetJsonProperty(reqBody, PowerModeAwareBoolProperty, &powermodeAwareExists, &powermodeAware)");
        if hr < 0 {
            break 'done;
        }
        let mut delay = 0u32;
        let n = w("PowerOnDelayInMinutes");
        let r = get_json_u32(req.get(), n.as_ptr(), &mut delay);
        let delay_exists = r != E_JSON_NOT_FOUND;
        let hr = if r == E_JSON_NOT_FOUND { S_OK } else { r };
        op.check(hr, 232, "XtfTryGetJsonProperty(reqBody, PowerOnDelayInMinutesDwordProperty, &powerOnDelayInMinutesExists, &powerOnDelayInMinutes)");
        if hr < 0 {
            break 'done;
        }
        if delay_exists {
            op.check(E_INVALIDARG, 264, "E_INVALIDARG");
            break 'done;
        }
        let mut restore = false;
        let hr = privilege::set_privilege(privilege::SE_SHUTDOWN_NAME, true, Some(&mut restore));
        if hr < 0 {
            op.fail(hr, 272, "XtfSetPrivilege(SE_SHUTDOWN_NAME, true, &fRestoreState)");
            break 'done;
        }
        let _ = (reboot, power_mode_aware);
        op.fail(E_NOTIMPL, 280, "XtfInitiateSystemShutdown(reboot, powermodeAware)");
    }
    op.xce_empty();
    drop(req);
    op.finish(activity)
}

pub unsafe extern "system" fn get_system_time_handler(_session: usize, _request: usize, reply: usize) -> HRESULT {
    let activity = Activity::start(b"XtfConsoleControl: GetSystemTimeHandler\0");
    let mut op = Op::new(SYSTEMTIME_CPP, "GetSystemTimeHandler", reply);
    let mut body = Com::null();
    let ft = console_time();
    'done: {
        let hr = xbtp::create_json_object(body.put());
        if hr < 0 {
            op.fail(hr, 65, "XbtpCreateJsonObject(&replyBody)");
            break 'done;
        }
        let n = w("ConsoleTime");
        let hr = set_json_property64(body.get(), n.as_ptr(), ft);
        if hr < 0 {
            op.fail(hr, 69, "XtfSetJsonProperty<UINT64>( replyBody, SystemTimeProperty, *(UINT64 *)(fTime))");
            break 'done;
        }
        let hr = serialize_end_body(body.get(), reply as Obj);
        if hr < 0 {
            op.fail(hr, 70, "XtfSerializeJsonBody(replyBody, pReply)");
            break 'done;
        }
    }
    if op.hr < 0 {
        op.xce_empty();
    }
    drop(body);
    op.finish(activity);
    S_OK
}

unsafe fn query_pending(key: usize, data: *mut u8, size: &mut u32) -> HRESULT {
    let n = w(PENDING_FILE_RENAME_OPERATIONS);
    let mut kind = 0u32;
    let mut cb = if data.is_null() { 0 } else { *size };
    let e = reg_query_value_ex_w(key, n.as_ptr(), core::ptr::null_mut(), &mut kind, data, &mut cb);
    if e == 0 || e == 234 {
        *size = cb;
    }
    if e != 0 {
        return hr_from_win32(e);
    }
    if kind == REG_MULTI_SZ {
        S_OK
    } else {
        E_UNEXPECTED
    }
}

unsafe fn read_pending(key: usize) -> (HRESULT, Option<Vec<u16>>) {
    let mut cb = 0u32;
    let hr = query_pending(key, core::ptr::null_mut(), &mut cb);
    if hr < 0 {
        return (hr, None);
    }
    let n = (cb >> 1).wrapping_sub(1);
    let mut buf: Option<Vec<u16>> = None;
    if n != 0 {
        if n > 0x7fff_fff0 {
            return (E_OUTOFMEMORY, None);
        }
        buf = Some(vec![0u16; n as usize + 1]);
    }
    let mut size = cb;
    let ptr = buf.as_mut().map_or(core::ptr::null_mut(), |b| b.as_mut_ptr() as *mut u8);
    if !ptr.is_null() && size as usize > (n as usize + 1) * 2 {
        size = (n as usize + 1) as u32 * 2;
    }
    let hr = query_pending(key, ptr, &mut size);
    if hr < 0 {
        return (hr, buf);
    }
    match buf {
        Some(b) if b[0] != 0 => (S_OK, Some(b)),
        _ => (S_OK, None),
    }
}

pub fn are_updates_pending() -> bool {
    unsafe {
        let sub = w(SESSION_MANAGER_KEY);
        let mut key = 0usize;
        let mut hr = E_UNEXPECTED;
        for sam in OPEN_SAMS {
            if hr < 0 {
                hr = hr_from_win32(reg_open_key_ex_w(HKEY_LOCAL_MACHINE, sub.as_ptr(), 0, sam, &mut key));
            }
        }
        if hr < 0 {
            return false;
        }
        let (_, buf) = read_pending(key);
        reg_close_key(key);
        matches!(buf, Some(b) if b[0] != 0)
    }
}

pub unsafe extern "system" fn updates_pending_handler(_session: usize, _request: usize, reply: usize) -> HRESULT {
    let activity = Activity::start(b"XtfConsoleControl: UpdatesPendingHandler\0");
    let mut op = Op::new(UPDATESPENDING_CPP, "UpdatesPendingHandler", reply);
    let mut body = Com::null();
    'done: {
        let hr = xbtp::create_json_object(body.put());
        if hr < 0 {
            op.fail(hr, 106, "XbtpCreateJsonObject(&replyBody)");
            break 'done;
        }
        let n = w("UpdatesPending");
        let hr = set_json_bool(body.get(), n.as_ptr(), are_updates_pending());
        if hr < 0 {
            op.fail(hr, 110, "XtfSetJsonProperty<bool>( replyBody, UpdatesPendingProperty, XtfAreUpdatesPending())");
            break 'done;
        }
        let hr = serialize_end_body(body.get(), reply as Obj);
        if hr < 0 {
            op.fail(hr, 111, "XtfSerializeJsonBody(replyBody, pReply)");
            break 'done;
        }
    }
    if op.hr < 0 {
        op.xce_empty();
    }
    drop(body);
    op.finish(activity);
    S_OK
}

pub unsafe extern "system" fn in_connected_standby_handler(_session: usize, _request: usize, reply: usize) -> HRESULT {
    let activity = Activity::start(b"XtfConsoleControl: InConnectedStandbyHandler\0");
    let mut op = Op::new(INCONNECTEDSTANDBY_CPP, "InConnectedStandbyHandler", reply);
    let mut body = Com::null();
    let in_cs = false;
    let allows_instant_on = false;
    let allow_xbox_app_connections = false;
    'done: {
        let hr = xbtp::create_json_object(body.put());
        if hr < 0 {
            op.fail(hr, 119, "XbtpCreateJsonObject(&replyBody)");
            break 'done;
        }
        let n = w("InConnectedStandby");
        let hr = set_json_bool(body.get(), n.as_ptr(), in_cs);
        if hr < 0 {
            op.fail(hr, 123, "XtfSetJsonProperty<bool>( replyBody, InConnectedStandbyProperty, isInCS)");
            break 'done;
        }
        let n = w("AllowsInstantOn");
        let hr = set_json_bool(body.get(), n.as_ptr(), allows_instant_on);
        if hr < 0 {
            op.fail(hr, 127, "XtfSetJsonProperty<bool>( replyBody, AllowsInstantOnProperty, allowsInstantOn)");
            break 'done;
        }
        let n = w("AllowXboxAppConnections");
        let hr = set_json_bool(body.get(), n.as_ptr(), allow_xbox_app_connections);
        if hr < 0 {
            op.fail(hr, 131, "XtfSetJsonProperty<bool>( replyBody, AllowsXboxAppConnectionsProperty, allowXboxAppConnections)");
            break 'done;
        }
        let hr = serialize_end_body(body.get(), reply as Obj);
        if hr < 0 {
            op.fail(hr, 132, "XtfSerializeJsonBody(replyBody, pReply)");
            break 'done;
        }
    }
    if op.hr < 0 {
        op.xce_empty();
    }
    drop(body);
    op.finish(activity);
    S_OK
}
