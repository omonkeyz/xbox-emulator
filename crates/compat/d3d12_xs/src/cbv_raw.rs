use compat_common::record;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

pub use tombox_xshader::cbv_raw_hlsl::{map_space, rewrite_hlsl, TB_CBRAW_SPACE};
const RANGE_SRV: u32 = 0;
const RANGE_CBV: u32 = 2;
const PARAM_TABLE: u32 = 0;
const PARAM_CBV: u32 = 2;
const PARAM_SRV: u32 = 3;
const XBOX_SLOT_COMPUTE_ROOT_CBV: usize = 41;
const XBOX_SLOT_GRAPHICS_ROOT_CBV: usize = 42;
const PC_SLOT_COMPUTE_ROOT_SRV: usize = 39;
const PC_SLOT_GRAPHICS_ROOT_SRV: usize = 40;
const DXGI_R32_TYPELESS: u32 = 39;
const SRV_DIMENSION_BUFFER: u32 = 1;
const DEFAULT_MAPPING: u32 = 0x1688;
const BUFFER_SRV_FLAG_RAW: u32 = 1;
const DEVICE_CREATE_SRV: usize = 18;

static PREV_RS: AtomicUsize = AtomicUsize::new(0);
static PREV_CBV: AtomicUsize = AtomicUsize::new(0);
static REAL_ROOT_SRV: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    On,
    Off,
}

pub fn parse_mode(v: Option<&str>) -> Result<Mode, String> {
    match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("auto") => Ok(Mode::Auto),
        Some("1") | Some("on") => Ok(Mode::On),
        Some("0") | Some("off") => Ok(Mode::Off),
        Some(other) => Err(format!("cbv_raw={other:?} is not one of auto, on, off, 1, 0; using auto")),
    }
}

pub fn xbox_one_title() -> bool {
    static X: OnceLock<bool> = OnceLock::new();
    *X.get_or_init(|| {
        extern "system" {
            fn GetModuleHandleW(n: *const u16) -> *mut c_void;
        }
        let name: Vec<u16> = "d3d12_x.dll".encode_utf16().chain(Some(0)).collect();
        !unsafe { GetModuleHandleW(name.as_ptr()) }.is_null()
    })
}

pub fn shader_kind_refusal(xbox_one: bool, durango: bool) -> Option<&'static str> {
    (xbox_one && !durango).then_some("Xbox One title shader is not a Durango program")
}

pub fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| {
        let raw = std::env::var("TOMBOX_CBV_RAW").ok().or_else(|| compat_common::cfg("cbv_raw"));
        let mode = parse_mode(raw.as_deref()).unwrap_or_else(|e| {
            record("d3d12_xs", "cbv_raw", &e);
            Mode::Auto
        });
        let xbox_one = xbox_one_title();
        let on = match mode {
            Mode::Auto => xbox_one,
            Mode::On => true,
            Mode::Off => false,
        };
        let why = match (mode, xbox_one) {
            (Mode::Auto, _) => "Xbox One title (d3d12_x.dll loaded)",
            (Mode::On, _) => "cbv_raw=on",
            (Mode::Off, true) => "cbv_raw=off on an Xbox One title (d3d12_x.dll loaded)",
            (Mode::Off, false) => "cbv_raw=off",
        };
        if on {
            record("d3d12_xs", "cbv_raw", &format!("{why}: constant buffers are bound as raw SRVs in space {TB_CBRAW_SPACE}+n so a CBV may start at any 16-byte-aligned address like on the console"));
        } else if mode == Mode::Off {
            record("d3d12_xs", "cbv_raw", &format!("{why}: constant buffers stay PC CBVs (BufferLocation must be 256-byte aligned)"));
        }
        on
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RsPatch {
    pub ranges: usize,
    pub root_cbvs: Vec<usize>,
    pub aliased: bool,
}

fn rd(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| format!("RTS0 read at {o:#x} out of bounds ({} bytes)", b.len()))
}

fn wr(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}


pub fn patch_rts0(b: &mut [u8]) -> Result<RsPatch, String> {
    let version = rd(b, 0)?;
    let range_size = match version {
        1 => 20,
        2 | 3 => 24,
        v => return Err(format!("RTS0 version {v} is not supported")),
    };
    let (np, po) = (rd(b, 4)? as usize, rd(b, 8)? as usize);
    let mut p = RsPatch::default();
    for i in 0..np {
        let at = po + i * 12;
        let (ty, payload) = (rd(b, at)?, rd(b, at + 8)? as usize);
        match ty {
            PARAM_TABLE => {
                let (nr, ro) = (rd(b, payload)? as usize, rd(b, payload + 4)? as usize);
                for r in 0..nr {
                    let ra = ro + r * range_size;
                    if rd(b, ra)? == RANGE_CBV {
                        let space = map_space(rd(b, ra + 12)?)?;
                        wr(b, ra, RANGE_SRV);
                        wr(b, ra + 12, space);
                        p.ranges += 1;
                    }
                }
            }
            PARAM_CBV => {
                let space = map_space(rd(b, payload + 4)?)?;
                wr(b, at, PARAM_SRV);
                wr(b, payload + 4, space);
                p.root_cbvs.push(i);
            }
            _ => {}
        }
    }
    Ok(p)
}

pub fn patch_blob(blob: &[u8]) -> Result<Option<(Vec<u8>, RsPatch)>, String> {
    let (o, n) = crate::console_slots_mesh::rts0_range(blob).ok_or("root signature blob has no RTS0 part")?;
    let mut body = blob.get(o..o + n).ok_or("RTS0 chunk out of bounds")?.to_vec();
    let mut p = patch_rts0(&mut body)?;
    if let Some(b) = crate::bl_alias::add_aliases(&body)? {
        body = b;
        p.aliased = true;
    }
    if p.ranges == 0 && p.root_cbvs.is_empty() && !p.aliased {
        return Ok(None);
    }
    Ok(Some((crate::bl_alias::replace_rts0(blob, &body)?, p)))
}

pub fn raw_view(loc: u64, size: u32, lookup: &dyn Fn(u64) -> Option<(crate::gpu_va::Entry, u64)>) -> Result<(usize, [u8; 40]), String> {
    let mut d = [0u8; 40];
    wr(&mut d, 0, DXGI_R32_TYPELESS);
    wr(&mut d, 4, SRV_DIMENSION_BUFFER);
    wr(&mut d, 8, DEFAULT_MAPPING);
    wr(&mut d, 32, BUFFER_SRV_FLAG_RAW);
    if loc == 0 {
        return Ok((0, d));
    }
    if loc & 15 != 0 {
        return Err(format!("CBV BufferLocation {loc:#x} is not 16-byte aligned (the console V# base granularity)"));
    }
    let (e, off) = lookup(loc).ok_or_else(|| format!("CBV BufferLocation {loc:#x} is not inside any known buffer"))?;
    if !e.buffer {
        return Err(format!("CBV BufferLocation {loc:#x} lies in a texture resource {:#x}", e.res));
    }
    if off & 15 != 0 {
        return Err(format!("CBV BufferLocation {loc:#x} is {off:#x} bytes into its resource, not 16-byte aligned"));
    }
    let avail = e.len.saturating_sub(off);
    let n = (size as u64).min(avail) / 4;
    if n == 0 {
        return Err(format!("CBV BufferLocation {loc:#x} size {size} leaves no data in its resource ({avail} bytes left)"));
    }
    d[16..24].copy_from_slice(&(off / 4).to_le_bytes());
    wr(&mut d, 24, n as u32);
    Ok((e.res, d))
}


unsafe fn real_of(this: *mut c_void) -> *mut c_void {
    unsafe { *(this as *const *mut c_void).add(1) }
}

unsafe fn vslot(o: *mut c_void, i: usize) -> usize {
    unsafe { *(*(o as *const *const usize)).add(i) }
}

unsafe extern "system" fn create_root_signature(this: *mut c_void, mask: u32, blob: *const u8, len: usize, riid: *const c_void, ppv: *mut *mut c_void) -> i32 {
    let f: unsafe extern "system" fn(*mut c_void, u32, *const u8, usize, *const c_void, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(PREV_RS.load(Ordering::Relaxed)) };
    if blob.is_null() || len == 0 || !enabled() {
        return unsafe { f(this, mask, blob, len, riid, ppv) };
    }
    if check_on() {
        log_root_regs(unsafe { core::slice::from_raw_parts(blob, len) });
    }
    let hr = match patch_blob(unsafe { core::slice::from_raw_parts(blob, len) }) {
        Ok(None) => unsafe { f(this, mask, blob, len, riid, ppv) },
        Ok(Some((b, p))) => {
            static N: AtomicUsize = AtomicUsize::new(0);
            if N.fetch_add(1, Ordering::Relaxed) < 3 {
                record("d3d12_xs", "cbv_raw", &format!("root signature: {} CBV range(s) and root CBV parameter(s) {:?} rebound as raw SRVs, unbounded ranges aliased: {}", p.ranges, p.root_cbvs, p.aliased));
            }
            unsafe { f(this, mask, b.as_ptr(), b.len(), riid, ppv) }
        }
        Err(e) => {
            record("d3d12_xs", "cbv_raw", &format!("root signature not rebound ({e}); created as given"));
            unsafe { f(this, mask, blob, len, riid, ppv) }
        }
    };
    if hr >= 0 && track_on() && !ppv.is_null() {
        let rs = unsafe { *ppv } as usize;
        let regs = root_cbv_params(unsafe { core::slice::from_raw_parts(blob, len) });
        dc_with(|d| { d.rs.insert(rs, regs); });
    }
    hr
}

unsafe extern "system" fn create_cbv(this: *mut c_void, pdesc: *const u8, dest: usize) {
    if !enabled() {
        let f: unsafe extern "system" fn(*mut c_void, *const u8, usize) = unsafe { core::mem::transmute(PREV_CBV.load(Ordering::Relaxed)) };
        return unsafe { f(this, pdesc, dest) };
    }
    let (loc, size) = if pdesc.is_null() {
        (0, 0)
    } else {
        unsafe { (core::ptr::read_unaligned(pdesc as *const u64), core::ptr::read_unaligned(pdesc.add(8) as *const u32)) }
    };
    let real = unsafe { real_of(this) };
    match raw_view(loc, size, &crate::gpu_va::lookup) {
        Ok((res, d)) => {
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(vslot(real, DEVICE_CREATE_SRV)) };
            unsafe { f(real, res as *mut c_void, d.as_ptr(), dest) };
            unsafe { crate::ecl_probe::after_device_call(real, "CreateConstantBufferView (raw SRV)", || format!("BufferLocation={loc:#x} size {size} res={res:#x} dest={dest:#x}")) };
        }
        Err(e) => {
            static N: AtomicUsize = AtomicUsize::new(0);
            if N.fetch_add(1, Ordering::Relaxed) < 16 {
                record("d3d12_xs", "cbv_raw", &format!("{e}; descriptor {dest:#x} left unwritten"));
            }
        }
    }
}

unsafe extern "system" fn set_compute_root_cbv_x(this: *mut c_void, i: u32, va: u64) {
    if check_on() {
        check_root(i, va, true);
    }
    if crate::rtcap::enabled() {
        crate::bindtrace::set_root_cbv(this as usize, true, i, va);
    }
    let f: unsafe extern "system" fn(*mut c_void, u32, u64) = unsafe { core::mem::transmute(REAL_ROOT_SRV[0].load(Ordering::Relaxed)) };
    unsafe { f(this, i, va) }
}

unsafe extern "system" fn set_graphics_root_cbv_x(this: *mut c_void, i: u32, va: u64) {
    if check_on() {
        check_root(i, va, false);
    }
    if crate::rtcap::enabled() {
        crate::bindtrace::set_root_cbv(this as usize, false, i, va);
    }
    if track_on() {
        dc_with(|d| {
            let l = d.lists.entry(this as usize).or_default();
            if (i as usize) < 64 {
                l.1 |= 1u64 << i;
                l.2.insert(i, va);
            }
            d.last.entry(this as usize).or_default().insert(i, va);
        });
    }
    let f: unsafe extern "system" fn(*mut c_void, u32, u64) = unsafe { core::mem::transmute(REAL_ROOT_SRV[1].load(Ordering::Relaxed)) };
    unsafe { f(this, i, va) }
}

fn check_on() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| std::env::var("TOMBOX_CBRAW_CHECK").map(|v| v == "1").unwrap_or(false))
}

fn check_root(i: u32, va: u64, compute: bool) {
    static N: AtomicUsize = AtomicUsize::new(0);
    static OK: AtomicUsize = AtomicUsize::new(0);
    let what = match (crate::gpu_va::lookup_gpu(va), crate::gpu_va::lookup(va)) {
        _ if va == 0 => Some("null VA".to_string()),
        (Some((e, off)), _) if e.len.saturating_sub(off) < 0x800 => Some(format!("PC GPU VA in res {:#x} at +{off:#x} of {:#x}, only {:#x} bytes left", e.res, e.len, e.len.saturating_sub(off))),
        (Some(_), _) => None,
        (None, Some((e, off))) => Some(format!("not a PC GPU VA; title-memory VA of res {:#x} +{off:#x} (len {:#x})", e.res, e.len)),
        (None, None) => Some("not inside any known buffer".to_string()),
    };
    match what {
        Some(w) if N.fetch_add(1, Ordering::Relaxed) < 64 => record("d3d12_xs", "cbv_raw_check", &format!("root CBV param {i} compute={compute} va={va:#x}: {w}")),
        None if OK.fetch_add(1, Ordering::Relaxed) == 0 => record("d3d12_xs", "cbv_raw_check", &format!("first good root CBV param {i} va={va:#x}")),
        _ => {}
    }
}

pub unsafe fn install_root_cbv_hooks(xbox_vt: &[AtomicUsize], real_vt: *const usize) {
    if !enabled() || xbox_vt.len() <= XBOX_SLOT_GRAPHICS_ROOT_CBV {
        return;
    }
    unsafe {
        REAL_ROOT_SRV[0].store(*real_vt.add(PC_SLOT_COMPUTE_ROOT_SRV), Ordering::Relaxed);
        REAL_ROOT_SRV[1].store(*real_vt.add(PC_SLOT_GRAPHICS_ROOT_SRV), Ordering::Relaxed);
    }
    xbox_vt[XBOX_SLOT_COMPUTE_ROOT_CBV].store(set_compute_root_cbv_x as usize, Ordering::Relaxed);
    xbox_vt[XBOX_SLOT_GRAPHICS_ROOT_CBV].store(set_graphics_root_cbv_x as usize, Ordering::Relaxed);
}

pub fn hook_device(v: &mut [usize]) {
    if v.len() <= 17 || v[16] == 0 || v[17] == 0 || v[17] == create_cbv as usize {
        return;
    }
    PREV_RS.store(v[16], Ordering::Relaxed);
    PREV_CBV.store(v[17], Ordering::Relaxed);
    v[16] = create_root_signature as usize;
    v[17] = create_cbv as usize;
}

pub fn clamp_knob() -> Option<(u32, u32, Option<u64>)> {
    static K: OnceLock<Option<(u32, u32, Option<u64>)>> = OnceLock::new();
    *K.get_or_init(|| parse_clamp(&std::env::var("TOMBOX_CBRAW_CLAMP").ok()?))
}

pub fn parse_clamp(v: &str) -> Option<(u32, u32, Option<u64>)> {
    let mut it = v.split(':');
    let a = it.next()?;
    let lim = it.next()?.trim().trim_start_matches("0x");
    let lim = if lim == "z" { u32::MAX } else { u32::from_str_radix(lim, 16).ok()? };
    let only = match it.next() {
        Some(h) => Some(u64::from_str_radix(h.trim(), 16).ok()?),
        None => None,
    };
    Some((a.trim().parse().ok()?, lim, only))
}

pub fn clamp_reads(h: &str, slot: u32, lim: u32) -> String {
    let decl = format!("ByteAddressBuffer tb_cbr{slot} : register(");
    let Some(at) = h.find(&decl) else { return h.to_string() };
    let eol = h[at..].find('\n').map(|i| at + i + 1).unwrap_or(h.len());
    let body = h[eol..].replace(&format!("tb_cbr{slot}.Load4("), &format!("tb_cbc{slot}("));
    if lim == u32::MAX {
        return format!("{}uint4 tb_cbc{slot}(uint o) {{ uint4 v = tb_cbr{slot}.Load4(o); return (v.x == 0x7fc1d00du && v.y == 0x7fc1d00du) ? v : uint4(0u, 0u, 0u, 0u); }}\n{}", &h[..eol], body);
    }
    format!("{}uint4 tb_cbc{slot}(uint o) {{ return o < {lim:#x}u ? tb_cbr{slot}.Load4(o) : uint4(0u, 0u, 0u, 0u); }}\n{}", &h[..eol], body)
}

pub fn root_cbv_regs(blob: &[u8]) -> Vec<(usize, u32, u32)> {
    root_cbv_params(blob).into_iter().map(|(i, r, sp, _)| (i, r, sp)).collect()
}

pub fn root_cbv_params(blob: &[u8]) -> Vec<(usize, u32, u32, u32)> {
    let mut out = Vec::new();
    let Some((o, n)) = crate::console_slots_mesh::rts0_range(blob) else { return out };
    let Some(b) = blob.get(o..o + n) else { return out };
    let (Ok(np), Ok(po)) = (rd(b, 4), rd(b, 8)) else { return out };
    for i in 0..np as usize {
        let at = po as usize + i * 12;
        if let (Ok(PARAM_CBV), Ok(vis), Ok(pl)) = (rd(b, at), rd(b, at + 4), rd(b, at + 8)) {
            if let (Ok(r), Ok(sp)) = (rd(b, pl as usize), rd(b, pl as usize + 4)) {
                out.push((i, r, sp, vis));
            }
        }
    }
    out
}

fn log_root_regs(blob: &[u8]) {
    let regs = root_cbv_regs(blob);
    if regs.is_empty() {
        return;
    }
    static SEEN: std::sync::Mutex<Vec<Vec<(usize, u32, u32)>>> = std::sync::Mutex::new(Vec::new());
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !g.contains(&regs) && g.len() < 64 {
        record("d3d12_xs", "cbv_raw_check", &format!("root signature root CBV params (param, register b, space): {regs:?}"));
        g.push(regs);
    }
}

#[derive(Default)]
struct DrawCheck {
    rs: std::collections::HashMap<usize, Vec<(usize, u32, u32, u32)>>,
    lists: std::collections::HashMap<usize, (usize, u64, std::collections::HashMap<u32, u64>)>,
    last: std::collections::HashMap<usize, std::collections::HashMap<u32, u64>>,
    seen: std::collections::HashSet<(usize, u64)>,
}

static DC: std::sync::Mutex<Option<DrawCheck>> = std::sync::Mutex::new(None);
static PREV_SET_RS: AtomicUsize = AtomicUsize::new(0);
static PREV_DRAW_IDX: AtomicUsize = AtomicUsize::new(0);
static PREV_DRAW: AtomicUsize = AtomicUsize::new(0);
static PREV_RESET: AtomicUsize = AtomicUsize::new(0);
static INHERITED: AtomicUsize = AtomicUsize::new(0);
const XBOX_SLOT_RESET: usize = 10;
const XBOX_SLOT_SET_GRAPHICS_RS: usize = 34;
const XBOX_SLOT_DRAW: usize = 12;
const XBOX_SLOT_DRAW_INDEXED: usize = 13;

fn dc_with<R>(f: impl FnOnce(&mut DrawCheck) -> R) -> R {
    let mut g = DC.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(DrawCheck::default))
}

unsafe extern "system" fn set_graphics_rs_checked(this: *mut c_void, rs: *mut c_void) {
    dc_with(|d| {
        let l = d.lists.entry(this as usize).or_default();
        if l.0 != rs as usize {
            *l = (rs as usize, 0, std::collections::HashMap::new());
        }
    });
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void) = unsafe { core::mem::transmute(PREV_SET_RS.load(Ordering::Relaxed)) };
    unsafe { f(this, rs) }
}

fn inherit_on() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| enabled() && (check_on() || std::env::var("TOMBOX_CBRAW_INHERIT").map(|v| v == "1").unwrap_or(false)))
}

fn track_on() -> bool {
    inherit_on() || crate::hang_cap::on()
}

unsafe extern "system" fn reset_checked(this: *mut c_void, alloc: *mut c_void, pso: *mut c_void) -> i32 {
    dc_with(|d| {
        d.lists.remove(&(this as usize));
        d.last.remove(&(this as usize));
    });
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32 = unsafe { core::mem::transmute(PREV_RESET.load(Ordering::Relaxed)) };
    unsafe { f(this, alloc, pso) }
}

pub fn inherit_plan(regs: &[(usize, u32, u32)], mask: u64, last: &std::collections::HashMap<u32, u64>) -> Vec<(u32, u64)> {
    regs.iter()
        .filter(|(i, _, _)| *i < 64 && mask & (1u64 << i) == 0)
        .filter_map(|(i, _, _)| last.get(&(*i as u32)).map(|va| (*i as u32, *va)))
        .collect()
}

fn check_draw(list: usize, text: &dyn Fn() -> String) {
    let (plan, msg) = dc_with(|d| {
        let (rs, mask, vas) = d.lists.get(&list).cloned().unwrap_or_default();
        let Some(params) = d.rs.get(&rs).cloned() else { return (Vec::new(), None) };
        if crate::hang_cap::on() {
            crate::hang_cap::draw(list, &format!("rs {rs:#x} {}", text()), &params, mask, &vas);
        }
        let regs: Vec<(usize, u32, u32)> = params.iter().map(|&(i, r, sp, _)| (i, r, sp)).collect();
        let plan = if inherit_on() { d.last.get(&list).map(|last| inherit_plan(&regs, mask, last)).unwrap_or_default() } else { Vec::new() };
        if let Some(l) = d.lists.get_mut(&list) {
            for (i, va) in &plan {
                l.1 |= 1u64 << i;
                l.2.insert(*i, *va);
            }
        }
        if !check_on() {
            return (plan, None);
        }
        let unset: Vec<String> = regs.iter().filter(|(i, _, _)| *i >= 64 || mask & (1u64 << i) == 0).map(|(i, r, sp)| {
            let inh = plan.iter().find(|(p, _)| *p as usize == *i).map(|(_, va)| format!(" inherited {va:#x}")).unwrap_or_else(|| " never set on this list".to_string());
            format!("param {i} (b{r} space{sp}){inh}")
        }).collect();
        if unset.is_empty() {
            return (plan, None);
        }
        let key = (rs, mask);
        if d.seen.len() >= 64 || !d.seen.insert(key) {
            return (plan, None);
        }
        (plan, Some(format!("draw on list {list:#x} with root signature {rs:#x}: root CBVs not set since SetGraphicsRootSignature: [{}]", unset.join(", "))))
    });
    if !plan.is_empty() {
        let f: unsafe extern "system" fn(*mut c_void, u32, u64) = unsafe { core::mem::transmute(REAL_ROOT_SRV[1].load(Ordering::Relaxed)) };
        for (i, va) in &plan {
            unsafe { f(list as *mut c_void, *i, *va) };
        }
        let n = INHERITED.fetch_add(plan.len(), Ordering::Relaxed);
        if n == 0 {
            record("d3d12_xs", "cbv_raw", "a draw used a root signature whose root CBV was not set since SetGraphicsRootSignature; the value last set at that parameter index on this list is re-applied, as console user-data registers keep it (TOMBOX_CBRAW_INHERIT=1)");
        }
    }
    if let Some(m) = msg {
        record("d3d12_xs", "cbv_raw_check", &m);
    }
}

unsafe extern "system" fn draw_indexed_checked(this: *mut c_void, a: u32, b: u32, c: u32, d: i32, e: u32) {
    check_draw(this as usize, &|| format!("DrawIndexedInstanced(idx={a} inst={b} startIdx={c} baseVtx={d} startInst={e})"));
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, u32, i32, u32) = unsafe { core::mem::transmute(PREV_DRAW_IDX.load(Ordering::Relaxed)) };
    unsafe { f(this, a, b, c, d, e) }
}

unsafe extern "system" fn draw_checked(this: *mut c_void, a: u32, b: u32, c: u32, d: u32) {
    check_draw(this as usize, &|| format!("DrawInstanced(vtx={a} inst={b} startVtx={c} startInst={d})"));
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, u32, u32) = unsafe { core::mem::transmute(PREV_DRAW.load(Ordering::Relaxed)) };
    unsafe { f(this, a, b, c, d) }
}

pub fn install_draw_check(xbox_vt: &[AtomicUsize]) {
    if !track_on() || xbox_vt.len() <= XBOX_SLOT_SET_GRAPHICS_RS || REAL_ROOT_SRV[1].load(Ordering::Relaxed) == 0 {
        return;
    }
    PREV_RESET.store(xbox_vt[XBOX_SLOT_RESET].load(Ordering::Relaxed), Ordering::Relaxed);
    xbox_vt[XBOX_SLOT_RESET].store(reset_checked as usize, Ordering::Relaxed);
    PREV_SET_RS.store(xbox_vt[XBOX_SLOT_SET_GRAPHICS_RS].load(Ordering::Relaxed), Ordering::Relaxed);
    PREV_DRAW_IDX.store(xbox_vt[XBOX_SLOT_DRAW_INDEXED].load(Ordering::Relaxed), Ordering::Relaxed);
    PREV_DRAW.store(xbox_vt[XBOX_SLOT_DRAW].load(Ordering::Relaxed), Ordering::Relaxed);
    xbox_vt[XBOX_SLOT_SET_GRAPHICS_RS].store(set_graphics_rs_checked as usize, Ordering::Relaxed);
    xbox_vt[XBOX_SLOT_DRAW_INDEXED].store(draw_indexed_checked as usize, Ordering::Relaxed);
    xbox_vt[XBOX_SLOT_DRAW].store(draw_checked as usize, Ordering::Relaxed);
    crate::hang_cap::install_cmdlist(xbox_vt);
    record("d3d12_xs", "cbv_raw", if inherit_on() { "root CBV inheritance across root signature changes installed" } else { "root CBV tracking installed for the hang capture (no inheritance)" });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_va::Entry;

    #[test]
    fn clamp_knob_takes_an_optional_shader_scope() {
        assert_eq!(parse_clamp("13:z"), Some((13, u32::MAX, None)));
        assert_eq!(parse_clamp("13:0x470"), Some((13, 0x470, None)));
        assert_eq!(parse_clamp("13:z:f861c56a34758d84"), Some((13, u32::MAX, Some(0xf861c56a34758d84))));
        assert_eq!(parse_clamp("13:z:nothex"), None);
    }

    fn rts0(version: u32, ranges: &[[u32; 4]], root_cbv: bool) -> Vec<u8> {
        let rs = if version == 1 { 20 } else { 24 };
        let np = 1 + root_cbv as usize;
        let mut b = vec![0u8; 24 + np * 12 + 8 + ranges.len() * rs + 12];
        let w = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        w(&mut b, 0, version);
        w(&mut b, 4, np as u32);
        w(&mut b, 8, 24);
        let table = 24 + np * 12;
        w(&mut b, 24, 0);
        w(&mut b, 32, table as u32);
        w(&mut b, table, ranges.len() as u32);
        w(&mut b, table + 4, (table + 8) as u32);
        for (k, r) in ranges.iter().enumerate() {
            let at = table + 8 + k * rs;
            for (i, v) in r.iter().enumerate() {
                w(&mut b, at + i * 4, *v);
            }
            w(&mut b, at + rs - 4, 0xffff_ffff);
        }
        if root_cbv {
            let d = table + 8 + ranges.len() * rs;
            w(&mut b, 36, 2);
            w(&mut b, 44, d as u32);
        }
        b
    }

    #[test]
    fn cbv_raw_mode_setting_is_validated() {
        assert_eq!(parse_mode(None), Ok(Mode::Auto));
        assert_eq!(parse_mode(Some("")), Ok(Mode::Auto));
        assert_eq!(parse_mode(Some("auto")), Ok(Mode::Auto));
        assert_eq!(parse_mode(Some(" ON ")), Ok(Mode::On));
        assert_eq!(parse_mode(Some("1")), Ok(Mode::On));
        assert_eq!(parse_mode(Some("off")), Ok(Mode::Off));
        assert_eq!(parse_mode(Some("0")), Ok(Mode::Off));
        assert!(parse_mode(Some("yes")).is_err());
    }

    #[test]
    fn only_xbox_one_titles_require_durango_programs() {
        assert_eq!(shader_kind_refusal(true, true), None);
        assert!(shader_kind_refusal(true, false).is_some());
        assert_eq!(shader_kind_refusal(false, false), None);
        assert_eq!(shader_kind_refusal(false, true), None);
    }

    #[test]
    fn rts0_cbv_ranges_become_raw_srv_ranges() {
        for v in [1, 2] {
            let mut b = rts0(v, &[[2, 3, 0, 0], [0, 2, 0, 0], [2, 1, 4, 1], [3, 1, 0, 0]], false);
            let p = patch_rts0(&mut b).unwrap();
            assert_eq!(p, RsPatch { ranges: 2, root_cbvs: vec![], aliased: false });
            let rs = if v == 1 { 20 } else { 24 };
            let base = 24 + 12 + 8;
            let r = |k: usize, i: usize| u32::from_le_bytes(b[base + k * rs + i * 4..base + k * rs + i * 4 + 4].try_into().unwrap());
            assert_eq!((r(0, 0), r(0, 1), r(0, 2), r(0, 3)), (0, 3, 0, TB_CBRAW_SPACE));
            assert_eq!((r(1, 0), r(1, 3)), (0, 0));
            assert_eq!((r(2, 0), r(2, 2), r(2, 3)), (0, 4, TB_CBRAW_SPACE + 1));
            assert_eq!(r(3, 0), 3);
            assert_eq!(r(0, (rs - 4) / 4), 0xffff_ffff);
        }
        let mut b = rts0(2, &[[2, 1, 0, 0]], true);
        assert_eq!(patch_rts0(&mut b).unwrap().root_cbvs, vec![1]);
        let d = u32::from_le_bytes(b[44..48].try_into().unwrap()) as usize;
        assert_eq!(u32::from_le_bytes(b[36..40].try_into().unwrap()), PARAM_SRV);
        assert_eq!(u32::from_le_bytes(b[d + 4..d + 8].try_into().unwrap()), TB_CBRAW_SPACE);
        let b = rts0(2, &[], true);
        assert!(patch_blob(&b).unwrap().is_some());
        let mut b = rts0(2, &[[2, 1, 0, 100_000]], false);
        assert!(patch_rts0(&mut b).is_err());
        let b = rts0(2, &[[3, 1, 0, 0]], false);
        assert_eq!(patch_blob(&b).unwrap(), None);
    }

    #[test]
    fn unset_root_cbvs_inherit_the_value_last_set_at_their_index() {
        let regs = vec![(0usize, 1u32, 0u32), (5, 13, 0), (6, 13, 0)];
        let last: std::collections::HashMap<u32, u64> = [(0u32, 0x1000u64), (5, 0x2000), (6, 0x3000), (7, 0x4000)].into_iter().collect();
        assert_eq!(inherit_plan(&regs, 1 << 0 | 1 << 5, &last), vec![(6, 0x3000)]);
        assert_eq!(inherit_plan(&regs, 0, &last), vec![(0, 0x1000), (5, 0x2000), (6, 0x3000)]);
        assert!(inherit_plan(&regs, 1 | 1 << 5 | 1 << 6, &last).is_empty());
        let partial: std::collections::HashMap<u32, u64> = [(5u32, 0x2000u64)].into_iter().collect();
        assert_eq!(inherit_plan(&regs, 0, &partial), vec![(5, 0x2000)]);
    }

    #[test]
    fn raw_view_starts_at_the_exact_console_address() {
        let e = Entry { start: 0xbe2_0000, len: 0x1000, res: 0x5550, buffer: true };
        let look = |a: u64| (a >= e.start && a < e.start + e.len).then(|| (e, a - e.start));
        let (res, d) = raw_view(0xbe200b0, 368, &look).unwrap();
        assert_eq!(res, 0x5550);
        assert_eq!(u32::from_le_bytes(d[0..4].try_into().unwrap()), DXGI_R32_TYPELESS);
        assert_eq!(u32::from_le_bytes(d[4..8].try_into().unwrap()), SRV_DIMENSION_BUFFER);
        assert_eq!(u64::from_le_bytes(d[16..24].try_into().unwrap()), 0xb0 / 4);
        assert_eq!(u32::from_le_bytes(d[24..28].try_into().unwrap()), 92);
        assert_eq!(u32::from_le_bytes(d[32..36].try_into().unwrap()), BUFFER_SRV_FLAG_RAW);
        let (_, d) = raw_view(0xbe20ff0, 368, &look).unwrap();
        assert_eq!(u32::from_le_bytes(d[24..28].try_into().unwrap()), 4);
        assert_eq!(raw_view(0, 0, &look).unwrap().0, 0);
        assert!(raw_view(0xbe200b4, 16, &look).unwrap_err().contains("16-byte"));
        assert!(raw_view(0x10, 16, &look).unwrap_err().contains("not inside"));
        let t = Entry { buffer: false, ..e };
        assert!(raw_view(0xbe20010, 16, &|a| Some((t, a - t.start))).unwrap_err().contains("texture"));
    }

    #[test]
    fn rewritten_dump_compiles() {
        let Ok(path) = std::env::var("TOMBOX_CBRAW_DXBC") else { return };
        assert!(tombox_xshader::dxc::available(), "TOMBOX_CBRAW_DXBC is set but dxcompiler.dll + dxil.dll are not found");
        let blob = std::fs::read(&path).unwrap();
        let ex = tombox_xshader::extract(&blob).unwrap();
        assert!(tombox_xshader::gcn::is_durango(&ex.fssm));
        let t = tombox_xshader::hlsl::translate(&ex).unwrap();
        let h = rewrite_hlsl(&t.hlsl).unwrap();
        assert!(h.contains("ByteAddressBuffer tb_cbr"), "{h}");
        let h = tombox_xshader::runtime_hlsl::finalize(&h, &[0; 8], true);
        let prof = tombox_xshader::dxc::profile_for(t.profile);
        let r = tombox_xshader::dxc::compile(&h, &prof);
        assert!(r.is_ok(), "{:?}", r.err());
    }

    #[test]
    fn hlsl_constant_buffers_become_byte_address_loads() {
        let h = "cbuffer CB0 : register(b0) { uint4 cb0[4]; };\ncbuffer CB65537 : register(b1, space1) { uint4 cb65537[21]; };\ncbuffer TbGmem : register(b0, space4000001) { uint4 tb_gmem_base; };\nvoid f() {\n    s0 = cb0[0].x;\n    uint a = cb65537[((t1 + 2u) >> 2) & 4095u][(t1 + 2u) & 3u];\n    uint b = cb0[cb0[1].y][2];\n    uint cb01 = 0u;\n}\n";
        let o = rewrite_hlsl(h).unwrap();
        assert!(o.contains("ByteAddressBuffer tb_cbr0 : register(t0, space4100000);"), "{o}");
        assert!(o.contains("ByteAddressBuffer tb_cbr65537 : register(t1, space4100001);"), "{o}");
        assert!(o.contains("cbuffer TbGmem : register(b0, space4000001)"), "{o}");
        assert!(o.contains("s0 = tb_cbr0.Load4((0) * 16u).x;"), "{o}");
        assert!(o.contains("uint a = tb_cbr65537.Load4((((t1 + 2u) >> 2) & 4095u) * 16u)[(t1 + 2u) & 3u];"), "{o}");
        assert!(o.contains("uint b = tb_cbr0.Load4((tb_cbr0.Load4((1) * 16u).y) * 16u)[2];"), "{o}");
        assert!(o.contains("uint cb01 = 0u;"), "{o}");
        assert!(!o.contains("cbuffer CB"), "{o}");
        assert_eq!(rewrite_hlsl("void f() {}\n").unwrap(), "void f() {}\n");
        assert!(rewrite_hlsl("struct TbBlCb5 { uint4 v[2]; };\nConstantBuffer<TbBlCb5> cb5_ : register(b0);\n").is_err());
        assert!(rewrite_hlsl("cbuffer CB0 : register(b0) { uint4 cb0[4]; };\nuint4 x = cb0;\n").is_err());
    }
}
