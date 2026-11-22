use crate::chk::chk;
use xtfmsg::chk::Activity;
use xtfmsg::com::{json, Com, Obj};
use xtfmsg::msg::{serialize_end_body, set_json_bool, set_json_i32, ErrorContext};
use xtfmsg::sys::*;
use xtfmsg::xbtp;

pub const EXTENSION_VERSION: &str = "220604";
pub const MESSAGEHELPERS_CPP: &str = "xbox\\devex\\xtf\\dll\\console\\messagehelpers.cpp";

pub fn w(s: &str) -> Vec<u16> {
    wide(s)
}

pub struct Bstr(pub BSTR);

impl Bstr {
    pub fn null() -> Bstr {
        Bstr(core::ptr::null_mut())
    }

    pub fn put(&mut self) -> *mut BSTR {
        self.reset();
        &mut self.0
    }

    pub fn reset(&mut self) {
        let p = core::mem::replace(&mut self.0, core::ptr::null_mut());
        if !p.is_null() {
            unsafe { SysFreeString(p) };
        }
    }

    pub fn detach(&mut self) -> BSTR {
        core::mem::replace(&mut self.0, core::ptr::null_mut())
    }

    pub fn ptr(&self) -> *const u16 {
        self.0
    }

    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    pub fn text(&self) -> Option<String> {
        if self.0.is_null() {
            None
        } else {
            Some(from_wide(self.0))
        }
    }

    pub fn from_str(s: &str) -> Bstr {
        let v = wide(s);
        Bstr(unsafe { SysAllocString(v.as_ptr()) })
    }
}

impl Drop for Bstr {
    fn drop(&mut self) {
        self.reset();
    }
}

pub unsafe fn get_string(body: Obj, name: &str, out: &mut Bstr) -> HRESULT {
    let n = w(name);
    json::get_string(body, n.as_ptr(), out.put())
}

pub unsafe fn set_string(body: Obj, name: &str, v: *const u16) -> HRESULT {
    let n = w(name);
    json::set_string(body, n.as_ptr(), v)
}

pub unsafe fn set_string_text(body: Obj, name: &str, v: &str) -> HRESULT {
    let t = w(v);
    set_string(body, name, t.as_ptr())
}

pub struct Op {
    pub file: &'static str,
    pub func: &'static str,
    pub ctx: ErrorContext,
    pub hr: HRESULT,
}

impl Op {
    pub fn new(file: &'static str, func: &'static str, reply: usize) -> Op {
        Op { file, func, ctx: ErrorContext::new(reply as Obj), hr: S_OK }
    }

    pub unsafe fn fail(&mut self, hr: HRESULT, line: u32, expr: &str) {
        self.ctx.append_error(hr, self.file, line as i32);
        self.ctx.hr = hr;
        self.hr = hr;
        chk(self.file, self.func, line, "_hr", hr, expr);
    }

    pub fn check(&mut self, hr: HRESULT, line: u32, name: &str) {
        self.hr = hr;
        chk(self.file, self.func, line, name, hr, "");
    }

    pub unsafe fn xce(&self, task_name: *const u16, task_value: *const u16) {
        let v = w(EXTENSION_VERSION);
        xtfmsg::xce::log_operation_result(v.as_ptr(), task_name, task_value, self.hr);
    }

    pub unsafe fn xce_empty(&self) {
        let e = w("");
        self.xce(e.as_ptr(), e.as_ptr());
    }

    pub unsafe fn complete(&mut self, reply: usize, body: Obj) {
        complete_operation(reply as Obj, body, &mut self.hr);
    }

    pub fn finish(self, activity: Box<Activity>) -> HRESULT {
        let hr = self.hr;
        drop(self.ctx);
        activity.stop();
        hr
    }
}

fn mh(line: u32, expr: &str, hr: HRESULT) {
    chk(MESSAGEHELPERS_CPP, "XtfConsoleControl::CompleteOperationHandler", line, expr, hr, "");
}

pub unsafe fn complete_operation(reply: Obj, body: Obj, hr: &mut HRESULT) {
    if body.is_null() {
        return;
    }
    let mut r;
    'done: {
        if *hr < 0 {
            let s = w("Successful");
            r = set_json_bool(body, s.as_ptr(), false);
            mh(48, "XtfSetJsonProperty(replyBody, SuccessfulProperty, false)", r);
            if r < 0 {
                break 'done;
            }
            let mut result = Com::null();
            r = xbtp::create_json_object(result.put());
            mh(49, "XbtpCreateJsonObject(&result)", r);
            if r < 0 {
                break 'done;
            }
            let rc = w("ResultCode");
            r = set_json_i32(result.get(), rc.as_ptr(), *hr);
            mh(50, "XtfSetJsonProperty(result, ResultCodeProperty, *handlerResult)", r);
            if r < 0 {
                break 'done;
            }
            let ro = w("Result");
            r = json::set_object(body, ro.as_ptr(), result.get());
            mh(51, "replyBody->SetObject(ResultObject, result)", r);
            if r < 0 {
                break 'done;
            }
            *hr = S_OK;
        } else {
            let s = w("Successful");
            r = set_json_bool(body, s.as_ptr(), true);
            mh(59, "XtfSetJsonProperty(replyBody, SuccessfulProperty, true)", r);
            if r < 0 {
                break 'done;
            }
        }
        r = serialize_end_body(body, reply);
        mh(63, "XtfSerializeJsonBody(replyBody, reply)", r);
    }
    if *hr >= 0 {
        *hr = r;
    }
}
