//! In-process network policy for the title.
//!
//! Project rule: the title must never talk to Fortnite/Epic production services. The title
//! and the SDKs it links use WinHTTP and Winsock directly, so the policy is enforced inside
//! the title process, at the API entry points, before anything leaves the machine.
//!
//! Two modes:
//!
//!   * **deny (default)** — every name resolution, outbound connect/sendto and WinHTTP
//!     connection fails locally.
//!   * **redirect** — `TOMBOX_NET_REDIRECT=<host>` (the project owner's own server). Every
//!     host name the title asks for is replaced by `<host>`:
//!       - `getaddrinfo` / `GetAddrInfoW` / `GetAddrInfoExW/A` / `gethostbyname` /
//!         `WSAConnectByNameW/A` resolve `<host>` instead of the requested name;
//!       - `WinHttpConnect` connects to `<host>` (same port), so TLS validates against
//!         `<host>`'s certificate and the Host header names `<host>`;
//!       - `connect` / `WSAConnect` / `sendto` / `WSASendTo` are allowed only towards
//!         loopback or an address `<host>` resolves to; any other destination is refused.
//!     Null/empty names, `localhost` and this machine's own name are passed through.
//!
//! Mechanism: each entry point is overwritten with `mov rax, imm64; jmp rax` to a hook. In
//! redirect mode the displaced instructions are relocated into a trampoline (iced-x86
//! block encoder) so the hook can call the original.
//!
//! Limits (documented, not hidden): a socket connected through `ConnectEx` to a literal IP
//! address is not covered; DNS-over-anything not using these APIs is not covered; other
//! processes are unaffected. Only one compat DLL per process installs the policy.
//! `TOMBOX_ALLOW_NETWORK=1` disables the policy entirely (never use with this title).

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

mod ext;

extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
    fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old_protect: *mut u32) -> i32;
    fn VirtualAlloc(addr: *mut c_void, size: usize, ty: u32, protect: u32) -> *mut c_void;
    fn FlushInstructionCache(process: *mut c_void, addr: *const c_void, size: usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
    fn SetLastError(code: u32);
    fn GetLastError() -> u32;
    fn GetComputerNameExA(kind: u32, buf: *mut u8, size: *mut u32) -> i32;
    fn GetTickCount64() -> u64;
}

const WSAENETUNREACH: u32 = 10051;
const WSAHOST_NOT_FOUND: i32 = 11001;
const ERROR_WINHTTP_CANNOT_CONNECT: u32 = 12029;

static DENIED: AtomicUsize = AtomicUsize::new(0);
static REDIRECTED: AtomicUsize = AtomicUsize::new(0);

fn note(what: &str) {
    let n = DENIED.fetch_add(1, Ordering::Relaxed);
    if n < 40 || n % 500 == 0 {
        crate::record_raw("net_guard", "denied", &format!("{what} (total blocked calls: {})", n + 1));
    }
}

fn note_redirect(api: &str, original: &str) {
    let n = REDIRECTED.fetch_add(1, Ordering::Relaxed);
    if n < 80 || n % 500 == 0 {
        crate::record_raw("net_guard", "redirect", &format!(
            "{api}: '{original}' -> '{}' (total redirected: {})", target().map(|t| t.name.as_str()).unwrap_or("?"), n + 1));
    }
}

// ---- deny stubs (argument-agnostic: the x64 caller cleans up) --------------------------------

unsafe extern "system" fn deny_sock() -> i32 {
    note("socket connect/send");
    SetLastError(WSAENETUNREACH); // WSAGetLastError reads the thread's last-error value
    -1
}
unsafe extern "system" fn deny_sock_bool() -> i32 {
    note("WSAConnectByName/List");
    SetLastError(WSAENETUNREACH);
    0
}
unsafe extern "system" fn deny_resolve() -> i32 {
    note("name resolution");
    SetLastError(WSAHOST_NOT_FOUND as u32);
    WSAHOST_NOT_FOUND
}
unsafe extern "system" fn deny_null() -> usize {
    note("WinHTTP connect / gethostbyname");
    SetLastError(ERROR_WINHTTP_CANNOT_CONNECT);
    0
}
unsafe extern "system" fn deny_false() -> i32 {
    note("WinHttpSendRequest");
    SetLastError(ERROR_WINHTTP_CANNOT_CONNECT);
    0
}

// ---- patching ---------------------------------------------------------------------------------

pub(crate) unsafe fn resolve_export(module: &[u8], func: &[u8]) -> *mut u8 {
    let mut h = GetModuleHandleA(module.as_ptr());
    if h.is_null() {
        h = LoadLibraryA(module.as_ptr());
    }
    if h.is_null() {
        return core::ptr::null_mut();
    }
    GetProcAddress(h, func.as_ptr()) as *mut u8
}

pub(crate) unsafe fn write_jump(p: *mut u8, stub: usize) -> bool {
    let mut code = [0u8; 12]; // mov rax, imm64 ; jmp rax
    code[0] = 0x48;
    code[1] = 0xB8;
    code[2..10].copy_from_slice(&(stub as u64).to_le_bytes());
    code[10] = 0xFF;
    code[11] = 0xE0;
    let mut old = 0u32;
    if VirtualProtect(p as *mut c_void, code.len(), 0x40, &mut old) == 0 {
        return false;
    }
    core::ptr::copy_nonoverlapping(code.as_ptr(), p, code.len());
    let mut tmp = 0u32;
    VirtualProtect(p as *mut c_void, code.len(), old, &mut tmp);
    FlushInstructionCache(GetCurrentProcess(), p as *const c_void, code.len());
    true
}

/// Executable page within +-1 GiB of `near` (so rip-relative operands can be re-encoded).
unsafe fn alloc_near(near: usize) -> *mut u8 {
    let base = near & !0xFFFF;
    for i in 1..0x3F00usize {
        for cand in [base.wrapping_sub(i << 16), base.wrapping_add(i << 16)] {
            let p = VirtualAlloc(cand as *mut c_void, 0x1000, 0x3000, 0x40);
            if !p.is_null() {
                return p as *mut u8;
            }
        }
    }
    core::ptr::null_mut()
}

/// Relocate the first >= 12 bytes of `p` into a trampoline that continues in the original.
unsafe fn make_trampoline(p: *mut u8) -> Option<usize> {
    use iced_x86::{BlockEncoder, BlockEncoderOptions, Decoder, DecoderOptions, FlowControl, InstructionBlock};
    let bytes = core::slice::from_raw_parts(p, 32);
    let mut dec = Decoder::with_ip(64, bytes, p as u64, DecoderOptions::NONE);
    let mut insns = Vec::new();
    let mut len = 0usize;
    while len < 12 {
        let i = dec.decode();
        if i.is_invalid() {
            return None;
        }
        // A return or indirect branch inside the displaced bytes means the function is too
        // short / oddly shaped to hook safely.
        if matches!(i.flow_control(), FlowControl::Return | FlowControl::IndirectBranch | FlowControl::Interrupt | FlowControl::Exception) {
            return None;
        }
        len += i.len();
        insns.push(i);
    }
    let tramp = alloc_near(p as usize);
    if tramp.is_null() {
        return None;
    }
    let block = InstructionBlock::new(&insns, tramp as u64);
    let enc = BlockEncoder::encode(64, block, BlockEncoderOptions::NONE).ok()?;
    let code = enc.code_buffer;
    if code.len() + 14 > 0x1000 {
        return None;
    }
    core::ptr::copy_nonoverlapping(code.as_ptr(), tramp, code.len());
    // jmp qword ptr [rip+0] ; dq original+len
    let tail = tramp.add(code.len());
    let j: [u8; 6] = [0xFF, 0x25, 0, 0, 0, 0];
    core::ptr::copy_nonoverlapping(j.as_ptr(), tail, 6);
    core::ptr::write_unaligned(tail.add(6) as *mut u64, p as u64 + len as u64);
    FlushInstructionCache(GetCurrentProcess(), tramp as *const c_void, 0x1000);
    Some(tramp as usize)
}

// ---- redirect mode ------------------------------------------------------------------------------

struct Target {
    name: String,
    ansi: Vec<u8>,  // NUL-terminated
    wide: Vec<u16>, // NUL-terminated
}
static TARGET: OnceLock<Option<Target>> = OnceLock::new();

fn target() -> Option<&'static Target> {
    TARGET.get().and_then(|t| t.as_ref())
}

/// Trampolines to the original functions, indexed by `H_*`.
const H_GETADDRINFO: usize = 0;
const H_GETADDRINFOW: usize = 1;
const H_GETADDRINFOEXW: usize = 2;
const H_GETADDRINFOEXA: usize = 3;
const H_GETHOSTBYNAME: usize = 4;
const H_CONNECT: usize = 5;
const H_WSACONNECT: usize = 6;
const H_CONNECTBYNAMEW: usize = 7;
const H_CONNECTBYNAMEA: usize = 8;
const H_SENDTO: usize = 9;
const H_WSASENDTO: usize = 10;
const H_WINHTTPCONNECT: usize = 11;
const H_WINHTTPOPENREQUEST: usize = 12;
const H_WINHTTPOPEN: usize = 13;
const H_WINHTTPSETOPTION: usize = 14;
const H_WINHTTPQUERYOPTION: usize = 15;
const H_WINHTTPSENDREQUEST: usize = 16;
const H_WINHTTPSETCALLBACK: usize = 17;
const H_WINHTTPRECEIVE: usize = 18;
const H_WINHTTPQUERYAVAIL: usize = 19;
const H_WINHTTPREADDATA: usize = 20;
const H_WINHTTPQUERYHEADERS: usize = 21;
const H_WINHTTPWSUPGRADE: usize = 22;
static ORIG: [AtomicUsize; 23] = [const { AtomicUsize::new(0) }; 23];
static GAME_CB: Mutex<Vec<(usize, usize, u32)>> = Mutex::new(Vec::new());
static PARENT: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

fn forget_handle(h: usize) {
    if let Ok(mut g) = GAME_CB.lock() {
        g.retain(|(k, _, _)| *k != h);
    }
    if let Ok(mut g) = PARENT.lock() {
        g.retain(|(c, _)| *c != h);
    }
}

fn set_parent(child: usize, parent: usize) {
    if child == 0 { return; }
    if let Ok(mut g) = GAME_CB.lock() {
        g.retain(|(k, _, _)| *k != child);
    }
    if let Ok(mut g) = PARENT.lock() {
        g.retain(|(c, _)| *c != child);
        g.push((child, parent));
        if g.len() > 8192 { g.remove(0); }
    }
    if let Some((cb, fl)) = find_cb(parent) {
        if let Ok(mut g) = GAME_CB.lock() {
            g.push((child, cb, fl));
            if g.len() > 4096 { g.remove(0); }
        }
    }
}

thread_local! {
    static CREATING_UNDER: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

fn adopt_creating(h: usize) -> bool {
    let parent = CREATING_UNDER.with(|c| c.get());
    if parent == 0 || parent == h {
        return false;
    }
    set_parent(h, parent);
    true
}

fn find_cb(h: usize) -> Option<(usize, u32)> {
    let cbs = GAME_CB.lock().ok()?;
    let par = PARENT.lock().ok()?;
    let mut cur = h;
    for _ in 0..4 {
        if let Some(&(_, c, f)) = cbs.iter().rev().find(|(k, _, _)| *k == cur) { return Some((c, f)); }
        match par.iter().rev().find(|(c, _)| *c == cur) { Some(&(_, p)) => cur = p, None => return None }
    }
    None
}
static TRACE_N: AtomicUsize = AtomicUsize::new(0);

fn trace(msg: &str) {
    if TRACE_N.fetch_add(1, Ordering::Relaxed) < 4000 {
        crate::record_raw("net_guard", "winhttp", msg);
    }
}
static LOCAL_PORT: AtomicUsize = AtomicUsize::new(0);
static BUILTIN_PORT: AtomicUsize = AtomicUsize::new(0);

macro_rules! orig {
    ($idx:expr, fn($($at:ty),*) -> $rt:ty) => {{
        let f: unsafe extern "system" fn($($at),*) -> $rt = core::mem::transmute(ORIG[$idx].load(Ordering::Relaxed));
        f
    }};
}

unsafe fn cstr(p: *const u8) -> String {
    let mut n = 0;
    while n < 1024 && *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf8_lossy(core::slice::from_raw_parts(p, n)).into_owned()
}
unsafe fn wstr(p: *const u16) -> String {
    let mut n = 0;
    while n < 1024 && *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(core::slice::from_raw_parts(p, n))
}

fn local_names() -> &'static Vec<String> {
    static N: OnceLock<Vec<String>> = OnceLock::new();
    N.get_or_init(|| {
        let mut v = Vec::new();
        for kind in [0u32, 1, 3] { // NetBIOS, DnsHostname, DnsFullyQualified
            let mut buf = [0u8; 256];
            let mut n = buf.len() as u32;
            if unsafe { GetComputerNameExA(kind, buf.as_mut_ptr(), &mut n) } != 0 && n > 0 {
                v.push(String::from_utf8_lossy(&buf[..n as usize]).to_ascii_lowercase());
            }
        }
        v
    })
}

/// Should this requested name be left alone (not a remote host)?
fn passthrough(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    n.is_empty() || n == "localhost" || n == "127.0.0.1" || n == "::1" || n == "0.0.0.0" || n == "::"
        || local_names().iter().any(|l| *l == n)
        || target().map_or(false, |t| t.name.eq_ignore_ascii_case(&n))
}

// Allowed destination addresses (IPv4 stored v4-mapped).
static ALLOWED: Mutex<Vec<[u8; 16]>> = Mutex::new(Vec::new());
static LAST_REFRESH: AtomicUsize = AtomicUsize::new(0);

fn v4(a: [u8; 4]) -> [u8; 16] {
    let mut o = [0u8; 16];
    o[10] = 0xff;
    o[11] = 0xff;
    o[12..].copy_from_slice(&a);
    o
}

unsafe fn sockaddr_ip(sa: *const u8, len: i32) -> Option<[u8; 16]> {
    if sa.is_null() || len < 8 {
        return None;
    }
    match core::ptr::read_unaligned(sa as *const u16) {
        2 if len >= 8 => Some(v4([*sa.add(4), *sa.add(5), *sa.add(6), *sa.add(7)])),
        23 if len >= 24 => {
            let mut o = [0u8; 16];
            core::ptr::copy_nonoverlapping(sa.add(8), o.as_mut_ptr(), 16);
            Some(o)
        }
        _ => None,
    }
}

static LAN_PEERS: Mutex<Vec<[u8; 16]>> = Mutex::new(Vec::new());
static LAN_DISCOVERY_PORT: AtomicUsize = AtomicUsize::new(0);

pub fn is_private_v4(a: [u8; 4]) -> bool {
    a[0] == 10 || a[0] == 127 || (a[0] == 172 && (16..=31).contains(&a[1])) || (a[0] == 192 && a[1] == 168) || (a[0] == 169 && a[1] == 254)
}

pub fn allow_lan_peer(a: [u8; 4]) -> bool {
    if !is_private_v4(a) {
        crate::record_raw("net_guard", "lan", &format!("refused LAN peer {}.{}.{}.{}: not a private, link-local or loopback address", a[0], a[1], a[2], a[3]));
        return false;
    }
    let ip = v4(a);
    let mut g = LAN_PEERS.lock().unwrap_or_else(|e| e.into_inner());
    if !g.contains(&ip) {
        g.push(ip);
        crate::record_raw("net_guard", "lan", &format!("LAN peer {}.{}.{}.{} allowed", a[0], a[1], a[2], a[3]));
    }
    true
}

pub fn allow_lan_broadcast(port: u16) {
    LAN_DISCOVERY_PORT.store(port as usize, Ordering::Relaxed);
}

pub fn lan_dest_ok(ip: &[u8; 16], port: u16) -> bool {
    if LAN_PEERS.lock().unwrap_or_else(|e| e.into_inner()).contains(ip) {
        return true;
    }
    let d = LAN_DISCOVERY_PORT.load(Ordering::Relaxed);
    if d == 0 || port as usize != d || ip[..10] != [0u8; 10] || ip[10] != 0xff || ip[11] != 0xff {
        return false;
    }
    let a = [ip[12], ip[13], ip[14], ip[15]];
    a == [255; 4] || (is_private_v4(a) && a[3] == 255)
}

unsafe fn sockaddr_port(sa: *const u8, len: i32) -> Option<u16> {
    if sa.is_null() || len < 4 {
        return None;
    }
    Some(u16::from_be_bytes([*sa.add(2), *sa.add(3)]))
}

fn is_loopback(ip: &[u8; 16]) -> bool {
    let mut l6 = [0u8; 16];
    l6[15] = 1;
    *ip == l6 || (ip[..10] == [0u8; 10] && ip[10] == 0xff && ip[11] == 0xff && ip[12] == 127)
}

/// Resolve the redirect host with the original getaddrinfo and remember its addresses.
unsafe fn refresh_allowed() {
    let Some(t) = target() else { return };
    let f = orig!(H_GETADDRINFO, fn(*const u8, *const u8, *const c_void, *mut *mut u8) -> i32);
    // Winsock may not be initialised yet when the policy is installed.
    static WSA: std::sync::Once = std::sync::Once::new();
    WSA.call_once(|| {
        let p = resolve_export(b"ws2_32.dll\0", b"WSAStartup\0");
        if !p.is_null() {
            let startup: unsafe extern "system" fn(u16, *mut u8) -> i32 = core::mem::transmute(p);
            let mut data = [0u8; 512];
            startup(0x0202, data.as_mut_ptr());
        }
    });
    let mut res: *mut u8 = core::ptr::null_mut();
    let rc = f(t.ansi.as_ptr(), core::ptr::null(), core::ptr::null(), &mut res);
    LAST_REFRESH.store(GetTickCount64() as usize, Ordering::Relaxed);
    if rc != 0 || res.is_null() {
        crate::record_raw("net_guard", "resolve", &format!("'{}' did not resolve (error {rc}) - outbound sockets stay refused", t.name));
        return;
    }
    let mut found = Vec::new();
    let mut p = res;
    while !p.is_null() {
        // addrinfo: ai_addrlen @16 (size_t), ai_addr @32, ai_next @40
        let alen = core::ptr::read_unaligned(p.add(16) as *const usize) as i32;
        let addr = core::ptr::read_unaligned(p.add(32) as *const *const u8);
        if let Some(ip) = sockaddr_ip(addr, alen) {
            if !found.contains(&ip) {
                found.push(ip);
            }
        }
        p = core::ptr::read_unaligned(p.add(40) as *const *mut u8);
    }
    let free = resolve_export(b"ws2_32.dll\0", b"freeaddrinfo\0");
    if !free.is_null() {
        let free: unsafe extern "system" fn(*mut u8) = core::mem::transmute(free);
        free(res);
    }
    let mut g = ALLOWED.lock().unwrap();
    let before = g.len();
    for ip in found {
        if !g.contains(&ip) {
            g.push(ip);
        }
    }
    if g.len() != before {
        crate::record_raw("net_guard", "resolve", &format!("'{}' -> {} address(es) allowed for outbound sockets", t.name, g.len()));
    }
}

/// May the title send to this socket address?
unsafe fn dest_allowed(sa: *const u8, len: i32) -> bool {
    if sa.is_null() {
        return true; // connected socket / no explicit destination
    }
    let Some(ip) = sockaddr_ip(sa, len) else {
        return false; // unknown address family
    };
    if is_loopback(&ip) || ALLOWED.lock().unwrap().contains(&ip) {
        return true;
    }
    if sockaddr_port(sa, len).map_or(false, |p| lan_dest_ok(&ip, p)) {
        return true;
    }
    // The redirect host's addresses may have changed: refresh at most every 10 s.
    let now = GetTickCount64() as usize;
    if now.wrapping_sub(LAST_REFRESH.load(Ordering::Relaxed)) > 10_000 {
        refresh_allowed();
        return ALLOWED.lock().unwrap().contains(&ip);
    }
    false
}

fn deny_dest(api: &str, sa: *const u8, len: i32) {
    let ip = unsafe { sockaddr_ip(sa, len) };
    let s = match ip {
        Some(ip) if ip[..10] == [0u8; 10] && ip[10] == 0xff => format!("{}.{}.{}.{}", ip[12], ip[13], ip[14], ip[15]),
        Some(ip) => ip.chunks(2).map(|c| format!("{:02x}{:02x}", c[0], c[1])).collect::<Vec<_>>().join(":"),
        None => "unknown-family".into(),
    };
    note(&format!("{api} to {s} (not the redirect host)"));
    unsafe { SetLastError(WSAENETUNREACH) };
}

unsafe extern "system" fn h_getaddrinfo(node: *const u8, service: *const u8, hints: *const c_void, res: *mut *mut u8) -> i32 {
    let f = orig!(H_GETADDRINFO, fn(*const u8, *const u8, *const c_void, *mut *mut u8) -> i32);
    if node.is_null() || passthrough(&cstr(node)) {
        return f(node, service, hints, res);
    }
    note_redirect("getaddrinfo", &cstr(node));
    f(target().unwrap().ansi.as_ptr(), service, hints, res)
}

unsafe extern "system" fn h_getaddrinfow(node: *const u16, service: *const u16, hints: *const c_void, res: *mut *mut u8) -> i32 {
    let f = orig!(H_GETADDRINFOW, fn(*const u16, *const u16, *const c_void, *mut *mut u8) -> i32);
    if node.is_null() || passthrough(&wstr(node)) {
        return f(node, service, hints, res);
    }
    note_redirect("GetAddrInfoW", &wstr(node));
    f(target().unwrap().wide.as_ptr(), service, hints, res)
}

type Ex<T> = unsafe extern "system" fn(*const T, *const T, u32, *const c_void, *const c_void, *mut *mut u8, *const c_void, *mut c_void, *const c_void, *mut *mut c_void) -> i32;

unsafe extern "system" fn h_getaddrinfoexw(
    name: *const u16, service: *const u16, ns: u32, nsp: *const c_void, hints: *const c_void, res: *mut *mut u8,
    timeout: *const c_void, ov: *mut c_void, routine: *const c_void, handle: *mut *mut c_void,
) -> i32 {
    let f: Ex<u16> = core::mem::transmute(ORIG[H_GETADDRINFOEXW].load(Ordering::Relaxed));
    if name.is_null() || passthrough(&wstr(name)) {
        return f(name, service, ns, nsp, hints, res, timeout, ov, routine, handle);
    }
    note_redirect("GetAddrInfoExW", &wstr(name));
    f(target().unwrap().wide.as_ptr(), service, ns, nsp, hints, res, timeout, ov, routine, handle)
}

unsafe extern "system" fn h_getaddrinfoexa(
    name: *const u8, service: *const u8, ns: u32, nsp: *const c_void, hints: *const c_void, res: *mut *mut u8,
    timeout: *const c_void, ov: *mut c_void, routine: *const c_void, handle: *mut *mut c_void,
) -> i32 {
    let f: Ex<u8> = core::mem::transmute(ORIG[H_GETADDRINFOEXA].load(Ordering::Relaxed));
    if name.is_null() || passthrough(&cstr(name)) {
        return f(name, service, ns, nsp, hints, res, timeout, ov, routine, handle);
    }
    note_redirect("GetAddrInfoExA", &cstr(name));
    f(target().unwrap().ansi.as_ptr(), service, ns, nsp, hints, res, timeout, ov, routine, handle)
}

unsafe extern "system" fn h_gethostbyname(name: *const u8) -> *mut c_void {
    let f = orig!(H_GETHOSTBYNAME, fn(*const u8) -> *mut c_void);
    if name.is_null() || passthrough(&cstr(name)) {
        return f(name);
    }
    note_redirect("gethostbyname", &cstr(name));
    f(target().unwrap().ansi.as_ptr())
}

unsafe extern "system" fn h_connect(s: usize, name: *const u8, len: i32) -> i32 {
    if !dest_allowed(name, len) {
        deny_dest("connect", name, len);
        return -1;
    }
    orig!(H_CONNECT, fn(usize, *const u8, i32) -> i32)(s, name, len)
}

unsafe extern "system" fn h_wsaconnect(s: usize, name: *const u8, len: i32, a: *mut c_void, b: *mut c_void, c: *mut c_void, d: *mut c_void) -> i32 {
    if !dest_allowed(name, len) {
        deny_dest("WSAConnect", name, len);
        return -1;
    }
    orig!(H_WSACONNECT, fn(usize, *const u8, i32, *mut c_void, *mut c_void, *mut c_void, *mut c_void) -> i32)(s, name, len, a, b, c, d)
}

type ByName<T> = unsafe extern "system" fn(usize, *const T, *const T, *mut u32, *mut c_void, *mut u32, *mut c_void, *const c_void, *mut c_void) -> i32;

unsafe extern "system" fn h_connectbynamew(
    s: usize, node: *const u16, service: *const u16, ll: *mut u32, la: *mut c_void, rl: *mut u32, ra: *mut c_void, timeout: *const c_void, reserved: *mut c_void,
) -> i32 {
    let f: ByName<u16> = core::mem::transmute(ORIG[H_CONNECTBYNAMEW].load(Ordering::Relaxed));
    if node.is_null() || passthrough(&wstr(node)) {
        return f(s, node, service, ll, la, rl, ra, timeout, reserved);
    }
    note_redirect("WSAConnectByNameW", &wstr(node));
    f(s, target().unwrap().wide.as_ptr(), service, ll, la, rl, ra, timeout, reserved)
}

unsafe extern "system" fn h_connectbynamea(
    s: usize, node: *const u8, service: *const u8, ll: *mut u32, la: *mut c_void, rl: *mut u32, ra: *mut c_void, timeout: *const c_void, reserved: *mut c_void,
) -> i32 {
    let f: ByName<u8> = core::mem::transmute(ORIG[H_CONNECTBYNAMEA].load(Ordering::Relaxed));
    if node.is_null() || passthrough(&cstr(node)) {
        return f(s, node, service, ll, la, rl, ra, timeout, reserved);
    }
    note_redirect("WSAConnectByNameA", &cstr(node));
    f(s, target().unwrap().ansi.as_ptr(), service, ll, la, rl, ra, timeout, reserved)
}

unsafe extern "system" fn h_sendto(s: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, tolen: i32) -> i32 {
    if !dest_allowed(to, tolen) {
        deny_dest("sendto", to, tolen);
        return -1;
    }
    orig!(H_SENDTO, fn(usize, *const u8, i32, i32, *const u8, i32) -> i32)(s, buf, len, flags, to, tolen)
}

unsafe extern "system" fn h_wsasendto(
    s: usize, bufs: *const c_void, count: u32, sent: *mut u32, flags: u32, to: *const u8, tolen: i32, ov: *mut c_void, routine: *const c_void,
) -> i32 {
    if !dest_allowed(to, tolen) {
        deny_dest("WSASendTo", to, tolen);
        return -1;
    }
    orig!(H_WSASENDTO, fn(usize, *const c_void, u32, *mut u32, u32, *const u8, i32, *mut c_void, *const c_void) -> i32)(s, bufs, count, sent, flags, to, tolen, ov, routine)
}

static LOCAL_PROXY_SESSIONS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn proxy_is_local(list: &str) -> bool {
    let entries: Vec<&str> = list.split([';', ' ']).map(str::trim).filter(|e| !e.is_empty()).collect();
    !entries.is_empty() && entries.iter().all(|e| {
        let e = e.rsplit('=').next().unwrap_or(e);
        let e = e.split("://").last().unwrap_or(e);
        let host = if let Some(r) = e.strip_prefix('[') { r.split(']').next().unwrap_or("") } else { e.rsplit_once(':').map_or(e, |(h, _)| h) };
        matches!(host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1")
    })
}

fn session_uses_local_proxy(session: *mut c_void) -> bool {
    LOCAL_PROXY_SESSIONS.lock().map_or(false, |g| g.contains(&(session as usize)))
}
unsafe extern "system" fn h_winhttpconnect(session: *mut c_void, server: *const u16, port: u16, reserved: u32) -> *mut c_void {
    let f = orig!(H_WINHTTPCONNECT, fn(*mut c_void, *const u16, u16, u32) -> *mut c_void);
    if !server.is_null() && session_uses_local_proxy(session) {
        trace(&format!("WinHttpConnect '{}' port {port} goes through a proxy on this machine; the proxy is the hop", wstr(server)));
        let c = f(session, server, port, reserved);
        set_parent(c as usize, session as usize);
        return c;
    }
    if server.is_null() || passthrough(&wstr(server)) {
        let bp = BUILTIN_PORT.load(Ordering::Relaxed) as u16;
        let loopback = !server.is_null() && matches!(wstr(server).trim().to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost");
        let port = if bp != 0 && loopback && port == 80 {
            trace(&format!("WinHttpConnect loopback:80 -> built-in online services port {bp}"));
            bp
        } else {
            port
        };
        let c = f(session, server, port, reserved);
        set_parent(c as usize, session as usize);
        return c;
    }
    note_redirect(&format!("WinHttpConnect port {port}"), &wstr(server));
    let lp = LOCAL_PORT.load(Ordering::Relaxed) as u16;
    let c = f(session, target().unwrap().wide.as_ptr(), if lp != 0 { lp } else { port }, reserved);
    set_parent(c as usize, session as usize);
    if lp != 0 && !c.is_null() {
        set_orig_host(c as usize, wstr(server));
    }
    c
}

static ORIG_HOST: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());

fn set_orig_host(connect: usize, host: String) {
    if let Ok(mut g) = ORIG_HOST.lock() {
        g.retain(|(c, _)| *c != connect);
        g.push((connect, host));
        if g.len() > 4096 { g.remove(0); }
    }
}

pub const INPROC_PORT_ENV: &str = "TOMBOX_NET_INPROC_PORT";

pub fn in_process_port() -> u16 {
    match LOCAL_PORT.load(Ordering::Relaxed) {
        0 => std::env::var(INPROC_PORT_ENV).ok().and_then(|v| v.trim().parse::<u16>().ok()).unwrap_or(0),
        p => p as u16,
    }
}

unsafe fn request_url(req: *mut c_void) -> Option<String> {
    static Q: OnceLock<usize> = OnceLock::new();
    let p = *Q.get_or_init(|| resolve_export(b"winhttp.dll\0", b"WinHttpQueryOption\0") as usize);
    if p == 0 {
        return None;
    }
    let q: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut u32) -> i32 = core::mem::transmute(p);
    let mut buf = vec![0u16; 4096];
    let mut n = (buf.len() * 2) as u32;
    let e = GetLastError();
    let ok = q(req, 34, buf.as_mut_ptr() as *mut c_void, &mut n) != 0;
    SetLastError(e);
    if !ok {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..(n as usize / 2).min(buf.len())]).trim_end_matches('\0').to_string())
}

pub fn url_is_in_process(url: &str, port: u16) -> bool {
    if port == 0 {
        return false;
    }
    let l = url.to_ascii_lowercase();
    let Some(rest) = l.strip_prefix("http://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    authority == format!("127.0.0.1:{port}")
}

pub fn in_process_request(req: *mut c_void) -> Option<String> {
    if req.is_null() {
        return None;
    }
    if LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        let local = PARENT.lock().ok().and_then(|p| p.iter().rev().find(|(c, _)| *c == req as usize).map(|(_, p)| *p)).and_then(|connect| {
            ORIG_HOST.lock().ok()?.iter().rev().find(|(c, _)| *c == connect).map(|(_, h)| h.clone())
        });
        if local.is_some() {
            return local;
        }
    }
    let url = unsafe { request_url(req) }?;
    if url_is_in_process(&url, in_process_port()) {
        Some(url)
    } else {
        None
    }
}

unsafe fn add_forwarded_host(req: *mut c_void, connect: usize) {
    let host = match ORIG_HOST.lock() {
        Ok(g) => g.iter().rev().find(|(c, _)| *c == connect).map(|(_, h)| h.clone()),
        Err(_) => None,
    };
    let Some(host) = host.filter(|h| h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':')) else { return };
    static ADD: OnceLock<usize> = OnceLock::new();
    let p = *ADD.get_or_init(|| resolve_export(b"winhttp.dll\0", b"WinHttpAddRequestHeaders\0") as usize);
    if p == 0 { return; }
    let add: unsafe extern "system" fn(*mut c_void, *const u16, u32, u32) -> i32 = core::mem::transmute(p);
    let line: Vec<u16> = format!("X-Forwarded-Host: {host}").encode_utf16().chain(Some(0)).collect();
    add(req, line.as_ptr(), u32::MAX, 0xA000_0000);
}

unsafe extern "system" fn h_winhttpopenrequest(
    connect: *mut c_void, verb: *const u16, object: *const u16, version: *const u16, referrer: *const u16, accept: *const *const u16, flags: u32,
) -> *mut c_void {
    let f = orig!(H_WINHTTPOPENREQUEST, fn(*mut c_void, *const u16, *const u16, *const u16, *const u16, *const *const u16, u32) -> *mut c_void);
    let local = LOCAL_PORT.load(Ordering::Relaxed) != 0;
    let nflags = if local { flags & !0x0080_0000 } else { flags };
    let h = f(connect, verb, object, version, referrer, accept, nflags);
    set_parent(h as usize, connect as usize);
    if local && !h.is_null() {
        add_forwarded_host(h, connect as usize);
    }
    if local {
        let v = if verb.is_null() { "GET".to_string() } else { wstr(verb) };
        let o = if object.is_null() { "/".to_string() } else { wstr(object) };
        let o: String = o.chars().take(160).collect();
        trace(&format!("OpenRequest {v} {o} flags={flags:#x} -> {:#x} err={}", h as usize, if h.is_null() { GetLastError() } else { 0 }));
    }
    h
}

unsafe extern "system" fn h_winhttpsetoption(h: *mut c_void, opt: u32, buf: *mut c_void, len: u32) -> i32 {
    let f = orig!(H_WINHTTPSETOPTION, fn(*mut c_void, u32, *mut c_void, u32) -> i32);
    if opt == 47 && buf.is_null() && len == 0 && in_process_request(h).is_some() {
        return 1;
    }
    let r = f(h, opt, buf, len);
    if r == 0 && LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        let e = GetLastError();
        trace(&format!("SetOption {:#x} opt={opt} len={len} failed err={e}", h as usize));
        SetLastError(e);
    }
    r
}

unsafe extern "system" fn h_winhttpqueryoption(h: *mut c_void, opt: u32, buf: *mut c_void, len: *mut u32) -> i32 {
    let f = orig!(H_WINHTTPQUERYOPTION, fn(*mut c_void, u32, *mut c_void, *mut u32) -> i32);
    let r = f(h, opt, buf, len);
    if r == 0 && LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        let e = GetLastError();
        if e != 122 {
            trace(&format!("QueryOption {:#x} opt={opt} failed err={e}", h as usize));
        }
        SetLastError(e);
    }
    r
}

unsafe extern "system" fn h_winhttpsendrequest(h: *mut c_void, hdr: *const u16, hlen: u32, opt: *mut c_void, olen: u32, total: u32, ctx: usize) -> i32 {
    let f = orig!(H_WINHTTPSENDREQUEST, fn(*mut c_void, *const u16, u32, *mut c_void, u32, u32, usize) -> i32);
    let r = f(h, hdr, hlen, opt, olen, total, ctx);
    if LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        let e = if r == 0 { GetLastError() } else { 0 };
        trace(&format!("SendRequest {:#x} body={olen}/{total} -> {r} err={e}", h as usize));
        if r == 0 { SetLastError(e); }
    }
    r
}

unsafe extern "system" fn h_winhttpreceive(h: *mut c_void, reserved: *mut c_void) -> i32 {
    let f = orig!(H_WINHTTPRECEIVE, fn(*mut c_void, *mut c_void) -> i32);
    let r = f(h, reserved);
    if LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        let e = if r == 0 { GetLastError() } else { 0 };
        trace(&format!("ReceiveResponse {:#x} -> {r} err={e}", h as usize));
        if r == 0 { SetLastError(e); }
    }
    r
}

unsafe extern "system" fn h_winhttpqueryavail(h: *mut c_void, n: *mut u32) -> i32 {
    let f = orig!(H_WINHTTPQUERYAVAIL, fn(*mut c_void, *mut u32) -> i32);
    let r = f(h, n);
    if LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        let e = if r == 0 { GetLastError() } else { 0 };
        let v = if n.is_null() || r == 0 { -1 } else { *n as i64 };
        trace(&format!("QueryDataAvailable {:#x} -> {r} n={v} err={e}", h as usize));
        if r == 0 { SetLastError(e); }
    }
    r
}

unsafe extern "system" fn h_winhttpreaddata(h: *mut c_void, buf: *mut c_void, len: u32, read: *mut u32) -> i32 {
    let f = orig!(H_WINHTTPREADDATA, fn(*mut c_void, *mut c_void, u32, *mut u32) -> i32);
    let r = f(h, buf, len, read);
    if LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        let e = if r == 0 { GetLastError() } else { 0 };
        let v = if read.is_null() || r == 0 { -1 } else { *read as i64 };
        trace(&format!("ReadData {:#x} len={len} -> {r} read={v} err={e}", h as usize));
        if r == 0 { SetLastError(e); }
    }
    r
}

unsafe extern "system" fn h_winhttpqueryheaders(h: *mut c_void, level: u32, name: *const u16, buf: *mut c_void, len: *mut u32, idx: *mut u32) -> i32 {
    let f = orig!(H_WINHTTPQUERYHEADERS, fn(*mut c_void, u32, *const u16, *mut c_void, *mut u32, *mut u32) -> i32);
    let r = f(h, level, name, buf, len, idx);
    if LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        let e = if r == 0 { GetLastError() } else { 0 };
        let val = if r != 0 && !buf.is_null() && level & 0x2000_0000 != 0 { format!(" num={}", *(buf as *const u32)) } else { String::new() };
        trace(&format!("QueryHeaders {:#x} level={level:#x} -> {r}{val} err={e}", h as usize));
        if r == 0 { SetLastError(e); }
    }
    r
}

unsafe extern "system" fn h_winhttpwsupgrade(req: *mut c_void, ctx: usize) -> *mut c_void {
    let f = orig!(H_WINHTTPWSUPGRADE, fn(*mut c_void, usize) -> *mut c_void);
    let prior = CREATING_UNDER.with(|c| c.replace(req as usize));
    let ws = f(req, ctx);
    CREATING_UNDER.with(|c| c.set(prior));
    if !ws.is_null() {
        set_parent(ws as usize, req as usize);
    }
    if LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        let e = if ws.is_null() { GetLastError() } else { 0 };
        trace(&format!("WebSocketCompleteUpgrade {:#x} -> {:#x} err={e}", req as usize, ws as usize));
        if ws.is_null() { SetLastError(e); }
    }
    ws
}

type StatusCb = unsafe extern "system" fn(*mut c_void, usize, u32, *mut c_void, u32);

unsafe extern "system" fn cb_wrap(h: *mut c_void, ctx: usize, status: u32, info: *mut c_void, len: u32) {
    match status {
        0x0020_0000 if !info.is_null() => {
            let res = core::ptr::read_unaligned(info as *const usize);
            let err = core::ptr::read_unaligned((info as *const u8).add(8) as *const u32);
            trace(&format!("callback {:#x} REQUEST_ERROR api={res} err={err}", h as usize));
        }
        0x0001_0000 => {
            let fl = if info.is_null() { 0 } else { core::ptr::read_unaligned(info as *const u32) };
            trace(&format!("callback {:#x} SECURE_FAILURE flags={fl:#x}", h as usize));
        }
        0x0002_0000 => trace(&format!("callback {:#x} HEADERS_AVAILABLE", h as usize)),
        0x0040_0000 => trace(&format!("callback {:#x} SENDREQUEST_COMPLETE", h as usize)),
        _ => {}
    }
    let owner = find_cb(h as usize).or_else(|| if adopt_creating(h as usize) { find_cb(h as usize) } else { None });
    let Some((cb, fl)) = owner else {
        trace(&format!("callback {:#x} status={status:#x} has no known owner - dropped", h as usize));
        return;
    };
    if status & fl != 0 {
        let f: StatusCb = core::mem::transmute(cb);
        f(h, ctx, status, info, len);
    }
    if status == 0x0000_0800 {
        forget_handle(h as usize);
    }
}

unsafe extern "system" fn h_winhttpsetcallback(h: *mut c_void, cb: usize, flags: u32, reserved: usize) -> usize {
    let f = orig!(H_WINHTTPSETCALLBACK, fn(*mut c_void, usize, u32, usize) -> usize);
    if LOCAL_PORT.load(Ordering::Relaxed) == 0 || cb == 0 || cb == usize::MAX {
        return f(h, cb, flags, reserved);
    }
    if let Ok(mut g) = GAME_CB.lock() {
        g.retain(|(k, _, _)| *k != h as usize);
        g.push((h as usize, cb, flags));
        if g.len() > 4096 { g.remove(0); }
    }
    let prev = f(h, cb_wrap as usize, flags | 0x0001_0000 | 0x0020_0000 | 0x0002_0000 | 0x0040_0000 | 0x0000_0800, reserved);
    if prev == cb_wrap as usize { 0 } else { prev }
}

unsafe extern "system" fn h_winhttpopen(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> *mut c_void {
    let f = orig!(H_WINHTTPOPEN, fn(*const u16, u32, *const u16, *const u16, u32) -> *mut c_void);
    let nflags = if LOCAL_PORT.load(Ordering::Relaxed) != 0 && flags & 0x3000_0000 == 0x3000_0000 { (flags & !0x3000_0000) | 0x1000_0000 } else { flags };
    let h = f(agent, access, proxy, bypass, nflags);
    if !h.is_null() && access == 3 && !proxy.is_null() && proxy_is_local(&wstr(proxy)) {
        if let Ok(mut g) = LOCAL_PROXY_SESSIONS.lock() {
            g.retain(|s| *s != h as usize);
            g.push(h as usize);
            if g.len() > 256 { g.remove(0); }
        }
    }
    if LOCAL_PORT.load(Ordering::Relaxed) != 0 {
        trace(&format!("Open flags={flags:#x} -> {nflags:#x} -> {:#x}", h as usize));
    }
    h
}

// ---- installation -------------------------------------------------------------------------------

fn install_deny() {
    let ws = b"ws2_32.dll\0";
    let wh = b"winhttp.dll\0";
    let table: [(&[u8], &[u8], usize); 14] = [
        (ws, b"getaddrinfo\0", deny_resolve as usize),
        (ws, b"GetAddrInfoW\0", deny_resolve as usize),
        (ws, b"GetAddrInfoExW\0", deny_resolve as usize),
        (ws, b"GetAddrInfoExA\0", deny_resolve as usize),
        (ws, b"gethostbyname\0", deny_null as usize),
        (ws, b"connect\0", deny_sock as usize),
        (ws, b"WSAConnect\0", deny_sock as usize),
        (ws, b"WSAConnectByNameW\0", deny_sock_bool as usize),
        (ws, b"WSAConnectByNameA\0", deny_sock_bool as usize),
        (ws, b"sendto\0", deny_sock as usize),
        (ws, b"WSASendTo\0", deny_sock as usize),
        (wh, b"WinHttpConnect\0", deny_null as usize),
        (wh, b"WinHttpOpenRequest\0", deny_null as usize),
        (wh, b"WinHttpSendRequest\0", deny_false as usize),
    ];
    let mut ok = 0;
    let mut missing = Vec::new();
    for (m, f, stub) in table {
        let p = unsafe { resolve_export(m, f) };
        if !p.is_null() && unsafe { write_jump(p, stub) } {
            ok += 1;
        } else {
            missing.push(String::from_utf8_lossy(&f[..f.len() - 1]).into_owned());
        }
    }
    crate::record_raw("net_guard", "installed", &format!(
        "{ok}/14 network entry points blocked in-process{}",
        if missing.is_empty() { String::new() } else { format!(" (not patched: {})", missing.join(", ")) }));
    ext::install(false);
}

fn install_redirect(host: &str) {
    let ws = b"ws2_32.dll\0";
    let wh = b"winhttp.dll\0";
    // (module, export, index, hook, deny stub used if the original cannot be trampolined)
    let table: [(&[u8], &[u8], usize, usize, usize); 23] = [
        (ws, b"getaddrinfo\0", H_GETADDRINFO, h_getaddrinfo as usize, deny_resolve as usize),
        (ws, b"GetAddrInfoW\0", H_GETADDRINFOW, h_getaddrinfow as usize, deny_resolve as usize),
        (ws, b"GetAddrInfoExW\0", H_GETADDRINFOEXW, h_getaddrinfoexw as usize, deny_resolve as usize),
        (ws, b"GetAddrInfoExA\0", H_GETADDRINFOEXA, h_getaddrinfoexa as usize, deny_resolve as usize),
        (ws, b"gethostbyname\0", H_GETHOSTBYNAME, h_gethostbyname as usize, deny_null as usize),
        (ws, b"connect\0", H_CONNECT, h_connect as usize, deny_sock as usize),
        (ws, b"WSAConnect\0", H_WSACONNECT, h_wsaconnect as usize, deny_sock as usize),
        (ws, b"WSAConnectByNameW\0", H_CONNECTBYNAMEW, h_connectbynamew as usize, deny_sock_bool as usize),
        (ws, b"WSAConnectByNameA\0", H_CONNECTBYNAMEA, h_connectbynamea as usize, deny_sock_bool as usize),
        (ws, b"sendto\0", H_SENDTO, h_sendto as usize, deny_sock as usize),
        (ws, b"WSASendTo\0", H_WSASENDTO, h_wsasendto as usize, deny_sock as usize),
        (wh, b"WinHttpConnect\0", H_WINHTTPCONNECT, h_winhttpconnect as usize, deny_null as usize),
        (wh, b"WinHttpOpenRequest\0", H_WINHTTPOPENREQUEST, h_winhttpopenrequest as usize, deny_null as usize),
        (wh, b"WinHttpOpen\0", H_WINHTTPOPEN, h_winhttpopen as usize, deny_null as usize),
        (wh, b"WinHttpSetOption\0", H_WINHTTPSETOPTION, h_winhttpsetoption as usize, deny_false as usize),
        (wh, b"WinHttpQueryOption\0", H_WINHTTPQUERYOPTION, h_winhttpqueryoption as usize, deny_false as usize),
        (wh, b"WinHttpSendRequest\0", H_WINHTTPSENDREQUEST, h_winhttpsendrequest as usize, deny_false as usize),
        (wh, b"WinHttpSetStatusCallback\0", H_WINHTTPSETCALLBACK, h_winhttpsetcallback as usize, deny_null as usize),
        (wh, b"WinHttpReceiveResponse\0", H_WINHTTPRECEIVE, h_winhttpreceive as usize, deny_false as usize),
        (wh, b"WinHttpQueryDataAvailable\0", H_WINHTTPQUERYAVAIL, h_winhttpqueryavail as usize, deny_false as usize),
        (wh, b"WinHttpReadData\0", H_WINHTTPREADDATA, h_winhttpreaddata as usize, deny_false as usize),
        (wh, b"WinHttpQueryHeaders\0", H_WINHTTPQUERYHEADERS, h_winhttpqueryheaders as usize, deny_false as usize),
        (wh, b"WinHttpWebSocketCompleteUpgrade\0", H_WINHTTPWSUPGRADE, h_winhttpwsupgrade as usize, deny_null as usize),
    ];
    let (mut hooked, mut denied, mut missing) = (0, Vec::new(), Vec::new());
    for (m, f, idx, hook, deny) in table {
        let name = String::from_utf8_lossy(&f[..f.len() - 1]).into_owned();
        let p = unsafe { resolve_export(m, f) };
        if p.is_null() {
            missing.push(name);
            continue;
        }
        match unsafe { make_trampoline(p) } {
            Some(t) => {
                ORIG[idx].store(t, Ordering::SeqCst);
                if unsafe { write_jump(p, hook) } { hooked += 1 } else { missing.push(name) }
            }
            None => {
                // Cannot call the original safely: fail closed for this entry point.
                if unsafe { write_jump(p, deny) } { denied.push(name) } else { missing.push(name) }
            }
        }
    }
    crate::record_raw("net_guard", "installed", &format!(
        "REDIRECT mode: every host -> '{host}'{}; {hooked}/23 entry points hooked{}{}",
        match LOCAL_PORT.load(Ordering::Relaxed) { 0 => String::new(), p => format!(" (local backend: WinHTTP -> port {p}, plain HTTP)") },
        if denied.is_empty() { String::new() } else { format!("; fail-closed (no trampoline): {}", denied.join(", ")) },
        if missing.is_empty() { String::new() } else { format!("; NOT PATCHED: {}", missing.join(", ")) }));
    ext::install(true);
    if ORIG[H_GETADDRINFO].load(Ordering::Relaxed) != 0 {
        unsafe { refresh_allowed() };
    }
}

/// Install the policy once per process (the first compat DLL to log wins).
pub fn ensure() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Several compat DLLs link this crate; only one may patch the process.
        let marker = format!("{}", unsafe { GetCurrentProcessId() });
        if std::env::var("TOMBOX_NET_GUARD_OWNER").ok().as_deref() == Some(marker.as_str()) {
            let _ = TARGET.set(None);
            return;
        }
        std::env::set_var("TOMBOX_NET_GUARD_OWNER", &marker);

        // Process-wide one-time setup that is not network policy: the local Xbox user.
        crate::gdk_user::install();
        if let Some(locale) = crate::cfg("locale").filter(|v| !v.is_empty()) {
            crate::locale::install(&locale);
        }

        if std::env::var_os("TOMBOX_ALLOW_NETWORK").map(|v| v == "1").unwrap_or(false) {
            let _ = TARGET.set(None);
            crate::record_raw("net_guard", "disabled", "TOMBOX_ALLOW_NETWORK=1 — title network access NOT restricted");
            return;
        }
        // Redirect host: environment first, else a `net_redirect=<host>` line in `tombox.cfg`
        // next to the title executable (so a plain double-click launch gets the same policy).
        let from_cfg = || crate::cfg("net_redirect");
        let builtin = crate::online::requested();
        let host = if builtin {
            match crate::online::start_for_config() {
                Ok(p) => {
                    BUILTIN_PORT.store(p as usize, Ordering::Relaxed);
                    crate::record_raw("net_guard", "online", &format!("built-in online services listening on 127.0.0.1:{p}"));
                    Some("127.0.0.1".to_string())
                }
                Err(e) => {
                    crate::record_raw("net_guard", "online", &format!("built-in online services failed to start ({e}) - falling back to deny-all"));
                    None
                }
            }
        } else {
            std::env::var("TOMBOX_NET_REDIRECT").ok().or_else(from_cfg).map(|h| h.trim().to_string()).filter(|h| !h.is_empty())
        };
        match host {
            Some(h) if h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') => {
                let mut ansi = h.clone().into_bytes();
                ansi.push(0);
                let wide: Vec<u16> = h.encode_utf16().chain(Some(0)).collect();
                let _ = TARGET.set(Some(Target { name: h.clone(), ansi, wide }));
                let lp = match BUILTIN_PORT.load(Ordering::Relaxed) {
                    0 => std::env::var("TOMBOX_NET_LOCAL_PORT").ok().or_else(|| crate::cfg("net_local_port"))
                        .and_then(|v| v.trim().parse::<u16>().ok()).unwrap_or(0),
                    p => p as u16,
                };
                if lp != 0 && h == "127.0.0.1" {
                    LOCAL_PORT.store(lp as usize, Ordering::Relaxed);
                    std::env::set_var(INPROC_PORT_ENV, lp.to_string());
                }
                install_redirect(&h);
            }
            Some(h) => {
                let _ = TARGET.set(None);
                crate::record_raw("net_guard", "config", &format!("TOMBOX_NET_REDIRECT='{h}' is not a plain host name - falling back to deny-all"));
                install_deny();
            }
            None => {
                let _ = TARGET.set(None);
                install_deny();
            }
        }
    });
}

#[cfg(test)]
mod callback_owner_tests {
    use super::*;

    static SEEN: Mutex<Vec<(usize, u32, usize)>> = Mutex::new(Vec::new());

    unsafe extern "system" fn http_cb(h: *mut c_void, ctx: usize, status: u32, _: *mut c_void, _: u32) {
        SEEN.lock().unwrap().push((h as usize, status, ctx ^ 0x1));
    }

    unsafe extern "system" fn ws_cb(h: *mut c_void, ctx: usize, status: u32, _: *mut c_void, _: u32) {
        SEEN.lock().unwrap().push((h as usize, status, ctx ^ 0x2));
    }

    fn register(h: usize, cb: usize, flags: u32) {
        GAME_CB.lock().unwrap().push((h, cb, flags));
    }

    #[test]
    fn reused_handle_value_does_not_inherit_a_closed_handles_callback() {
        let (old_req, ws_parent, reused) = (0x7a10_0000usize, 0x7a20_0000usize, 0x7a30_0000usize);
        set_parent(old_req, 0x7a00_0000);
        register(old_req, http_cb as usize, 0xffff_ffff);
        register(reused, http_cb as usize, 0xffff_ffff);
        register(ws_parent, ws_cb as usize, 0x0008_0000);
        set_parent(reused, ws_parent);
        assert_eq!(find_cb(reused), Some((ws_cb as usize, 0x0008_0000)));
        unsafe { cb_wrap(old_req as *mut c_void, 0x10, 0x0000_0800, core::ptr::null_mut(), 0) };
        assert!(find_cb(old_req).is_none());
        let seen = SEEN.lock().unwrap().clone();
        assert!(seen.contains(&(old_req, 0x800, 0x11)), "{seen:x?}");
    }

    #[test]
    fn websocket_keeps_its_callback_after_the_upgrade_request_closes_and_its_value_is_reused() {
        let (session, req, ws, other_parent) = (0x7b00_0000usize, 0x7b10_0000usize, 0x7b20_0000usize, 0x7b30_0000usize);
        set_parent(req, session);
        register(req, ws_cb as usize, 0xffff_ffff);
        set_parent(ws, req);
        unsafe { cb_wrap(req as *mut c_void, 0x30, 0x0000_0800, core::ptr::null_mut(), 0) };
        register(other_parent, http_cb as usize, 0x0000_0800);
        set_parent(req, other_parent);
        register(req, http_cb as usize, 0xffff_ffff);
        assert_eq!(find_cb(ws), Some((ws_cb as usize, 0xffff_ffff)));
        unsafe { cb_wrap(ws as *mut c_void, 0x40, 0x0008_0000, core::ptr::null_mut(), 0) };
        assert!(SEEN.lock().unwrap().contains(&(ws, 0x0008_0000, 0x42)));
        unsafe { cb_wrap(ws as *mut c_void, 0x40, 0x0000_0800, core::ptr::null_mut(), 0) };
        assert!(find_cb(ws).is_none());
    }
}
