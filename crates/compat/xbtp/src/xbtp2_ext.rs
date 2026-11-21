use crate::sys::*;
use crate::xbtp2_json::IID_IUNKNOWN;
use crate::xbtp2_sys::XbtpInitialize;
use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

pub const IID_XBTP_EXTENSION_METADATA: GUID = GUID::new(0x2cae48d5, 0x166f, 0x4059, [0x82, 0xc0, 0x6c, 0xb9, 0x90, 0x83, 0x9c, 0x30]);
pub const IID_XBTP_EXTENSION_METADATA_IMPL: GUID = GUID::new(0xe2b6c063, 0x7d82, 0x414a, [0xbe, 0x86, 0x67, 0xed, 0x1b, 0x44, 0x77, 0x97]);
pub const IID_XBTP_OPERATION_HANDLER: GUID = GUID::new(0x07772e34, 0x6294, 0x4518, [0x9b, 0x11, 0x30, 0x81, 0x5f, 0xc8, 0x63, 0xe0]);
pub const IID_XBTP_MESSAGE: GUID = GUID::new(0xd24904a1, 0x8c12, 0x4856, [0x9d, 0x86, 0x4e, 0x1f, 0x3b, 0x55, 0x5d, 0x73]);
pub const IID_XBTP_MESSAGE_IMPL: GUID = GUID::new(0x6965e04a, 0x2829, 0x41a8, [0xb1, 0x94, 0x48, 0x73, 0x15, 0x07, 0x5e, 0x7a]);

pub const HR_OBJECT_ALREADY_EXISTS: HRESULT = 0x8007_1392u32 as i32;
pub const HR_UNEXPECTED_JSON_TRANSLATION: HRESULT = 0x8C11_0401u32 as i32;
const HANDLER_BUCKETS: u32 = 13;
const CP_UTF8: u32 = 65001;

type Qi = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
type Ref = unsafe extern "system" fn(*mut c_void) -> u32;
pub type OperationFn = unsafe extern "system" fn(usize, usize, usize) -> HRESULT;

#[repr(C)]
pub struct HandlerVtbl {
    pub query_interface: Qi,
    pub add_ref: Ref,
    pub release: Ref,
    pub get_operation_id: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    pub invoke: unsafe extern "system" fn(*mut c_void, usize, usize, usize) -> HRESULT,
}

#[repr(C)]
pub struct FunctionHandler {
    pub vtbl: *const HandlerVtbl,
    pub refs: AtomicU32,
    pub operation_id: u32,
    pub func: OperationFn,
}

unsafe fn vt<T>(p: *mut c_void) -> &'static T {
    &**(p as *const *const T)
}

pub(crate) unsafe fn com_add_ref(p: *mut c_void) -> u32 {
    let v: &[usize; 3] = vt(p);
    let f: Ref = core::mem::transmute(v[1]);
    f(p)
}

pub(crate) unsafe fn com_release(p: *mut c_void) -> u32 {
    let v: &[usize; 3] = vt(p);
    let f: Ref = core::mem::transmute(v[2]);
    f(p)
}

pub(crate) unsafe fn com_qi(p: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    let v: &[usize; 3] = vt(p);
    let f: Qi = core::mem::transmute(v[0]);
    f(p, iid, out)
}

pub(crate) unsafe fn qi_match(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void, ids: &[GUID]) -> HRESULT {
    if ids.iter().any(|g| *g == *iid) {
        *out = this;
        com_add_ref(this);
        S_OK
    } else {
        E_NOINTERFACE
    }
}

unsafe extern "system" fn fh_qi(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    qi_match(this, iid, out, &[IID_IUNKNOWN, IID_XBTP_OPERATION_HANDLER])
}

unsafe extern "system" fn fh_add_ref(this: *mut c_void) -> u32 {
    (*(this as *mut FunctionHandler)).refs.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn fh_release(this: *mut c_void) -> u32 {
    let n = (*(this as *mut FunctionHandler)).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        drop(Box::from_raw(this as *mut FunctionHandler));
    }
    n
}

unsafe extern "system" fn fh_get_operation_id(this: *mut c_void, out: *mut u32) -> HRESULT {
    *out = (*(this as *mut FunctionHandler)).operation_id;
    S_OK
}

unsafe extern "system" fn fh_invoke(this: *mut c_void, a: usize, b: usize, c: usize) -> HRESULT {
    ((*(this as *mut FunctionHandler)).func)(a, b, c)
}

pub static FUNCTION_HANDLER_VTBL: HandlerVtbl = HandlerVtbl {
    query_interface: fh_qi,
    add_ref: fh_add_ref,
    release: fh_release,
    get_operation_id: fh_get_operation_id,
    invoke: fh_invoke,
};

#[derive(Default)]
pub struct HandlerTable {
    pub buckets: Vec<Vec<(u32, *mut c_void)>>,
    pub count: u32,
}

impl HandlerTable {
    fn contains(&self, id: u32) -> bool {
        if self.buckets.is_empty() {
            return false;
        }
        self.buckets[(id % self.buckets.len() as u32) as usize].iter().any(|e| e.0 == id)
    }

    fn add(&mut self, id: u32, h: *mut c_void) -> HRESULT {
        if self.buckets.is_empty() {
            self.buckets = (0..HANDLER_BUCKETS).map(|_| Vec::new()).collect();
            self.count = 0;
        }
        if self.contains(id) {
            return HR_OBJECT_ALREADY_EXISTS;
        }
        let n = self.buckets.len() as u32;
        self.buckets[(id % n) as usize].insert(0, (id, h));
        self.count += 1;
        S_OK
    }

    fn iter(&self) -> impl Iterator<Item = *mut c_void> + '_ {
        self.buckets.iter().flat_map(|b| b.iter().map(|e| e.1))
    }
}

#[repr(C)]
pub struct MetadataVtbl {
    pub query_interface: Qi,
    pub add_ref: Ref,
    pub release: Ref,
    pub get_handler_count: unsafe extern "system" fn(*mut ExtensionMetadata, *mut u32) -> HRESULT,
    pub get_handlers: unsafe extern "system" fn(*mut ExtensionMetadata, u32, *mut *mut c_void) -> HRESULT,
    pub add_handlers: unsafe extern "system" fn(*mut ExtensionMetadata, u32, *const *mut c_void) -> HRESULT,
    pub add_function: unsafe extern "system" fn(*mut ExtensionMetadata, u32, Option<OperationFn>) -> HRESULT,
    pub add_handler: unsafe extern "system" fn(*mut ExtensionMetadata, *mut c_void) -> HRESULT,
}

#[repr(C)]
pub struct ExtensionMetadata {
    pub vtbl: *const MetadataVtbl,
    pub refs: AtomicU32,
    pub lock: Mutex<HandlerTable>,
}

unsafe extern "system" fn md_qi(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    qi_match(this, iid, out, &[IID_XBTP_EXTENSION_METADATA, IID_IUNKNOWN, IID_XBTP_EXTENSION_METADATA_IMPL])
}

unsafe extern "system" fn md_add_ref(this: *mut c_void) -> u32 {
    (*(this as *mut ExtensionMetadata)).refs.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn md_release(this: *mut c_void) -> u32 {
    let m = this as *mut ExtensionMetadata;
    let n = (*m).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        let b = Box::from_raw(m);
        let handlers: Vec<*mut c_void> = b.lock.lock().map(|t| t.iter().collect()).unwrap_or_default();
        for h in handlers {
            com_release(h);
        }
        drop(b);
    }
    n
}

unsafe fn md_table(m: *mut ExtensionMetadata) -> std::sync::MutexGuard<'static, HandlerTable> {
    let l: &'static Mutex<HandlerTable> = &*(&(*m).lock as *const Mutex<HandlerTable>);
    l.lock().unwrap_or_else(|e| e.into_inner())
}

unsafe extern "system" fn md_get_handler_count(m: *mut ExtensionMetadata, out: *mut u32) -> HRESULT {
    let t = md_table(m);
    if out.is_null() {
        return E_INVALIDARG;
    }
    *out = t.count;
    S_OK
}

unsafe extern "system" fn md_get_handlers(m: *mut ExtensionMetadata, size: u32, out: *mut *mut c_void) -> HRESULT {
    let t = md_table(m);
    if out.is_null() {
        return E_INVALIDARG;
    }
    let need = t.count.wrapping_mul(8);
    if size < need {
        return HR_INSUFFICIENT_BUFFER;
    }
    if need == 0 {
        return S_OK;
    }
    for (i, h) in t.iter().take(t.count as usize).enumerate() {
        *out.add(i) = h;
    }
    S_OK
}

unsafe extern "system" fn md_add_handlers(m: *mut ExtensionMetadata, n: u32, list: *const *mut c_void) -> HRESULT {
    if list.is_null() {
        return E_INVALIDARG;
    }
    let mut hr = S_OK;
    for i in 0..n as usize {
        let h = *list.add(i);
        if h.is_null() {
            continue;
        }
        hr = ((*(*m).vtbl).add_handler)(m, h);
        if hr < 0 {
            break;
        }
    }
    hr
}

unsafe extern "system" fn md_add_function(m: *mut ExtensionMetadata, id: u32, func: Option<OperationFn>) -> HRESULT {
    let func = match func {
        Some(f) => f,
        None => return E_INVALIDARG,
    };
    let h = Box::into_raw(Box::new(FunctionHandler { vtbl: &FUNCTION_HANDLER_VTBL, refs: AtomicU32::new(1), operation_id: id, func })) as *mut c_void;
    com_add_ref(h);
    let hr = ((*(*m).vtbl).add_handler)(m, h);
    if hr < 0 {
        com_release(h);
    }
    hr
}

unsafe extern "system" fn md_add_handler(m: *mut ExtensionMetadata, h: *mut c_void) -> HRESULT {
    let mut t = md_table(m);
    if h.is_null() {
        return E_INVALIDARG;
    }
    let hv: &HandlerVtbl = vt(h);
    let mut id = 0u32;
    let hr = (hv.get_operation_id)(h, &mut id);
    if hr < 0 {
        return hr;
    }
    let hr = t.add(id, h);
    if hr < 0 {
        return hr;
    }
    com_add_ref(h);
    S_OK
}

pub static METADATA_VTBL: MetadataVtbl = MetadataVtbl {
    query_interface: md_qi,
    add_ref: md_add_ref,
    release: md_release,
    get_handler_count: md_get_handler_count,
    get_handlers: md_get_handlers,
    add_handlers: md_add_handlers,
    add_function: md_add_function,
    add_handler: md_add_handler,
};

#[no_mangle]
pub unsafe extern "C" fn XbtpCreateExtensionMetadata(iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let hr = XbtpInitialize();
    if hr < 0 {
        return hr;
    }
    let m = Box::into_raw(Box::new(ExtensionMetadata { vtbl: &METADATA_VTBL, refs: AtomicU32::new(1), lock: Mutex::new(HandlerTable::default()) })) as *mut c_void;
    let hr = com_qi(m, iid, out);
    com_release(m);
    hr
}

#[derive(Default)]
pub struct Body {
    pub ptr: *mut u8,
    pub count: u32,
    pub owned: Option<Vec<u8>>,
}

impl Body {
    fn configure(&mut self, count: u32, src: *const u8, copy: bool) -> HRESULT {
        self.owned = None;
        self.ptr = null_mut();
        self.count = 0;
        if copy {
            if count != 0 {
                let mut v = Vec::new();
                if v.try_reserve_exact(count as usize).is_err() {
                    return E_OUTOFMEMORY;
                }
                v.resize(count as usize, 0u8);
                if !src.is_null() {
                    unsafe { core::ptr::copy_nonoverlapping(src, v.as_mut_ptr(), count as usize) };
                }
                self.ptr = v.as_mut_ptr();
                self.owned = Some(v);
            }
        } else {
            self.ptr = src as *mut u8;
        }
        self.count = count;
        S_OK
    }

    fn adopt(&mut self, mut v: Vec<u8>) {
        self.count = v.len() as u32;
        self.ptr = v.as_mut_ptr();
        self.owned = Some(v);
    }
}

#[derive(Default)]
pub struct MessageBodies {
    pub json: Body,
    pub binary: Body,
    pub received: crate::msvc::SharedPtr<crate::encoding::XBTP_MESSAGE_BODY>,
}

pub unsafe fn new_message() -> *mut Message {
    Box::into_raw(Box::new(Message { vtbl: &MESSAGE_VTBL, refs: AtomicU32::new(1), lock: Mutex::new(MessageBodies::default()) }))
}

pub unsafe fn message_set_received(m: *mut Message, body: crate::msvc::SharedPtr<crate::encoding::XBTP_MESSAGE_BODY>) -> HRESULT {
    let mut b = msg_lock(m);
    if !body.ptr.is_null() {
        let src = &*body.ptr;
        b.binary.owned = None;
        b.binary.ptr = src.binary_body;
        b.binary.count = src.binary_body_length;
        b.json.owned = None;
        b.json.ptr = src.json_body;
        b.json.count = src.json_body_length;
    }
    let mut old = core::mem::replace(&mut b.received, body);
    drop(b);
    old.release();
    S_OK
}

#[repr(C)]
pub struct MessageVtbl {
    pub query_interface: Qi,
    pub add_ref: Ref,
    pub release: Ref,
    pub get_json_body: unsafe extern "system" fn(*mut Message, *mut u32, *mut *mut u8) -> HRESULT,
    pub get_binary_body: unsafe extern "system" fn(*mut Message, *mut u32, *mut *mut u8) -> HRESULT,
    pub set_json_body: unsafe extern "system" fn(*mut Message, u32, *const u8) -> HRESULT,
    pub set_json_body_copy: unsafe extern "system" fn(*mut Message, u32, *const u8) -> HRESULT,
    pub set_json_body_w: unsafe extern "system" fn(*mut Message, *const u16) -> HRESULT,
    pub set_binary_body: unsafe extern "system" fn(*mut Message, u32, *const u8) -> HRESULT,
    pub set_binary_body_copy: unsafe extern "system" fn(*mut Message, u32, *const u8) -> HRESULT,
}

#[repr(C)]
pub struct Message {
    pub vtbl: *const MessageVtbl,
    pub refs: AtomicU32,
    pub lock: Mutex<MessageBodies>,
}

unsafe fn msg_lock(m: *mut Message) -> std::sync::MutexGuard<'static, MessageBodies> {
    let l: &'static Mutex<MessageBodies> = &*(&(*m).lock as *const Mutex<MessageBodies>);
    l.lock().unwrap_or_else(|e| e.into_inner())
}

unsafe extern "system" fn msg_qi(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    qi_match(this, iid, out, &[IID_XBTP_MESSAGE, IID_IUNKNOWN, IID_XBTP_MESSAGE_IMPL])
}

unsafe extern "system" fn msg_add_ref(this: *mut c_void) -> u32 {
    (*(this as *mut Message)).refs.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn msg_release(this: *mut c_void) -> u32 {
    let n = (*(this as *mut Message)).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        drop(Box::from_raw(this as *mut Message));
    }
    n
}

unsafe extern "system" fn msg_get_json_body(m: *mut Message, count: *mut u32, body: *mut *mut u8) -> HRESULT {
    let b = msg_lock(m);
    if body.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    *count = b.json.count;
    *body = b.json.ptr;
    S_OK
}

unsafe extern "system" fn msg_get_binary_body(m: *mut Message, count: *mut u32, body: *mut *mut u8) -> HRESULT {
    let b = msg_lock(m);
    if count.is_null() || body.is_null() {
        return E_INVALIDARG;
    }
    *count = b.binary.count;
    *body = b.binary.ptr;
    S_OK
}

unsafe extern "system" fn msg_set_json_body(m: *mut Message, n: u32, s: *const u8) -> HRESULT {
    msg_lock(m).json.configure(n, s, false)
}

unsafe extern "system" fn msg_set_json_body_copy(m: *mut Message, n: u32, s: *const u8) -> HRESULT {
    msg_lock(m).json.configure(n, s, true)
}

unsafe extern "system" fn msg_set_binary_body(m: *mut Message, n: u32, s: *const u8) -> HRESULT {
    msg_lock(m).binary.configure(n, s, false)
}

unsafe extern "system" fn msg_set_binary_body_copy(m: *mut Message, n: u32, s: *const u8) -> HRESULT {
    msg_lock(m).binary.configure(n, s, true)
}

pub unsafe fn convert_wide_to_utf8(w: *const u16) -> Result<Vec<u8>, HRESULT> {
    if w.is_null() {
        return Err(E_INVALIDARG);
    }
    let n = WideCharToMultiByte(CP_UTF8, 0, w, -1, null_mut(), 0, core::ptr::null(), null_mut());
    if n <= 0 {
        let hr = hr_last_error();
        if hr != 0 {
            return Err(hr);
        }
    }
    let size = n.max(0) as usize;
    let mut v = Vec::new();
    if v.try_reserve_exact(size).is_err() {
        return Err(E_OUTOFMEMORY);
    }
    v.resize(size, 0u8);
    let got = WideCharToMultiByte(CP_UTF8, 0, w, -1, v.as_mut_ptr(), n, core::ptr::null(), null_mut());
    if got != n {
        return Err(HR_UNEXPECTED_JSON_TRANSLATION);
    }
    Ok(v)
}

unsafe extern "system" fn msg_set_json_body_w(m: *mut Message, w: *const u16) -> HRESULT {
    let mut b = msg_lock(m);
    if w.is_null() {
        return E_INVALIDARG;
    }
    match convert_wide_to_utf8(w) {
        Ok(v) => {
            b.json.adopt(v);
            S_OK
        }
        Err(e) => e,
    }
}

pub static MESSAGE_VTBL: MessageVtbl = MessageVtbl {
    query_interface: msg_qi,
    add_ref: msg_add_ref,
    release: msg_release,
    get_json_body: msg_get_json_body,
    get_binary_body: msg_get_binary_body,
    set_json_body: msg_set_json_body,
    set_json_body_copy: msg_set_json_body_copy,
    set_json_body_w: msg_set_json_body_w,
    set_binary_body: msg_set_binary_body,
    set_binary_body_copy: msg_set_binary_body_copy,
};

#[no_mangle]
pub unsafe extern "C" fn XbtpCreateMessage(iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let hr = XbtpInitialize();
    if hr < 0 {
        return hr;
    }
    let m = Box::into_raw(Box::new(Message { vtbl: &MESSAGE_VTBL, refs: AtomicU32::new(1), lock: Mutex::new(MessageBodies::default()) })) as *mut c_void;
    let hr = com_qi(m, iid, out);
    com_release(m);
    hr
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "system" fn op_sum(a: usize, b: usize, c: usize) -> HRESULT {
        (a + b + c) as HRESULT
    }

    #[test]
    fn create_extension_metadata_handlers() {
        unsafe {
            assert_eq!(XbtpCreateExtensionMetadata(&IID_XBTP_EXTENSION_METADATA, null_mut()), E_INVALIDARG);
            let mut p: *mut c_void = null_mut();
            assert_eq!(XbtpCreateExtensionMetadata(&IID_XBTP_OPERATION_HANDLER, &mut p), E_NOINTERFACE);
            assert!(p.is_null());
            assert_eq!(XbtpCreateExtensionMetadata(&IID_XBTP_EXTENSION_METADATA, &mut p), S_OK);
            let m = p as *mut ExtensionMetadata;
            let v: &MetadataVtbl = vt(p);
            assert_eq!((v.add_function)(m, 14, None), E_INVALIDARG);
            assert_eq!((v.add_function)(m, 14, Some(op_sum)), S_OK);
            assert_eq!((v.add_function)(m, 1, Some(op_sum)), S_OK);
            assert_eq!((v.add_function)(m, 27, Some(op_sum)), S_OK);
            assert_eq!((v.add_function)(m, 1, Some(op_sum)), HR_OBJECT_ALREADY_EXISTS);
            let mut c = 0u32;
            assert_eq!((v.get_handler_count)(m, &mut c), S_OK);
            assert_eq!(c, 3);
            assert_eq!((v.get_handler_count)(m, null_mut()), E_INVALIDARG);
            let mut hs = [null_mut::<c_void>(); 3];
            assert_eq!((v.get_handlers)(m, 23, hs.as_mut_ptr()), HR_INSUFFICIENT_BUFFER);
            assert_eq!((v.get_handlers)(m, 24, hs.as_mut_ptr()), S_OK);
            let ids: Vec<u32> = hs
                .iter()
                .map(|&h| {
                    let mut id = 0;
                    (vt::<HandlerVtbl>(h).get_operation_id)(h, &mut id);
                    id
                })
                .collect();
            assert_eq!(ids, vec![27, 1, 14]);
            assert_eq!((vt::<HandlerVtbl>(hs[0]).invoke)(hs[0], 1, 2, 3), 6);
            let mut q: *mut c_void = null_mut();
            assert_eq!((vt::<HandlerVtbl>(hs[0]).query_interface)(hs[0], &IID_XBTP_OPERATION_HANDLER, &mut q), S_OK);
            com_release(q);
            let list = [null_mut(), hs[1]];
            assert_eq!((v.add_handlers)(m, 2, list.as_ptr()), HR_OBJECT_ALREADY_EXISTS);
            assert_eq!((v.add_handlers)(m, 1, core::ptr::null()), E_INVALIDARG);
            assert_eq!(com_release(p), 0);
        }
    }

    #[test]
    fn create_message_bodies() {
        unsafe {
            assert_eq!(XbtpCreateMessage(&IID_XBTP_MESSAGE, null_mut()), E_INVALIDARG);
            let mut p: *mut c_void = null_mut();
            assert_eq!(XbtpCreateMessage(&IID_XBTP_MESSAGE, &mut p), S_OK);
            let m = p as *mut Message;
            let v: &MessageVtbl = vt(p);
            let mut n = 7u32;
            let mut b: *mut u8 = 1 as *mut u8;
            assert_eq!((v.get_json_body)(m, &mut n, &mut b), S_OK);
            assert_eq!((n, b), (0, null_mut()));
            assert_eq!((v.get_json_body)(m, null_mut(), &mut b), E_INVALIDARG);
            let w = wide("{\"a\":1}");
            assert_eq!((v.set_json_body_w)(m, w.as_ptr()), S_OK);
            assert_eq!((v.get_json_body)(m, &mut n, &mut b), S_OK);
            assert_eq!(core::slice::from_raw_parts(b, n as usize), b"{\"a\":1}\0");
            assert_eq!((v.set_json_body_w)(m, core::ptr::null()), E_INVALIDARG);
            let data = [1u8, 2, 3];
            assert_eq!((v.set_binary_body)(m, 3, data.as_ptr()), S_OK);
            assert_eq!((v.get_binary_body)(m, &mut n, &mut b), S_OK);
            assert_eq!((n, b as *const u8), (3, data.as_ptr()));
            assert_eq!((v.set_binary_body_copy)(m, 3, data.as_ptr()), S_OK);
            assert_eq!((v.get_binary_body)(m, &mut n, &mut b), S_OK);
            assert_ne!(b as *const u8, data.as_ptr());
            assert_eq!(core::slice::from_raw_parts(b, 3), &data);
            assert_eq!((v.set_json_body_copy)(m, 0, data.as_ptr()), S_OK);
            assert_eq!((v.get_json_body)(m, &mut n, &mut b), S_OK);
            assert_eq!((n, b), (0, null_mut()));
            assert_eq!((v.set_json_body)(m, 2, data.as_ptr()), S_OK);
            assert_eq!((v.get_json_body)(m, &mut n, &mut b), S_OK);
            assert_eq!((n, b as *const u8), (2, data.as_ptr()));
            let mut q: *mut c_void = null_mut();
            assert_eq!(com_qi(p, &IID_XBTP_MESSAGE_IMPL, &mut q), S_OK);
            com_release(q);
            assert_eq!(com_qi(p, &IID_XBTP_EXTENSION_METADATA, &mut q), E_NOINTERFACE);
            assert_eq!(com_release(p), 0);
        }
    }
}
