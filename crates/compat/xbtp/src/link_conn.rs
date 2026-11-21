use crate::conn::{channel_base_destroy, channel_base_init, channel_construct, channel_no_detach, channel_rearm, channel_receive_exact, channel_true, channel_wait_idle, resolve_offline, wrap_into, ChannelVtbl, ConnectionVtbl, XbtpChannel, XbtpConnection};
use crate::link::*;
use crate::msvc::{cpp_delete, cpp_new, SharedPtr};
use crate::sys::*;
use core::ffi::c_void;
use core::ptr::null_mut;

pub const LINK_ACCEPT_TIMEOUT_MS: u32 = 4000;

#[repr(C)]
pub struct XbtpLinkConnection {
    pub vtbl: *const ConnectionVtbl,
    pub marker: i64,
    pub handle: *mut u8,
    pub ready_event: HANDLE,
    pub server_name: Vec<u16>,
    pub port: u32,
}

pub static LINK_CONNECTION_VTBL: ConnectionVtbl = ConnectionVtbl {
    dtor: link_connection_deleting_dtor,
    connect: link_connection_connect,
    open_redirect: link_connection_open_redirect,
    accept: link_connection_accept,
    get_event: link_connection_event,
};

pub static LINK_CHANNEL_VTBL: ChannelVtbl = ChannelVtbl {
    dtor: link_channel_dtor,
    receive: channel_receive_exact,
    get_address: link_channel_get_address,
    is_network: channel_true,
    detach: channel_no_detach,
    send_bytes: link_channel_send_bytes,
    recv_bytes: link_channel_recv_bytes,
    decode: link_channel_decode,
    cancel: link_channel_cancel,
};

unsafe fn link_connection_new() -> *mut XbtpLinkConnection {
    let c = cpp_new(core::mem::size_of::<XbtpLinkConnection>()) as *mut XbtpLinkConnection;
    if !c.is_null() {
        core::ptr::write(c, XbtpLinkConnection { vtbl: &LINK_CONNECTION_VTBL, marker: -1, handle: null_mut(), ready_event: null_mut(), server_name: Vec::new(), port: 0 });
    }
    c
}

pub unsafe fn link_connection_destroy(c: *mut XbtpLinkConnection) {
    (*c).vtbl = &LINK_CONNECTION_VTBL;
    if !(*c).handle.is_null() {
        XbtpLinkCloseHandle((*c).handle);
        (*c).handle = null_mut();
    }
    core::ptr::drop_in_place(&mut (*c).server_name);
}

unsafe extern "C" fn link_connection_deleting_dtor(this: *mut c_void, flags: u32) -> *mut c_void {
    link_connection_destroy(this as *mut XbtpLinkConnection);
    if flags & 1 != 0 {
        cpp_delete(this);
    }
    this
}

pub unsafe fn link_initialize_client(name: *const u16, port: u32, out: *mut SharedPtr<XbtpConnection>) -> HRESULT {
    let c = link_connection_new();
    if c.is_null() {
        return E_OUTOFMEMORY;
    }
    let n = wcslen(name);
    (*c).server_name = core::slice::from_raw_parts(name, n).to_vec();
    (*c).port = port;
    wrap_into(c as *mut XbtpConnection, out);
    S_OK
}

pub unsafe fn link_initialize_server(port: u32, out: *mut SharedPtr<XbtpConnection>) -> HRESULT {
    let c = link_connection_new();
    if c.is_null() {
        return E_OUTOFMEMORY;
    }
    (*c).handle = XbtpLinkCreateHandle();
    if (*c).handle.is_null() {
        link_connection_deleting_dtor(c as *mut c_void, 1);
        return E_OUTOFMEMORY;
    }
    let hr = XbtpLinkListen((*c).handle, 1usize as *mut c_void, port as u16, &mut (*c).ready_event);
    if hr < 0 {
        XbtpLinkCloseHandle((*c).handle);
        (*c).handle = null_mut();
        (*c).ready_event = CreateEventW(null_mut(), 1, 0, core::ptr::null());
    }
    wrap_into(c as *mut XbtpConnection, out);
    S_OK
}

unsafe fn new_link_channel(h: *mut u8, client: bool, out: *mut *mut XbtpChannel) -> HRESULT {
    let ch = channel_construct(&LINK_CHANNEL_VTBL, 0);
    if ch.is_null() {
        XbtpLinkCloseHandle(h);
        return E_OUTOFMEMORY;
    }
    let hr = channel_base_init(ch, client);
    if hr < 0 {
        XbtpLinkCloseHandle(h);
        link_channel_dtor(ch, 1);
        return hr;
    }
    (*ch).handle = h as usize;
    (*ch).started = 1;
    channel_rearm(ch);
    *out = ch;
    S_OK
}

unsafe extern "C" fn link_connection_connect(this: *mut c_void, _timeout: u32, out: *mut *mut XbtpChannel) -> HRESULT {
    let c = this as *mut XbtpLinkConnection;
    *out = null_mut();
    let mut name = (*c).server_name.clone();
    name.push(0);
    let addr = match resolve_offline(name.as_ptr(), (*c).port) {
        Ok(a) => a,
        Err(hr) => return hr,
    };
    let h = XbtpLinkCreateHandle();
    if h.is_null() {
        return E_OUTOFMEMORY;
    }
    let hr = XbtpLinkConnect(h, addr.addr.as_ptr(), addr.size, 0);
    if hr < 0 {
        XbtpLinkCloseHandle(h);
        return hr;
    }
    new_link_channel(h, true, out)
}

unsafe extern "C" fn link_connection_open_redirect(_this: *mut c_void, _location: *const u16, _out: *mut *mut c_void) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "C" fn link_connection_accept(this: *mut c_void, out: *mut *mut XbtpChannel) -> HRESULT {
    let c = this as *mut XbtpLinkConnection;
    if (*c).handle.is_null() {
        return E_INVALIDARG;
    }
    let mut h: *mut u8 = null_mut();
    let hr = XbtpLinkAccept((*c).handle, &mut h, LINK_ACCEPT_TIMEOUT_MS);
    if hr < 0 {
        if !h.is_null() {
            XbtpLinkCloseHandle(h);
        }
        return hr;
    }
    new_link_channel(h, false, out)
}

unsafe extern "C" fn link_connection_event(this: *mut c_void, out: *mut HANDLE) -> HRESULT {
    *out = (*(this as *mut XbtpLinkConnection)).ready_event;
    S_OK
}

unsafe extern "C" fn link_channel_dtor(ch: *mut XbtpChannel, flags: u32) -> *mut XbtpChannel {
    let h = (*ch).handle as *mut u8;
    if !h.is_null() {
        XbtpLinkShutdownHandle(h);
    }
    channel_wait_idle(ch);
    if !h.is_null() {
        XbtpLinkCloseHandle(h);
        (*ch).handle = 0;
    }
    channel_base_destroy(ch);
    if flags & 1 != 0 {
        cpp_delete(ch as *mut c_void);
    }
    ch
}

unsafe extern "C" fn link_channel_get_address(_ch: *mut XbtpChannel, out: *mut BSTR) -> HRESULT {
    let w = wide("xbtplink");
    *out = SysAllocString(w.as_ptr());
    if (*out).is_null() {
        E_OUTOFMEMORY
    } else {
        S_OK
    }
}

unsafe extern "C" fn link_channel_send_bytes(ch: *mut XbtpChannel, n: u32, bufs: *const WSABUF, ov: *mut OVERLAPPED) -> HRESULT {
    let mut sent = 0u32;
    XbtpLinkSend((*ch).handle as *mut u8, bufs, n, &mut sent, 0, ov)
}

unsafe extern "C" fn link_channel_recv_bytes(ch: *mut XbtpChannel, len: u32, buf: *mut u8, ov: *mut OVERLAPPED) -> HRESULT {
    let h = (*ch).handle as *mut u8;
    if h.is_null() {
        return LINK_E_NULL_HANDLE;
    }
    let mut got = 0u32;
    recv_bytes(h, buf, len, &mut got, 0, ov, u32::MAX)
}

unsafe extern "C" fn link_channel_decode(ch: *mut XbtpChannel, ov: *mut OVERLAPPED) -> HRESULT {
    let mut n = 0u32;
    XbtpLinkGetOverlappedResult((*ch).handle as *mut u8, ov, &mut n)
}

unsafe extern "C" fn link_channel_cancel(ch: *mut XbtpChannel, _ov: *mut OVERLAPPED) -> i32 {
    XbtpLinkShutdownHandle((*ch).handle as *mut u8)
}
