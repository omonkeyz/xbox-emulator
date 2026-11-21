use crate::base::{log_error_args, log_info_args};
use crate::link::{closesocket, shutdown, FreeAddrInfoW, GetAddrInfoW, WSAGetLastError, WSARecv, WSASend, ADDRINFOW, INVALID_SOCKET, SOCKET, WSABUF};
use crate::msvc::{cpp_delete, cpp_new, SharedPtr};
use crate::sys::*;
use crate::xbtp2_sys::{channel_submit, ChannelRequest, STATUS_CANCELLED, TCP_BYTES_RECEIVED, TCP_BYTES_SENT};
use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicI32, Ordering};

pub const E_COULD_NOT_CONNECT: HRESULT = 0x8c11_0303u32 as i32;
pub const E_UNEXPECTED_SERVER_SOCKET: HRESULT = 0x8c11_0305u32 as i32;
pub const E_INVALID_REDIRECT_FORMAT: HRESULT = 0x8c11_0308u32 as i32;
pub const E_SOCKET_CLOSED: HRESULT = 0x8c11_030au32 as i32;
pub const E_INCOMPLETE_BUFFER_RECEIVE: HRESULT = 0x8c11_030bu32 as i32;
pub const PREFETCH_SIZE: u32 = 0x40;
pub const MID_MESSAGE_TIMEOUT: u32 = 60_000;
pub const TCP_REDIRECT_PREFIX: &str = "xbtp::tcp:";

const STATUS_PENDING: usize = 0x103;
const WAIT_TIMEOUT: u32 = 0x102;
const INFINITE: u32 = 0xFFFF_FFFF;
const FD_CONNECT: i32 = 0x10;
const FD_ACCEPT_CLOSE: i32 = 0x28;

#[repr(C)]
pub struct CriticalSection(pub [u8; 0x28]);

#[link(name = "kernel32")]
extern "system" {
    fn InitializeCriticalSection(cs: *mut CriticalSection);
    fn DeleteCriticalSection(cs: *mut CriticalSection);
    fn EnterCriticalSection(cs: *mut CriticalSection);
    fn LeaveCriticalSection(cs: *mut CriticalSection);
    fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
    fn SetNamedPipeHandleState(h: HANDLE, mode: *mut u32, max: *mut u32, timeout: *mut u32) -> i32;
    fn CreateNamedPipeW(name: *const u16, open: u32, mode: u32, max: u32, out: u32, inb: u32, timeout: u32, sa: *mut c_void) -> HANDLE;
    fn ConnectNamedPipe(h: HANDLE, ov: *mut OVERLAPPED) -> i32;
}

#[repr(C)]
pub struct FdSet {
    pub count: u32,
    pub sockets: [SOCKET; 64],
}

#[repr(C)]
pub struct TimeVal {
    pub sec: i32,
    pub usec: i32,
}

#[repr(C)]
pub struct NetworkEvents {
    pub events: i32,
    pub errors: [i32; 10],
}

#[allow(clashing_extern_declarations)]
#[link(name = "ws2_32")]
extern "system" {
    fn WSASocketW(af: i32, ty: i32, proto: i32, info: *mut c_void, g: u32, flags: u32) -> SOCKET;
    fn ioctlsocket(s: SOCKET, cmd: i32, arg: *mut u32) -> i32;
    fn setsockopt(s: SOCKET, level: i32, name: i32, val: *const u8, len: i32) -> i32;
    fn getsockopt(s: SOCKET, level: i32, name: i32, val: *mut u8, len: *mut i32) -> i32;
    fn bind(s: SOCKET, addr: *const u8, len: i32) -> i32;
    fn listen(s: SOCKET, backlog: i32) -> i32;
    fn WSAIoctl(s: SOCKET, code: u32, inb: *const c_void, inl: u32, outb: *mut c_void, outl: u32, ret: *mut u32, ov: *mut OVERLAPPED, cr: *const c_void) -> i32;
    fn WSAConnect(s: SOCKET, addr: *const u8, len: i32, a: *mut c_void, b: *mut c_void, c: *mut c_void, d: *mut c_void) -> i32;
    fn WSAEventSelect(s: SOCKET, ev: HANDLE, mask: i32) -> i32;
    fn WSAEnumNetworkEvents(s: SOCKET, ev: HANDLE, out: *mut NetworkEvents) -> i32;
    fn WSAAccept(s: SOCKET, addr: *mut u8, len: *mut i32, cond: *const c_void, ctx: usize) -> SOCKET;
    fn select(n: i32, r: *mut FdSet, w: *mut FdSet, e: *mut FdSet, tv: *const TimeVal) -> i32;
    fn getsockname(s: SOCKET, addr: *mut u8, len: *mut i32) -> i32;
    fn getpeername(s: SOCKET, addr: *mut u8, len: *mut i32) -> i32;
}

pub fn map_wsa_error(e: i32) -> HRESULT {
    match e {
        0x2746 | 0x2af9 | 0x2afa | 0x2750 | 0x2751 | 0x35 | 0x40 => E_COULD_NOT_CONNECT,
        _ => hr_from_win32(e as u32),
    }
}

unsafe fn last_error_hr() -> HRESULT {
    hr_from_win32(GetLastError())
}

pub type SendCallback = unsafe extern "C" fn(u32, u32, u32, *mut c_void);

#[repr(C)]
pub struct ChannelVtbl {
    pub dtor: unsafe extern "C" fn(*mut XbtpChannel, u32) -> *mut XbtpChannel,
    pub receive: unsafe extern "C" fn(*mut XbtpChannel, u32, u32, *mut u8) -> HRESULT,
    pub get_address: unsafe extern "C" fn(*mut XbtpChannel, *mut BSTR) -> HRESULT,
    pub is_network: unsafe extern "C" fn(*mut XbtpChannel) -> bool,
    pub detach: unsafe extern "C" fn(*mut XbtpChannel, *mut usize) -> bool,
    pub send_bytes: unsafe extern "C" fn(*mut XbtpChannel, u32, *const WSABUF, *mut OVERLAPPED) -> HRESULT,
    pub recv_bytes: unsafe extern "C" fn(*mut XbtpChannel, u32, *mut u8, *mut OVERLAPPED) -> HRESULT,
    pub decode: unsafe extern "C" fn(*mut XbtpChannel, *mut OVERLAPPED) -> HRESULT,
    pub cancel: unsafe extern "C" fn(*mut XbtpChannel, *mut OVERLAPPED) -> i32,
}

#[repr(C)]
pub struct XbtpChannel {
    pub vtbl: *const ChannelVtbl,
    pub is_client: u8,
    pub pad: [u8; 3],
    pub exchange: AtomicI32,
    pub recv_cs: CriticalSection,
    pub ov: OVERLAPPED,
    pub next: *mut XbtpChannel,
    pub recv_event: HANDLE,
    pub read_event: HANDLE,
    pub buffer: [u8; 0x40],
    pub pending: u8,
    pub result: i32,
    pub have: u32,
    pub used: u32,
    pub closed: u8,
    pub started: u8,
    pub send_cs: CriticalSection,
    pub send_event: HANDLE,
    pub handle: usize,
    pub request: ChannelRequest,
}

pub static TCP_CHANNEL_VTBL: ChannelVtbl = ChannelVtbl {
    dtor: tcp_channel_dtor,
    receive: tcp_channel_receive,
    get_address: tcp_channel_get_address,
    is_network: channel_true,
    detach: tcp_channel_detach,
    send_bytes: tcp_channel_send_bytes,
    recv_bytes: tcp_channel_recv_bytes,
    decode: channel_decode,
    cancel: channel_cancel,
};

pub static NP_CHANNEL_VTBL: ChannelVtbl = ChannelVtbl {
    dtor: np_channel_dtor,
    receive: channel_receive_exact,
    get_address: channel_no_address,
    is_network: channel_false,
    detach: channel_no_detach,
    send_bytes: np_channel_send_bytes,
    recv_bytes: np_channel_recv_bytes,
    decode: channel_decode,
    cancel: channel_cancel,
};

pub(crate) unsafe fn channel_construct(vtbl: &'static ChannelVtbl, handle: usize) -> *mut XbtpChannel {
    let ch = cpp_new(core::mem::size_of::<XbtpChannel>()) as *mut XbtpChannel;
    if ch.is_null() {
        return ch;
    }
    InitializeCriticalSection(&mut (*ch).recv_cs);
    InitializeCriticalSection(&mut (*ch).send_cs);
    (*ch).vtbl = vtbl;
    (*ch).handle = handle;
    (*ch).request.process = channel_issue_prefetch;
    (*ch).request.context = ch as *mut c_void;
    ch
}

pub unsafe fn new_tcp_channel() -> *mut XbtpChannel {
    channel_construct(&TCP_CHANNEL_VTBL, INVALID_SOCKET)
}

pub unsafe fn new_np_channel() -> *mut XbtpChannel {
    channel_construct(&NP_CHANNEL_VTBL, 0)
}

pub(crate) unsafe fn channel_base_init(ch: *mut XbtpChannel, client: bool) -> HRESULT {
    (*ch).is_client = client as u8;
    (*ch).recv_event = CreateEventW(null_mut(), 1, 0, core::ptr::null());
    if (*ch).recv_event.is_null() {
        return E_OUTOFMEMORY;
    }
    (*ch).read_event = CreateEventW(null_mut(), 1, 0, core::ptr::null());
    if (*ch).read_event.is_null() {
        return E_OUTOFMEMORY;
    }
    (*ch).send_event = CreateEventW(null_mut(), 1, 0, core::ptr::null());
    if (*ch).send_event.is_null() {
        return E_OUTOFMEMORY;
    }
    S_OK
}

pub unsafe fn tcp_channel_initialize(ch: *mut XbtpChannel, s: SOCKET, client: bool) -> HRESULT {
    let hr = channel_base_init(ch, client);
    if hr < 0 {
        return hr;
    }
    (*ch).handle = s;
    if (*ch).is_client != 0 {
        (*ch).started = 1;
        channel_rearm(ch);
    }
    hr
}

pub unsafe fn np_channel_initialize(ch: *mut XbtpChannel, h: HANDLE, client: bool) -> HRESULT {
    let hr = channel_base_init(ch, client);
    if hr < 0 {
        return hr;
    }
    (*ch).handle = h as usize;
    (*ch).started = 1;
    channel_rearm(ch);
    hr
}

unsafe fn channel_issue_prefetch(r: *mut ChannelRequest) -> HRESULT {
    let ch = (*r).context as *mut XbtpChannel;
    ((*(*ch).vtbl).recv_bytes)(ch, PREFETCH_SIZE, (*ch).buffer.as_mut_ptr(), &mut (*ch).ov)
}

pub unsafe fn channel_rearm(ch: *mut XbtpChannel) {
    EnterCriticalSection(&mut (*ch).recv_cs);
    if (*ch).started != 0 && (*ch).pending == 0 && (*ch).closed == 0 && (*ch).used >= (*ch).have {
        (*ch).ov.internal_high = 0;
        (*ch).ov.offset = 0;
        (*ch).ov.offset_high = 0;
        (*ch).ov.event = (*ch).recv_event;
        (*ch).have = 0;
        (*ch).used = 0;
        (*ch).ov.internal = STATUS_PENDING;
        (*ch).result = E_PENDING;
        (*ch).pending = 1;
        ResetEvent((*ch).recv_event);
        (*ch).request.status = 0;
        (*ch).request.event = (*ch).recv_event;
        (*ch).request.next = null_mut();
        channel_submit(&mut (*ch).request);
    }
    LeaveCriticalSection(&mut (*ch).recv_cs);
}

pub(crate) unsafe fn channel_wait_idle(ch: *mut XbtpChannel) {
    EnterCriticalSection(&mut (*ch).recv_cs);
    if (*ch).pending != 0 {
        WaitForSingleObject((*ch).recv_event, INFINITE);
    }
    LeaveCriticalSection(&mut (*ch).recv_cs);
}

pub(crate) unsafe fn channel_base_destroy(ch: *mut XbtpChannel) {
    DeleteCriticalSection(&mut (*ch).recv_cs);
    DeleteCriticalSection(&mut (*ch).send_cs);
    for h in [(*ch).recv_event, (*ch).read_event, (*ch).send_event] {
        if !h.is_null() {
            CloseHandle(h);
        }
    }
}

pub unsafe extern "C" fn channel_receive_exact(ch: *mut XbtpChannel, timeout: u32, len: u32, buf: *mut u8) -> HRESULT {
    if len == 0 {
        return S_OK;
    }
    let mut left = len;
    let mut out = buf;
    let mut wait_ms = timeout;
    EnterCriticalSection(&mut (*ch).recv_cs);
    let hr = 'run: {
        if (*ch).pending != 0 {
            if WaitForSingleObject((*ch).recv_event, timeout) == WAIT_TIMEOUT {
                break 'run E_PENDING;
            }
            (*ch).pending = 0;
            if (*ch).request.status == STATUS_CANCELLED {
                (*ch).ov.internal = STATUS_CANCELLED as usize;
            }
            let r = ((*(*ch).vtbl).decode)(ch, &mut (*ch).ov);
            (*ch).result = r;
            (*ch).have = (*ch).ov.internal_high as u32;
            if r < 0 {
                break 'run r;
            }
            if (*ch).have == 0 {
                (*ch).closed = 1;
            }
        }
        if (*ch).closed != 0 {
            break 'run E_SOCKET_CLOSED;
        }
        if (*ch).have > PREFETCH_SIZE {
            (*ch).have = PREFETCH_SIZE;
        }
        if (*ch).have > (*ch).used {
            let n = ((*ch).have - (*ch).used).min(left);
            core::ptr::copy_nonoverlapping((*ch).buffer.as_ptr().add((*ch).used as usize), out, n as usize);
            (*ch).used += n;
            left -= n;
            out = out.add(n as usize);
            if (*ch).used >= (*ch).have {
                (*ch).used = 0;
                (*ch).have = 0;
                ResetEvent((*ch).recv_event);
            }
            wait_ms = MID_MESSAGE_TIMEOUT;
        }
        while left != 0 {
            let mut ov = OVERLAPPED { event: (*ch).read_event, ..Default::default() };
            ResetEvent((*ch).read_event);
            let r = ((*(*ch).vtbl).recv_bytes)(ch, left, out, &mut ov);
            if r == E_PENDING {
                if WaitForSingleObject((*ch).read_event, wait_ms) == WAIT_TIMEOUT {
                    ((*(*ch).vtbl).cancel)(ch, &mut ov);
                    WaitForSingleObject((*ch).read_event, INFINITE);
                    break 'run E_INCOMPLETE_BUFFER_RECEIVE;
                }
                wait_ms = MID_MESSAGE_TIMEOUT;
            }
            let r = ((*(*ch).vtbl).decode)(ch, &mut ov);
            if r < 0 {
                break 'run r;
            }
            let got = ov.internal_high as u32;
            if got == 0 {
                (*ch).closed = 1;
                break 'run E_SOCKET_CLOSED;
            }
            if got > left {
                left = 0;
            } else {
                left -= got;
                out = out.add(got as usize);
            }
        }
        S_OK
    };
    channel_rearm(ch);
    LeaveCriticalSection(&mut (*ch).recv_cs);
    hr
}

pub unsafe fn channel_send(ch: *mut XbtpChannel, bufs: &[WSABUF], cb: Option<SendCallback>, ctx: *mut c_void) -> HRESULT {
    let total: u32 = bufs.iter().map(|b| b.len).sum();
    EnterCriticalSection(&mut (*ch).send_cs);
    ResetEvent((*ch).send_event);
    let mut ov = OVERLAPPED { event: (*ch).send_event, ..Default::default() };
    let r = ((*(*ch).vtbl).send_bytes)(ch, bufs.len() as u32, bufs.as_ptr(), &mut ov);
    if r < 0 && r != E_PENDING {
        SetEvent((*ch).send_event);
    }
    WaitForSingleObject((*ch).send_event, INFINITE);
    let hr = ((*(*ch).vtbl).decode)(ch, &mut ov);
    LeaveCriticalSection(&mut (*ch).send_cs);
    if let Some(f) = cb {
        f(total, 0, if hr < 0 { 0x57 } else { 0 }, ctx);
    }
    hr
}

pub unsafe fn channel_receive(ch: *mut XbtpChannel, timeout: u32, len: u32, buf: *mut u8) -> HRESULT {
    ((*(*ch).vtbl).receive)(ch, timeout, len, buf)
}

pub unsafe fn channel_delete(ch: *mut XbtpChannel) {
    if !ch.is_null() {
        ((*(*ch).vtbl).dtor)(ch, 1);
    }
}

pub unsafe fn channel_next_exchange_id(ch: *mut XbtpChannel) -> i32 {
    if (*ch).is_client != 0 {
        (*ch).exchange.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        (*ch).exchange.fetch_sub(1, Ordering::SeqCst) - 1
    }
}

pub(crate) unsafe extern "C" fn channel_true(_ch: *mut XbtpChannel) -> bool {
    true
}

unsafe extern "C" fn channel_false(_ch: *mut XbtpChannel) -> bool {
    false
}

unsafe extern "C" fn channel_no_address(_ch: *mut XbtpChannel, _out: *mut BSTR) -> HRESULT {
    E_NOTIMPL
}

pub(crate) unsafe extern "C" fn channel_no_detach(_ch: *mut XbtpChannel, _out: *mut usize) -> bool {
    false
}

unsafe extern "C" fn channel_decode(ch: *mut XbtpChannel, ov: *mut OVERLAPPED) -> HRESULT {
    let mut n = 0u32;
    if GetOverlappedResult((*ch).handle as HANDLE, ov, &mut n, 0) == 0 {
        return last_error_hr();
    }
    S_OK
}

unsafe extern "C" fn channel_cancel(ch: *mut XbtpChannel, ov: *mut OVERLAPPED) -> i32 {
    CancelIoEx((*ch).handle as HANDLE, ov)
}

unsafe extern "C" fn tcp_channel_dtor(ch: *mut XbtpChannel, flags: u32) -> *mut XbtpChannel {
    let s = (*ch).handle;
    if s != INVALID_SOCKET {
        shutdown(s, 1);
        if (*ch).pending != 0 && (*ch).ov.internal == STATUS_PENDING {
            CancelIoEx(s as HANDLE, &mut (*ch).ov);
        }
    }
    channel_wait_idle(ch);
    if (*ch).handle != INVALID_SOCKET {
        closesocket((*ch).handle);
    }
    channel_base_destroy(ch);
    if flags & 1 != 0 {
        cpp_delete(ch as *mut c_void);
    }
    ch
}

unsafe extern "C" fn np_channel_dtor(ch: *mut XbtpChannel, flags: u32) -> *mut XbtpChannel {
    if (*ch).handle != 0 {
        CloseHandle((*ch).handle as HANDLE);
        (*ch).handle = 0;
    }
    channel_wait_idle(ch);
    channel_base_destroy(ch);
    if flags & 1 != 0 {
        cpp_delete(ch as *mut c_void);
    }
    ch
}

unsafe extern "C" fn tcp_channel_receive(ch: *mut XbtpChannel, timeout: u32, len: u32, buf: *mut u8) -> HRESULT {
    let hr = channel_receive_exact(ch, timeout, len, buf);
    if hr >= 0 {
        TCP_BYTES_RECEIVED.fetch_add(len as u64, Ordering::SeqCst);
    }
    hr
}

unsafe extern "C" fn tcp_channel_detach(ch: *mut XbtpChannel, out: *mut usize) -> bool {
    *out = (*ch).handle;
    if (*ch).handle == INVALID_SOCKET {
        return false;
    }
    (*ch).handle = INVALID_SOCKET;
    true
}

unsafe extern "C" fn tcp_channel_send_bytes(ch: *mut XbtpChannel, n: u32, bufs: *const WSABUF, ov: *mut OVERLAPPED) -> HRESULT {
    let mut sent = 0u32;
    let mut hr = S_OK;
    if WSASend((*ch).handle, bufs, n, &mut sent, 0, ov, core::ptr::null()) == -1 {
        let e = GetLastError();
        hr = if e == 0x3e5 { E_PENDING } else { hr_from_win32(e) };
        if hr < 0 && hr != E_PENDING {
            return hr;
        }
    }
    TCP_BYTES_SENT.fetch_add(sent as u64, Ordering::SeqCst);
    hr
}

unsafe extern "C" fn tcp_channel_recv_bytes(ch: *mut XbtpChannel, len: u32, buf: *mut u8, ov: *mut OVERLAPPED) -> HRESULT {
    let b = WSABUF { len, buf };
    let mut got = 0u32;
    let mut flags = 0u32;
    if WSARecv((*ch).handle, &b, 1, &mut got, &mut flags, ov, core::ptr::null()) == -1 {
        let e = GetLastError();
        return if e == 0x3e5 { E_PENDING } else { hr_from_win32(e) };
    }
    S_OK
}

unsafe extern "C" fn np_channel_send_bytes(ch: *mut XbtpChannel, n: u32, bufs: *const WSABUF, ov: *mut OVERLAPPED) -> HRESULT {
    let mut hr = S_OK;
    for i in 0..n as usize {
        let b = *bufs.add(i);
        let mut local = OVERLAPPED { event: (*ch).send_event, ..Default::default() };
        ResetEvent((*ch).send_event);
        let mut written = 0u32;
        if WriteFile((*ch).handle as HANDLE, b.buf as *const c_void, b.len, &mut written, &mut local) == 0 {
            let e = GetLastError();
            if e == 0x3e5 {
                WaitForSingleObject((*ch).send_event, INFINITE);
                hr = ((*(*ch).vtbl).decode)(ch, &mut local);
                if hr < 0 {
                    return hr;
                }
                (*ov).internal_high += local.internal_high;
            } else {
                hr = hr_from_win32(e);
                if hr < 0 {
                    return hr;
                }
            }
        }
    }
    hr
}

unsafe extern "C" fn np_channel_recv_bytes(ch: *mut XbtpChannel, len: u32, buf: *mut u8, ov: *mut OVERLAPPED) -> HRESULT {
    let mut got = 0u32;
    if ReadFile((*ch).handle as HANDLE, buf as *mut c_void, len, &mut got, ov) == 0 {
        let e = GetLastError();
        return if e == 0x3e5 { E_PENDING } else { hr_from_win32(e) };
    }
    S_OK
}

unsafe extern "C" fn tcp_channel_get_address(ch: *mut XbtpChannel, out: *mut BSTR) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    *out = null_mut();
    let mut addr = [0u8; 0x80];
    let mut len = 0x80i32;
    if getpeername((*ch).handle, addr.as_mut_ptr(), &mut len) != 0 {
        return map_wsa_error(WSAGetLastError());
    }
    match ip_address_text(&addr) {
        Some((host, _port)) => {
            let b = bstr_from(&host);
            if b.is_null() {
                return E_OUTOFMEMORY;
            }
            *out = b;
            S_OK
        }
        None => E_INVALIDARG,
    }
}

pub fn ip_address_text(addr: &[u8]) -> Option<(String, String)> {
    let family = u16::from_le_bytes([addr[0], addr[1]]);
    let port = u16::from_be_bytes([addr[2], addr[3]]);
    match family {
        2 => Some((format!("{}.{}.{}.{}", addr[4], addr[5], addr[6], addr[7]), format!("{}", port))),
        0x17 => {
            let w: Vec<String> = (0..8).map(|i| format!("{:04X}", u16::from_be_bytes([addr[8 + i * 2], addr[9 + i * 2]]))).collect();
            Some((w.join(":"), format!("{}", port)))
        }
        _ => None,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SocketAddress {
    pub size: u32,
    pub addr: [u8; 0x1c],
}

pub unsafe fn resolve(host: *const u16, port: u32) -> Result<SocketAddress, HRESULT> {
    let service = wide(&format!("{}", port));
    let mut hints: ADDRINFOW = core::mem::zeroed();
    hints.ai_flags = 1;
    hints.ai_socktype = 1;
    hints.ai_protocol = 6;
    let name: Vec<u16>;
    let node: *const u16 = if host.is_null() || *host == 0 {
        hints.ai_family = 0x17;
        core::ptr::null()
    } else {
        let text = from_wide(host);
        name = wide(text.trim_matches(|c: char| c.is_whitespace()));
        name.as_ptr()
    };
    let mut res: *mut ADDRINFOW = null_mut();
    let rc = GetAddrInfoW(node, service.as_ptr(), &hints, &mut res);
    if rc != 0 {
        if !res.is_null() {
            FreeAddrInfoW(res);
        }
        return Err(hr_from_win32(rc as u32));
    }
    let mut v4: *mut ADDRINFOW = null_mut();
    let mut v6: *mut ADDRINFOW = null_mut();
    let mut link_local: *mut ADDRINFOW = null_mut();
    let mut p = res;
    while !p.is_null() && v4.is_null() {
        if (*p).ai_family == 2 {
            if (*p).ai_addrlen == 0x10 {
                v4 = p;
            }
        } else if (*p).ai_family == 0x17 && (*p).ai_addrlen == 0x1c {
            let a = (*p).ai_addr;
            if *a.add(8) == 0xfe && (*a.add(9) & 0xc0) == 0x80 {
                if link_local.is_null() {
                    link_local = p;
                }
            } else if v6.is_null() {
                v6 = p;
            }
        }
        p = (*p).ai_next;
    }
    let pick = if !v4.is_null() { v4 } else if !v6.is_null() { v6 } else { link_local };
    let out = if pick.is_null() {
        Err(E_UNEXPECTED)
    } else {
        let mut sa = SocketAddress { size: (*pick).ai_addrlen as u32, addr: [0; 0x1c] };
        core::ptr::copy_nonoverlapping((*pick).ai_addr, sa.addr.as_mut_ptr(), (*pick).ai_addrlen.min(0x1c));
        Ok(sa)
    };
    if !res.is_null() {
        FreeAddrInfoW(res);
    }
    out
}

pub unsafe fn resolve_offline(host: *const u16, port: u32) -> Result<SocketAddress, HRESULT> {
    if matches!(crate::offline::classify_wide(host), crate::offline::HostClass::RemoteName) {
        return Err(crate::offline::WSAHOST_NOT_FOUND_HR);
    }
    resolve(host, port)
}

unsafe fn set_option(s: SOCKET, level: i32, name: i32, value: u32) -> HRESULT {
    if setsockopt(s, level, name, &value as *const u32 as *const u8, 4) == -1 {
        return last_error_hr();
    }
    S_OK
}

pub unsafe fn create_socket(af: u16) -> Result<SOCKET, HRESULT> {
    let s = WSASocketW(af as i32, 1, 6, null_mut(), 0, 1);
    if s == INVALID_SOCKET {
        let e = GetLastError();
        if e != 0 {
            return Err(hr_from_win32(e));
        }
    }
    let fail = |hr: HRESULT| -> Result<SOCKET, HRESULT> {
        if s != INVALID_SOCKET {
            closesocket(s);
        }
        Err(hr)
    };
    let mut nb = 1u32;
    if ioctlsocket(s, 0x8004667eu32 as i32, &mut nb) == -1 {
        let e = GetLastError();
        if e != 0 {
            return fail(hr_from_win32(e));
        }
    }
    let hr = set_option(s, 0xffff, 0x3002, 0);
    if hr < 0 {
        return fail(hr);
    }
    let hr = set_option(s, 0xffff, 1, 1);
    if hr < 0 {
        return fail(hr);
    }
    let hr = set_option(s, 0xffff, 0x80, 0);
    if hr < 0 {
        return fail(hr);
    }
    if af == 0x17 {
        let hr = set_option(s, 0x29, 0x1b, 0);
        if hr < 0 {
            return fail(hr);
        }
    }
    Ok(s)
}

unsafe fn close_socket_full(s: SOCKET) {
    if s != INVALID_SOCKET {
        shutdown(s, 2);
        closesocket(s);
    }
}

pub unsafe fn socket_listen(addr: &SocketAddress) -> Result<SOCKET, HRESULT> {
    let s = create_socket(u16::from_le_bytes([addr.addr[0], addr.addr[1]]))?;
    let mut hr = S_OK;
    if bind(s, addr.addr.as_ptr(), addr.size as i32) == -1 {
        let e = GetLastError();
        hr = hr_from_win32(e);
        if e != 0 {
            close_socket_full(s);
            return Err(hr);
        }
    }
    if listen(s, 0x7fff_ffff) == -1 {
        hr = last_error_hr();
    }
    if hr < 0 {
        close_socket_full(s);
        return Err(hr);
    }
    Ok(s)
}

pub unsafe fn socket_connect(addr: &SocketAddress) -> Result<SOCKET, HRESULT> {
    let s = create_socket(u16::from_le_bytes([addr.addr[0], addr.addr[1]]))?;
    let keepalive: [u32; 3] = [1, 50_000, 2_000];
    let mut ret = 0u32;
    if WSAIoctl(s, 0x9800_0004, keepalive.as_ptr() as *const c_void, 12, null_mut(), 0, &mut ret, null_mut(), core::ptr::null()) != 0 {
        close_socket_full(s);
        return Err(E_FAIL);
    }
    if WSAConnect(s, addr.addr.as_ptr(), addr.size as i32, null_mut(), null_mut(), null_mut(), null_mut()) == -1 {
        let e = GetLastError();
        if e != 0x2733 {
            let hr = hr_from_win32(e);
            if hr < 0 {
                close_socket_full(s);
                return Err(hr);
            }
        }
    }
    Ok(s)
}

pub unsafe fn socket_check_ready(s: SOCKET) -> Result<u32, HRESULT> {
    let mut r: FdSet = core::mem::zeroed();
    let mut w: FdSet = core::mem::zeroed();
    let mut x: FdSet = core::mem::zeroed();
    r.count = 1;
    r.sockets[0] = s;
    w.count = 1;
    w.sockets[0] = s;
    x.count = 1;
    x.sockets[0] = s;
    let tv = TimeVal { sec: 0, usec: 0 };
    if select(0, &mut r, &mut w, &mut x, &tv) == -1 {
        let e = GetLastError();
        if e != 0 {
            return Err(hr_from_win32(e));
        }
    }
    if x.count != 0 {
        let mut err = 0i32;
        let mut size = 4i32;
        if getsockopt(x.sockets[0], 0xffff, 0x1007, &mut err as *mut i32 as *mut u8, &mut size) == -1 {
            let e = GetLastError();
            if e != 0 {
                return Err(hr_from_win32(e));
            }
        }
        if err != 0 {
            let hr = hr_from_win32(err as u32);
            if hr != 0 {
                return Err(hr);
            }
        }
        return Err(0x8007_2746u32 as i32);
    }
    let mut flags = 0u32;
    if r.count != 0 {
        flags = 1;
    }
    if w.count != 0 {
        flags |= 2;
    }
    Ok(flags)
}

fn addr_is_local(a: &SocketAddress) -> bool {
    let fam = u16::from_le_bytes([a.addr[0], a.addr[1]]);
    if fam == 0x17 && a.addr[8..24].iter().all(|b| *b == 0) {
        return true;
    }
    unsafe { crate::offline::sockaddr_is_loopback(a.addr.as_ptr(), a.size as usize) }
}

pub fn loopback_listen_address(port: u32) -> SocketAddress {
    let mut a = SocketAddress { size: 0x1c, addr: [0; 0x1c] };
    a.addr[0] = 0x17;
    a.addr[2..4].copy_from_slice(&(port as u16).to_be_bytes());
    a.addr[8 + 10] = 0xff;
    a.addr[8 + 11] = 0xff;
    a.addr[8 + 12..8 + 16].copy_from_slice(&[127, 0, 0, 1]);
    a
}

unsafe fn log_connect_hex(name: BSTR, port: u32, hr: HRESULT) {
    log_error_args(b"Could not connect to %ws %d (0x%X)\0", &[name as usize, port as usize, hr as u32 as usize]);
}

pub unsafe fn tcp_client_connect(name: BSTR, port: u32, timeout: u32) -> Result<SOCKET, HRESULT> {
    let mut s = INVALID_SOCKET;
    let mut ev: HANDLE = null_mut();
    let res = 'run: {
        let addr = match crate::offline::classify_wide(name) {
            crate::offline::HostClass::RemoteName => break 'run Err(crate::offline::WSAHOST_NOT_FOUND_HR),
            crate::offline::HostClass::RemoteLiteral => {
                log_connect_hex(name, port, crate::offline::WSAEHOSTUNREACH_HR);
                break 'run Err(E_COULD_NOT_CONNECT);
            }
            _ => match resolve(name, port) {
                Ok(a) => a,
                Err(hr) => break 'run Err(hr),
            },
        };
        if !addr_is_local(&addr) {
            log_connect_hex(name, port, crate::offline::WSAEHOSTUNREACH_HR);
            break 'run Err(E_COULD_NOT_CONNECT);
        }
        s = match socket_connect(&addr) {
            Ok(v) => v,
            Err(hr) => break 'run Err(hr),
        };
        ev = CreateEventW(null_mut(), 1, 0, core::ptr::null());
        if ev.is_null() {
            break 'run Err(last_error_hr());
        }
        if WSAEventSelect(s, ev, FD_CONNECT) == -1 {
            let hr = map_wsa_error(WSAGetLastError());
            if hr < 0 {
                break 'run Err(hr);
            }
        }
        let w = WaitForSingleObject(ev, timeout);
        WSAEventSelect(s, ev, 0);
        if w != 0 {
            log_error_args(b"Could not connect to %ws %d (%d)\0", &[name as usize, port as usize, w as usize]);
            CloseHandle(ev);
            close_socket_full(s);
            return Err(E_COULD_NOT_CONNECT);
        }
        match socket_check_ready(s) {
            Ok(f) if f & 3 != 0 => {
                let out = s;
                s = INVALID_SOCKET;
                Ok(out)
            }
            Ok(_) => {
                log_connect_hex(name, port, S_OK);
                Err(E_COULD_NOT_CONNECT)
            }
            Err(hr) => {
                log_connect_hex(name, port, hr);
                Err(E_COULD_NOT_CONNECT)
            }
        }
    };
    let res = match res {
        Err(hr) if matches!(hr as u32, 0x8007_2746 | 0x8007_2af9 | 0x8007_2afa | 0x8007_2750 | 0x8007_2751) => {
            log_connect_hex(name, port, hr);
            Err(E_COULD_NOT_CONNECT)
        }
        r => r,
    };
    if !ev.is_null() {
        CloseHandle(ev);
    }
    close_socket_full(s);
    res
}

#[repr(C)]
pub struct ConnectionVtbl {
    pub dtor: unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void,
    pub connect: unsafe extern "C" fn(*mut c_void, u32, *mut *mut XbtpChannel) -> HRESULT,
    pub open_redirect: unsafe extern "C" fn(*mut c_void, *const u16, *mut *mut c_void) -> HRESULT,
    pub accept: unsafe extern "C" fn(*mut c_void, *mut *mut XbtpChannel) -> HRESULT,
    pub get_event: unsafe extern "C" fn(*mut c_void, *mut HANDLE) -> HRESULT,
}

#[repr(C)]
pub struct XbtpConnection {
    pub vtbl: *const ConnectionVtbl,
    pub shutdown_event: HANDLE,
}

pub unsafe fn connection_connect(c: *mut XbtpConnection, timeout: u32, out: *mut *mut XbtpChannel) -> HRESULT {
    ((*(*c).vtbl).connect)(c as *mut c_void, timeout, out)
}

pub unsafe fn connection_accept(c: *mut XbtpConnection, out: *mut *mut XbtpChannel) -> HRESULT {
    ((*(*c).vtbl).accept)(c as *mut c_void, out)
}

pub unsafe fn connection_open_redirect(c: *mut XbtpConnection, location: *const u16, out: *mut *mut XbtpConnection) -> HRESULT {
    ((*(*c).vtbl).open_redirect)(c as *mut c_void, location, out as *mut *mut c_void)
}

pub unsafe fn connection_event(c: *mut XbtpConnection, out: *mut HANDLE) -> HRESULT {
    ((*(*c).vtbl).get_event)(c as *mut c_void, out)
}

pub unsafe fn connection_delete(c: *mut XbtpConnection) {
    if !c.is_null() {
        ((*(*c).vtbl).dtor)(c as *mut c_void, 1);
    }
}

#[repr(C)]
pub struct TcpConnection {
    pub vtbl: *const ConnectionVtbl,
    pub shutdown_event: HANDLE,
    pub listen_socket: SOCKET,
    pub name_vtbl: usize,
    pub server_name: BSTR,
    pub port: u32,
    pub event_vtbl: usize,
    pub socket_event: HANDLE,
}

// Keep the second listener outside the console-visible TcpConnection layout.
static IPV6_LISTENERS: std::sync::Mutex<std::collections::BTreeMap<SOCKET, SOCKET>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

pub static TCP_CONNECTION_VTBL: ConnectionVtbl = ConnectionVtbl {
    dtor: tcp_connection_deleting_dtor,
    connect: tcp_connection_connect,
    open_redirect: tcp_connection_open_redirect,
    accept: tcp_connection_accept,
    get_event: tcp_connection_event,
};

unsafe fn tcp_connection_new() -> *mut TcpConnection {
    let c = cpp_new(core::mem::size_of::<TcpConnection>()) as *mut TcpConnection;
    if !c.is_null() {
        (*c).vtbl = &TCP_CONNECTION_VTBL;
        (*c).shutdown_event = INVALID_HANDLE_VALUE;
        (*c).listen_socket = INVALID_SOCKET;
    }
    c
}

#[export_name = "??1TcpConnection@@UEAA@XZ"]
pub unsafe extern "C" fn TcpConnection_dtor(this: *mut TcpConnection) {
    if (*this).listen_socket != INVALID_SOCKET {
        if let Some(ipv6) = IPV6_LISTENERS.lock().unwrap().remove(&(*this).listen_socket) {
            closesocket(ipv6);
        }
        closesocket((*this).listen_socket);
        (*this).listen_socket = INVALID_SOCKET;
    }
    if !(*this).socket_event.is_null() {
        CloseHandle((*this).socket_event);
        (*this).socket_event = null_mut();
    }
    if !(*this).server_name.is_null() {
        SysFreeString((*this).server_name);
    }
    (*this).server_name = null_mut();
}

pub(crate) unsafe extern "C" fn tcp_connection_deleting_dtor(this: *mut c_void, flags: u32) -> *mut c_void {
    TcpConnection_dtor(this as *mut TcpConnection);
    if flags & 1 != 0 {
        cpp_delete(this);
    }
    this
}

#[export_name = "?InitializeClient@TcpConnection@@SAJPEBGKPEAPEAV1@@Z"]
pub unsafe extern "C" fn TcpConnection_InitializeClient(name: *const u16, port: u32, out: *mut *mut TcpConnection) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    *out = null_mut();
    let c = tcp_connection_new();
    if c.is_null() {
        return E_OUTOFMEMORY;
    }
    if !name.is_null() {
        if *name != 0 {
            let b = SysAllocString(name);
            if b.is_null() {
                tcp_connection_deleting_dtor(c as *mut c_void, 1);
                return E_OUTOFMEMORY;
            }
            (*c).server_name = b;
        }
    }
    (*c).port = port;
    *out = c;
    S_OK
}

#[export_name = "?InitializeClient@TcpConnection@@SAJPEBGKPEAV?$shared_ptr@VTcpConnection@@@std@@@Z"]
pub unsafe extern "C" fn TcpConnection_InitializeClient_sp(name: *const u16, port: u32, out: *mut SharedPtr<TcpConnection>) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let mut raw: *mut TcpConnection = null_mut();
    let hr = TcpConnection_InitializeClient(name, port, &mut raw);
    wrap_into(raw, out);
    hr
}

pub(crate) unsafe fn wrap_into<T>(raw: *mut T, out: *mut SharedPtr<T>) {
    match SharedPtr::new_virtual(raw) {
        Some(sp) => sp.assign_into(out),
        None => {
            if !raw.is_null() {
                crate::msvc::virtual_delete(raw as *mut c_void);
            }
        }
    }
}

unsafe fn tcp_server_setup(c: *mut TcpConnection, port: u32) -> HRESULT {
    (*c).socket_event = CreateEventW(null_mut(), 1, 0, core::ptr::null());
    if (*c).socket_event.is_null() {
        return S_OK;
    }
    let s = match socket_listen(&loopback_listen_address(port)) {
        Ok(s) => s,
        Err(hr) => {
            log_error_args(b"Failed 0x%X to initialize the server socket\0", &[hr as u32 as usize]);
            return S_OK;
        }
    };
    (*c).listen_socket = s;
    WSAEventSelect(s, (*c).socket_event, FD_ACCEPT_CLOSE);

    // Use the assigned port even when the caller requested an ephemeral one.
    let mut addr = loopback_listen_address(port);
    let mut len = addr.size as i32;
    if getsockname(s, addr.addr.as_mut_ptr(), &mut len) == 0 {
        addr.addr[8..24].copy_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        if let Ok(ipv6) = socket_listen(&addr) {
            if WSAEventSelect(ipv6, (*c).socket_event, FD_ACCEPT_CLOSE) == 0 {
                IPV6_LISTENERS.lock().unwrap().insert(s, ipv6);
            } else {
                closesocket(ipv6);
            }
        }
    }
    S_OK
}

#[export_name = "?InitializeServer@TcpConnection@@SAJKPEAPEAV1@@Z"]
pub unsafe extern "C" fn TcpConnection_InitializeServer(port: u32, out: *mut *mut TcpConnection) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    *out = null_mut();
    let c = tcp_connection_new();
    if c.is_null() {
        return E_OUTOFMEMORY;
    }
    let hr = tcp_server_setup(c, port);
    if hr < 0 {
        log_error_args(b"Failed to initialize server socket\0", &[]);
        tcp_connection_deleting_dtor(c as *mut c_void, 1);
        return hr;
    }
    *out = c;
    hr
}

#[export_name = "?InitializeServer@TcpConnection@@SAJKPEAV?$shared_ptr@VTcpConnection@@@std@@@Z"]
pub unsafe extern "C" fn TcpConnection_InitializeServer_sp(port: u32, out: *mut SharedPtr<TcpConnection>) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let mut raw: *mut TcpConnection = null_mut();
    let hr = TcpConnection_InitializeServer(port, &mut raw);
    wrap_into(raw, out);
    hr
}

unsafe extern "C" fn tcp_connection_connect(this: *mut c_void, timeout: u32, out: *mut *mut XbtpChannel) -> HRESULT {
    let c = this as *mut TcpConnection;
    if out.is_null() {
        return E_INVALIDARG;
    }
    *out = null_mut();
    if (*c).listen_socket != INVALID_SOCKET {
        log_error_args(b"Cannot connect on a connection that was initialized as a server connection\0", &[]);
        return E_UNEXPECTED_SERVER_SOCKET;
    }
    let s = match tcp_client_connect((*c).server_name, (*c).port, timeout) {
        Ok(s) => s,
        Err(hr) => {
            log_error_args(b"Failed 0x%X to connect server %ws : %d\0", &[hr as u32 as usize, (*c).server_name as usize, (*c).port as usize]);
            return hr;
        }
    };
    let ch = new_tcp_channel();
    if ch.is_null() {
        closesocket(s);
        return E_OUTOFMEMORY;
    }
    let hr = tcp_channel_initialize(ch, s, true);
    if hr < 0 {
        closesocket(s);
        channel_delete(ch);
        return hr;
    }
    *out = ch;
    hr
}

unsafe extern "C" fn tcp_connection_accept(this: *mut c_void, out: *mut *mut XbtpChannel) -> HRESULT {
    let c = this as *mut TcpConnection;
    if out.is_null() {
        return E_INVALIDARG;
    }
    if (*c).listen_socket == INVALID_SOCKET {
        log_error_args(b"Cannot accept connections with client connection\0", &[]);
        return E_INVALIDARG;
    }
    *out = null_mut();
    let primary = (*c).listen_socket;
    let ipv6 = IPV6_LISTENERS.lock().unwrap().get(&primary).copied();
    // Clear once before polling both queues. Clearing between sockets could
    // discard a notification for the socket we just finished polling.
    ResetEvent((*c).socket_event);
    for socket in std::iter::once(primary).chain(ipv6) {
        let mut ne: NetworkEvents = core::mem::zeroed();
        WSAEnumNetworkEvents(socket, null_mut(), &mut ne);
        let cs = WSAAccept(socket, null_mut(), null_mut(), core::ptr::null(), 0);
        if cs == INVALID_SOCKET {
            let e = WSAGetLastError();
            if e == 0x2733 {
                continue;
            }
            log_error_args(b"Failed to accept connection %d\0", &[e as usize]);
            return map_wsa_error(e);
        }
        // Another queue may already have signalled before the reset.
        if ipv6.is_some() {
            SetEvent((*c).socket_event);
        }
        let ch = new_tcp_channel();
        if ch.is_null() {
            closesocket(cs);
            return E_OUTOFMEMORY;
        }
        let hr = tcp_channel_initialize(ch, cs, false);
        if hr < 0 {
            closesocket(cs);
            channel_delete(ch);
            return hr;
        }
        *out = ch;
        return hr;
    }
    E_PENDING
}

unsafe extern "C" fn tcp_connection_event(this: *mut c_void, out: *mut HANDLE) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    *out = (*(this as *mut TcpConnection)).socket_event;
    S_OK
}

unsafe extern "C" fn tcp_connection_open_redirect(_this: *mut c_void, location: *const u16, out: *mut *mut c_void) -> HRESULT {
    if location.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    *out = null_mut();
    let text = from_wide(location);
    let prefix_len = TCP_REDIRECT_PREFIX.encode_utf16().count();
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() < prefix_len || String::from_utf16_lossy(&units[..prefix_len]).to_ascii_lowercase() != TCP_REDIRECT_PREFIX {
        return E_INVALID_REDIRECT_FORMAT;
    }
    let rest = &units[prefix_len..];
    let colon = match rest.iter().rposition(|&u| u == ':' as u16) {
        Some(i) => i,
        None => return E_INVALID_REDIRECT_FORMAT,
    };
    let port = wtoi(&rest[colon + 1..]);
    if port == 0 || colon > 0x200 {
        return E_INVALID_REDIRECT_FORMAT;
    }
    let address = SysAllocStringLen(rest.as_ptr(), colon as u32);
    if address.is_null() && colon != 0 {
        return E_OUTOFMEMORY;
    }
    let mut conn: *mut TcpConnection = null_mut();
    let hr = TcpConnection_InitializeClient(address, port as u32, &mut conn);
    if hr >= 0 {
        *out = conn as *mut c_void;
    }
    if !address.is_null() {
        SysFreeString(address);
    }
    hr
}

fn wtoi(s: &[u16]) -> i32 {
    let text = String::from_utf16_lossy(s);
    let t = text.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: i64 = 0;
    for b in digits.bytes() {
        if !b.is_ascii_digit() {
            break;
        }
        v = (v * 10 + (b - b'0') as i64).min(i32::MAX as i64 + 1);
    }
    let v = if neg { -v } else { v };
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

pub unsafe fn tcp_redirect_location(port: u32, address: *const u16, len: *mut u32, out: *mut *mut u8) -> HRESULT {
    if address.is_null() || len.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let text = wide(&format!("{}{}:{}", TCP_REDIRECT_PREFIX, from_wide(address), port as i32));
    *len = 0;
    *out = null_mut();
    let n = WideCharToMultiByte(65001, 0, text.as_ptr(), -1, null_mut(), 0, core::ptr::null(), null_mut());
    if n <= 0 {
        let e = GetLastError();
        if e != 0 {
            return hr_from_win32(e);
        }
    }
    let buf = malloc(n.max(0) as usize) as *mut u8;
    if buf.is_null() {
        return E_OUTOFMEMORY;
    }
    let m = WideCharToMultiByte(65001, 0, text.as_ptr(), -1, buf, n, core::ptr::null(), null_mut());
    if m != n {
        free(buf as *mut c_void);
        return last_error_hr();
    }
    *len = n as u32;
    *out = buf;
    S_OK
}

#[repr(C)]
pub struct NpConnection {
    pub vtbl: *const ConnectionVtbl,
    pub shutdown_event: HANDLE,
    pub pipe: HANDLE,
    pub path_vtbl: usize,
    pub pipe_path: BSTR,
    pub ov: OVERLAPPED,
    pub event_vtbl: usize,
    pub connect_event: HANDLE,
}

pub static NP_CONNECTION_VTBL: ConnectionVtbl = ConnectionVtbl {
    dtor: np_connection_deleting_dtor,
    connect: np_connection_connect,
    open_redirect: np_connection_open_redirect,
    accept: np_connection_accept,
    get_event: np_connection_event,
};

unsafe fn valid_pipe_path(p: *const u16) -> bool {
    !p.is_null() && *p == b'\\' as u16 && *p.add(1) == b'\\' as u16
}

unsafe fn np_connection_new() -> *mut NpConnection {
    let c = cpp_new(core::mem::size_of::<NpConnection>()) as *mut NpConnection;
    if !c.is_null() {
        (*c).vtbl = &NP_CONNECTION_VTBL;
        (*c).shutdown_event = INVALID_HANDLE_VALUE;
        (*c).pipe = INVALID_HANDLE_VALUE;
    }
    c
}

#[export_name = "??1NpConnection@@UEAA@XZ"]
pub unsafe extern "C" fn NpConnection_dtor(this: *mut NpConnection) {
    if (*this).pipe != INVALID_HANDLE_VALUE {
        CancelIoEx((*this).pipe, &mut (*this).ov);
        CloseHandle((*this).pipe);
    }
    if !(*this).connect_event.is_null() {
        CloseHandle((*this).connect_event);
        (*this).connect_event = null_mut();
    }
    if !(*this).pipe_path.is_null() {
        SysFreeString((*this).pipe_path);
    }
    (*this).pipe_path = null_mut();
}

unsafe extern "C" fn np_connection_deleting_dtor(this: *mut c_void, flags: u32) -> *mut c_void {
    NpConnection_dtor(this as *mut NpConnection);
    if flags & 1 != 0 {
        cpp_delete(this);
    }
    this
}

#[export_name = "?InitializeClient@NpConnection@@SAJPEBGPEAPEAV1@@Z"]
pub unsafe extern "C" fn NpConnection_InitializeClient(path: *const u16, out: *mut *mut NpConnection) -> HRESULT {
    if !valid_pipe_path(path) || out.is_null() {
        return E_INVALIDARG;
    }
    *out = null_mut();
    let c = np_connection_new();
    if c.is_null() {
        return E_OUTOFMEMORY;
    }
    let b = SysAllocString(path);
    if b.is_null() {
        np_connection_deleting_dtor(c as *mut c_void, 1);
        return E_OUTOFMEMORY;
    }
    (*c).pipe_path = b;
    *out = c;
    S_OK
}

#[export_name = "?InitializeClient@NpConnection@@SAJPEBGPEAV?$shared_ptr@VNpConnection@@@std@@@Z"]
pub unsafe extern "C" fn NpConnection_InitializeClient_sp(path: *const u16, out: *mut SharedPtr<NpConnection>) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let mut raw: *mut NpConnection = null_mut();
    let hr = NpConnection_InitializeClient(path, &mut raw);
    if hr < 0 {
        return hr;
    }
    wrap_into(raw, out);
    hr
}

unsafe fn np_create_server_pipe(c: *mut NpConnection, path: *const u16) -> HRESULT {
    let h = CreateNamedPipeW(path, 0x4000_0003, 8, 0xff, 0x6000, 0x6000, 0, null_mut());
    if h == INVALID_HANDLE_VALUE {
        let e = GetLastError();
        log_error_args(b"Failed %d to create server pipe\0", &[e as usize]);
        if e != 0 {
            return hr_from_win32(e);
        }
    }
    ResetEvent((*c).connect_event);
    (*c).ov = OVERLAPPED { event: (*c).connect_event, ..Default::default() };
    let ok = ConnectNamedPipe(h, &mut (*c).ov);
    let e = GetLastError();
    let hr = if ok != 0 {
        log_error_args(b"Failed %d to setup for client connection\0", &[e as usize]);
        hr_from_win32(e)
    } else if e == 0x217 {
        SetEvent((*c).connect_event);
        S_OK
    } else if e == 0x3e5 {
        S_OK
    } else {
        hr_from_win32(e)
    };
    if hr < 0 {
        if h != INVALID_HANDLE_VALUE {
            CloseHandle(h);
        }
        return hr;
    }
    (*c).pipe = h;
    hr
}

unsafe fn np_server_setup(c: *mut NpConnection, path: *const u16) -> HRESULT {
    (*c).connect_event = CreateEventW(null_mut(), 1, 0, core::ptr::null());
    if (*c).connect_event.is_null() {
        let hr = last_error_hr();
        return if hr < 0 { hr } else { E_FAIL };
    }
    let hr = np_create_server_pipe(c, path);
    if hr < 0 {
        log_error_args(b"Failed 0x%X to initialize the server pipe\0", &[hr as u32 as usize]);
        return hr;
    }
    if !path.is_null() && *path != 0 {
        let b = SysAllocString(path);
        if !b.is_null() {
            if !(*c).pipe_path.is_null() {
                SysFreeString((*c).pipe_path);
            }
            (*c).pipe_path = b;
        }
    }
    hr
}

#[export_name = "?InitializeServer@NpConnection@@SAJPEBGPEAPEAV1@@Z"]
pub unsafe extern "C" fn NpConnection_InitializeServer(path: *const u16, out: *mut *mut NpConnection) -> HRESULT {
    if !valid_pipe_path(path) || out.is_null() {
        return E_INVALIDARG;
    }
    *out = null_mut();
    let c = np_connection_new();
    if c.is_null() {
        return E_OUTOFMEMORY;
    }
    let hr = np_server_setup(c, path);
    if hr < 0 {
        log_error_args(b"Failed to initialize server pipe\0", &[]);
        np_connection_deleting_dtor(c as *mut c_void, 1);
        return hr;
    }
    *out = c;
    hr
}

#[export_name = "?InitializeServer@NpConnection@@SAJPEBGPEAV?$shared_ptr@VNpConnection@@@std@@@Z"]
pub unsafe extern "C" fn NpConnection_InitializeServer_sp(path: *const u16, out: *mut SharedPtr<NpConnection>) -> HRESULT {
    if !valid_pipe_path(path) || out.is_null() {
        return E_INVALIDARG;
    }
    let mut raw: *mut NpConnection = null_mut();
    let hr = NpConnection_InitializeServer(path, &mut raw);
    wrap_into(raw, out);
    hr
}

pub unsafe fn np_client_open(path: *const u16, timeout: u32) -> Result<HANDLE, HRESULT> {
    let h = CreateFileW(path, 0xC000_0000, 0, null_mut(), 3, 0x4000_0000, null_mut());
    let mut hr = S_OK;
    if h == INVALID_HANDLE_VALUE {
        let e = GetLastError();
        if e == 0xe7 {
            if WaitNamedPipeW(path, timeout) == 0 {
                let e = GetLastError();
                log_error_args(b"Could not connect to pipe after wait %d\0", &[e as usize]);
                hr = hr_from_win32(e);
            }
        } else {
            hr = hr_from_win32(e);
        }
    }
    if hr >= 0 {
        let mut mode = 0u32;
        if SetNamedPipeHandleState(h, &mut mode, null_mut(), null_mut()) == 0 {
            let e = GetLastError();
            log_error_args(b"Failed to set pipe state %d\0", &[e as usize]);
            hr = hr_from_win32(e);
        }
    }
    if hr < 0 {
        log_error_args(b"Could not connect to %ws(0x%X)\0", &[path as usize, hr as u32 as usize]);
        if h != INVALID_HANDLE_VALUE {
            CloseHandle(h);
        }
        return Err(E_COULD_NOT_CONNECT);
    }
    Ok(h)
}

unsafe extern "C" fn np_connection_connect(this: *mut c_void, timeout: u32, out: *mut *mut XbtpChannel) -> HRESULT {
    let c = this as *mut NpConnection;
    if out.is_null() {
        return E_INVALIDARG;
    }
    *out = null_mut();
    if (*c).pipe != INVALID_HANDLE_VALUE {
        log_error_args(b"Cannot connect on a connection that was initialized as a server connection\0", &[]);
        return E_UNEXPECTED_SERVER_SOCKET;
    }
    let h = match np_client_open((*c).pipe_path, timeout) {
        Ok(h) => h,
        Err(hr) => {
            log_error_args(b"Failed 0x%X to connect server %ws\0", &[hr as u32 as usize, (*c).pipe_path as usize]);
            return hr;
        }
    };
    let ch = new_np_channel();
    if ch.is_null() {
        CloseHandle(h);
        return E_OUTOFMEMORY;
    }
    let hr = np_channel_initialize(ch, h, true);
    if hr < 0 {
        channel_delete(ch);
        CloseHandle(h);
        return hr;
    }
    *out = ch;
    hr
}

unsafe extern "C" fn np_connection_accept(this: *mut c_void, out: *mut *mut XbtpChannel) -> HRESULT {
    let c = this as *mut NpConnection;
    if out.is_null() {
        return E_INVALIDARG;
    }
    if (*c).pipe == INVALID_HANDLE_VALUE {
        log_error_args(b"Cannot accept connections with client connection\0", &[]);
        return E_INVALIDARG;
    }
    *out = null_mut();
    let w = if (*c).shutdown_event != INVALID_HANDLE_VALUE {
        let hs = [(*c).connect_event, (*c).shutdown_event];
        WaitForMultipleObjects(2, hs.as_ptr(), 0, INFINITE)
    } else {
        WaitForSingleObject((*c).connect_event, INFINITE)
    };
    if w == 1 {
        log_info_args(b"Accept stopped due to shutdown\0", &[]);
        return E_PENDING;
    }
    if w != 0 {
        let e = GetLastError();
        log_info_args(b"Accept was cancelled (%d)\0", &[e as usize]);
        let hr = hr_from_win32(e);
        if hr < 0 {
            return hr;
        }
    }
    let ch = new_np_channel();
    if ch.is_null() {
        return E_OUTOFMEMORY;
    }
    let hr = np_channel_initialize(ch, (*c).pipe, false);
    if hr < 0 {
        (*ch).handle = 0;
        channel_delete(ch);
        return hr;
    }
    *out = ch;
    (*c).pipe = INVALID_HANDLE_VALUE;
    np_create_server_pipe(c, (*c).pipe_path)
}

unsafe extern "C" fn np_connection_event(this: *mut c_void, out: *mut HANDLE) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    *out = (*(this as *mut NpConnection)).connect_event;
    S_OK
}

unsafe extern "C" fn np_connection_open_redirect(_this: *mut c_void, location: *const u16, out: *mut *mut c_void) -> HRESULT {
    if location.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    E_NOTIMPL
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    pub(crate) static NET_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn init() {
        unsafe {
            assert!(crate::xbtp2_sys::XbtpInitialize() >= 0);
        }
    }

    pub(crate) unsafe fn tcp_pair(port: u32) -> (*mut TcpConnection, *mut XbtpChannel, *mut XbtpChannel) {
        let mut server: *mut TcpConnection = null_mut();
        assert_eq!(TcpConnection_InitializeServer(port, &mut server), S_OK);
        assert_ne!((*server).listen_socket, INVALID_SOCKET);
        let name = wide("localhost");
        let mut client: *mut TcpConnection = null_mut();
        assert_eq!(TcpConnection_InitializeClient(name.as_ptr(), port, &mut client), S_OK);
        let mut cch: *mut XbtpChannel = null_mut();
        assert_eq!(connection_connect(client as *mut XbtpConnection, 5000, &mut cch), S_OK);
        let mut ev: HANDLE = null_mut();
        assert_eq!(connection_event(server as *mut XbtpConnection, &mut ev), S_OK);
        assert_eq!(WaitForSingleObject(ev, 5000), 0);
        let mut sch: *mut XbtpChannel = null_mut();
        let mut hr = E_PENDING;
        for _ in 0..50 {
            hr = connection_accept(server as *mut XbtpConnection, &mut sch);
            if hr != E_PENDING {
                break;
            }
            Sleep(20);
        }
        assert_eq!(hr, S_OK);
        connection_delete(client as *mut XbtpConnection);
        (server, cch, sch)
    }

    #[test]
    fn map_wsa_error_matches_console() {
        assert_eq!(map_wsa_error(0x2746), E_COULD_NOT_CONNECT);
        assert_eq!(map_wsa_error(0x40), E_COULD_NOT_CONNECT);
        assert_eq!(map_wsa_error(0x2733) as u32, 0x8007_2733);
    }

    #[test]
    fn tcp_initialize_client_raw_and_shared() {
        unsafe {
            assert_eq!(TcpConnection_InitializeClient(null_mut(), 1, null_mut()), E_INVALIDARG);
            let name = wide("host");
            let mut c: *mut TcpConnection = null_mut();
            assert_eq!(TcpConnection_InitializeClient(name.as_ptr(), 4242, &mut c), S_OK);
            assert_eq!(from_wide((*c).server_name), "host");
            assert_eq!((*c).port, 4242);
            assert_eq!((*c).listen_socket, INVALID_SOCKET);
            assert_eq!((*c).shutdown_event, INVALID_HANDLE_VALUE);
            let empty = wide("");
            let mut e: *mut TcpConnection = null_mut();
            assert_eq!(TcpConnection_InitializeClient(empty.as_ptr(), 1, &mut e), S_OK);
            assert!((*e).server_name.is_null());
            connection_delete(e as *mut XbtpConnection);
            TcpConnection_dtor(c);
            assert!((*c).server_name.is_null());
            cpp_delete(c as *mut c_void);
            let mut sp = SharedPtr::<TcpConnection>::default();
            assert_eq!(TcpConnection_InitializeClient_sp(name.as_ptr(), 7, &mut sp), S_OK);
            assert_eq!((*sp.ptr).port, 7);
            assert_eq!(sp.use_count(), 1);
            assert_eq!(TcpConnection_InitializeClient_sp(name.as_ptr(), 7, null_mut()), E_INVALIDARG);
        }
    }

    #[test]
    fn tcp_redirect_parses_console_format() {
        unsafe {
            let mut base: *mut TcpConnection = null_mut();
            assert_eq!(TcpConnection_InitializeClient(null_mut(), 1, &mut base), S_OK);
            let b = base as *mut XbtpConnection;
            let mut out: *mut XbtpConnection = null_mut();
            let loc = wide("XBTP::TCP:10.0.0.5:4211");
            assert_eq!(connection_open_redirect(b, loc.as_ptr(), &mut out), S_OK);
            let t = out as *mut TcpConnection;
            assert_eq!(from_wide((*t).server_name), "10.0.0.5");
            assert_eq!((*t).port, 4211);
            connection_delete(out);
            let bad = wide("xbtp::np:foo:1");
            assert_eq!(connection_open_redirect(b, bad.as_ptr(), &mut out), E_INVALID_REDIRECT_FORMAT);
            let noport = wide("xbtp::tcp:host:0");
            assert_eq!(connection_open_redirect(b, noport.as_ptr(), &mut out), E_INVALID_REDIRECT_FORMAT);
            assert_eq!(connection_open_redirect(b, null_mut(), &mut out), E_INVALIDARG);
            let mut len = 0u32;
            let mut text: *mut u8 = null_mut();
            let addr = wide("1.2.3.4");
            assert_eq!(tcp_redirect_location(99, addr.as_ptr(), &mut len, &mut text), S_OK);
            assert_eq!(core::slice::from_raw_parts(text, len as usize), b"xbtp::tcp:1.2.3.4:99\0");
            free(text as *mut c_void);
            connection_delete(b);
        }
    }

    #[test]
    fn tcp_server_accept_connect_and_transfer() {
        let _g = NET_LOCK.lock().unwrap();
        init();
        unsafe {
            let mut cnull: *mut XbtpChannel = null_mut();
            let mut server0: *mut TcpConnection = null_mut();
            assert_eq!(TcpConnection_InitializeServer(0, null_mut()), E_INVALIDARG);
            assert_eq!(TcpConnection_InitializeServer(47311, &mut server0), S_OK);
            assert_eq!(connection_connect(server0 as *mut XbtpConnection, 10, &mut cnull), E_UNEXPECTED_SERVER_SOCKET);
            assert_eq!(connection_accept(server0 as *mut XbtpConnection, &mut cnull), E_PENDING);
            connection_delete(server0 as *mut XbtpConnection);
            let (server, cch, sch) = tcp_pair(47312);
            let msg = *b"hello xbtp channel";
            let bufs = [WSABUF { len: 5, buf: msg.as_ptr() as *mut u8 }, WSABUF { len: 13, buf: msg.as_ptr().add(5) as *mut u8 }];
            assert_eq!(channel_send(sch, &bufs, None, null_mut()), S_OK);
            let mut got = [0u8; 18];
            assert_eq!(channel_receive(cch, 5000, 18, got.as_mut_ptr()), S_OK);
            assert_eq!(&got, &msg);
            assert_eq!(channel_receive(cch, 50, 4, got.as_mut_ptr()), E_PENDING);
            let back = *b"pong";
            let b2 = [WSABUF { len: 4, buf: back.as_ptr() as *mut u8 }];
            assert_eq!(channel_send(cch, &b2, None, null_mut()), S_OK);
            let mut g2 = [0u8; 4];
            assert_eq!(channel_receive(sch, 5000, 4, g2.as_mut_ptr()), S_OK);
            assert_eq!(&g2, b"pong");
            let mut addr: BSTR = null_mut();
            assert_eq!(((*(*sch).vtbl).get_address)(sch, &mut addr), S_OK);
            assert_eq!(from_wide(addr), "0000:0000:0000:0000:0000:FFFF:7F00:0001");
            SysFreeString(addr);
            assert_eq!(channel_next_exchange_id(cch), 1);
            assert_eq!(channel_next_exchange_id(sch), -1);
            channel_delete(sch);
            assert_eq!(channel_receive(cch, 5000, 1, g2.as_mut_ptr()), E_SOCKET_CLOSED);
            channel_delete(cch);
            let mut sp = SharedPtr::<TcpConnection>::default();
            connection_delete(server as *mut XbtpConnection);
            assert_eq!(TcpConnection_InitializeServer_sp(47313, &mut sp), S_OK);
            assert_ne!((*sp.ptr).listen_socket, INVALID_SOCKET);
            assert_eq!(TcpConnection_InitializeServer_sp(1, null_mut()), E_INVALIDARG);
        }
    }

    #[test]
    fn tcp_connect_refused_maps_to_could_not_connect() {
        let _g = NET_LOCK.lock().unwrap();
        init();
        unsafe {
            let name = wide("127.0.0.1");
            let mut c: *mut TcpConnection = null_mut();
            assert_eq!(TcpConnection_InitializeClient(name.as_ptr(), 1, &mut c), S_OK);
            let mut ch: *mut XbtpChannel = null_mut();
            assert_eq!(connection_connect(c as *mut XbtpConnection, 3000, &mut ch), E_COULD_NOT_CONNECT);
            assert!(ch.is_null());
            assert_eq!(connection_connect(c as *mut XbtpConnection, 3000, null_mut()), E_INVALIDARG);
            connection_delete(c as *mut XbtpConnection);
        }
    }

    pub(crate) unsafe fn np_pair(name: &str) -> (*mut NpConnection, *mut XbtpChannel, *mut XbtpChannel) {
        let path = wide(name);
        let mut server: *mut NpConnection = null_mut();
        assert_eq!(NpConnection_InitializeServer(path.as_ptr(), &mut server), S_OK);
        let mut client: *mut NpConnection = null_mut();
        assert_eq!(NpConnection_InitializeClient(path.as_ptr(), &mut client), S_OK);
        let mut cch: *mut XbtpChannel = null_mut();
        assert_eq!(connection_connect(client as *mut XbtpConnection, 2000, &mut cch), S_OK);
        let mut sch: *mut XbtpChannel = null_mut();
        assert_eq!(connection_accept(server as *mut XbtpConnection, &mut sch), S_OK);
        assert_ne!((*server).pipe, INVALID_HANDLE_VALUE);
        connection_delete(client as *mut XbtpConnection);
        (server, cch, sch)
    }

    #[test]
    fn np_connection_lifecycle_and_transfer() {
        init();
        unsafe {
            let bad = wide("pipe");
            let mut c: *mut NpConnection = null_mut();
            assert_eq!(NpConnection_InitializeClient(bad.as_ptr(), &mut c), E_INVALIDARG);
            assert_eq!(NpConnection_InitializeServer(bad.as_ptr(), &mut c), E_INVALIDARG);
            let mut sp = SharedPtr::<NpConnection>::default();
            assert_eq!(NpConnection_InitializeClient_sp(bad.as_ptr(), &mut sp), E_INVALIDARG);
            assert_eq!(NpConnection_InitializeServer_sp(bad.as_ptr(), &mut sp), E_INVALIDARG);
            let name = format!(r"\\.\pipe\xbtp_conn_test_{}", GetCurrentProcessId());
            let (server, cch, sch) = np_pair(&name);
            let mut out: *mut XbtpConnection = null_mut();
            let loc = wide("x");
            assert_eq!(connection_open_redirect(server as *mut XbtpConnection, loc.as_ptr(), &mut out), E_NOTIMPL);
            assert_eq!(connection_open_redirect(server as *mut XbtpConnection, null_mut(), &mut out), E_INVALIDARG);
            let mut chx: *mut XbtpChannel = null_mut();
            assert_eq!(connection_connect(server as *mut XbtpConnection, 1, &mut chx), E_UNEXPECTED_SERVER_SOCKET);
            let msg = *b"pipe message body";
            let bufs = [WSABUF { len: msg.len() as u32, buf: msg.as_ptr() as *mut u8 }];
            assert_eq!(channel_send(cch, &bufs, None, null_mut()), S_OK);
            let mut got = [0u8; 17];
            assert_eq!(channel_receive(sch, 5000, 17, got.as_mut_ptr()), S_OK);
            assert_eq!(&got, &msg);
            assert_eq!(channel_receive(sch, 30, 1, got.as_mut_ptr()), E_PENDING);
            let mut addr: BSTR = null_mut();
            assert_eq!(((*(*sch).vtbl).get_address)(sch, &mut addr), E_NOTIMPL);
            assert!(!((*(*sch).vtbl).is_network)(sch));
            channel_delete(cch);
            assert_eq!(channel_receive(sch, 5000, 1, got.as_mut_ptr()), 0x8007_006du32 as i32);
            channel_delete(sch);
            NpConnection_dtor(server);
            assert!((*server).pipe_path.is_null());
            cpp_delete(server as *mut c_void);
            let mut ssp = SharedPtr::<NpConnection>::default();
            let path = wide(&name);
            assert_eq!(NpConnection_InitializeServer_sp(path.as_ptr(), &mut ssp), S_OK);
            let mut csp = SharedPtr::<NpConnection>::default();
            assert_eq!(NpConnection_InitializeClient_sp(path.as_ptr(), &mut csp), S_OK);
            assert_eq!(from_wide((*csp.ptr).pipe_path), name);
        }
    }

    #[test]
    fn np_connect_without_server_fails() {
        unsafe {
            let path = wide(r"\\.\pipe\xbtp_no_such_pipe_for_test");
            let mut c: *mut NpConnection = null_mut();
            assert_eq!(NpConnection_InitializeClient(path.as_ptr(), &mut c), S_OK);
            let mut ch: *mut XbtpChannel = null_mut();
            assert_eq!(connection_connect(c as *mut XbtpConnection, 10, &mut ch), E_COULD_NOT_CONNECT);
            connection_delete(c as *mut XbtpConnection);
        }
    }
}
