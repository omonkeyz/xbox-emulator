
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::collections::HashMap;
use std::sync::Mutex;
use compat_common::record;

pub const R9G9B9E5_SHAREDEXP: u32 = 67;
pub const R16G16B16A16_FLOAT: u32 = 10;
const FLAG_ALLOW_UAV: u32 = 0x4;
const FEATURE_FORMAT_SUPPORT: u32 = 3;
pub const SUPPORT1_TYPED_UAV: u32 = 0x0200_0000;
const DEV_CHECK_FEATURE_SUPPORT: usize = 13;
const DEV_CREATE_SRV: usize = 18;
const DEV_CREATE_UAV: usize = 19;
const DEV_CREATE_RTV: usize = 20;
const RES_GET_DESC: usize = 10;
const HEAP_GET_DESC: usize = 8;
const CL_COPY_TEXTURE: usize = 16;
const CL_COPY_RESOURCE: usize = 17;
const CL_COPY_TEXTURE_X: usize = 83;
const CL_COPY_RESOURCE_X: usize = 84;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subst {
    pub console: u32,
    pub host: u32,
}

pub const TABLE: [Subst; 1] = [Subst { console: R9G9B9E5_SHAREDEXP, host: R16G16B16A16_FLOAT }];

impl Subst {
    pub fn console_bpe(&self) -> usize {
        4
    }

    pub fn host_bpe(&self) -> usize {
        8
    }

    pub fn widen(&self, src: &[u8], dst: &mut [u8]) {
        let v = rgb9e5_to_rgba16f(u32::from_le_bytes(src[..4].try_into().unwrap()));
        dst[..8].copy_from_slice(&v.to_le_bytes());
    }
}

pub fn entry(console: u32) -> Option<Subst> {
    TABLE.iter().copied().find(|s| s.console == console)
}

fn index(console: u32) -> Option<usize> {
    TABLE.iter().position(|s| s.console == console)
}

pub fn decide(dimension: u32, format: u32, flags: u32, host_support1: Option<u32>) -> Option<Subst> {
    if dimension < 2 || flags & FLAG_ALLOW_UAV == 0 {
        return None;
    }
    let s = entry(format)?;
    match host_support1 {
        Some(bits) if bits & SUPPORT1_TYPED_UAV == 0 => Some(s),
        _ => None,
    }
}


const UNKNOWN: u32 = u32::MAX;
static SUPPORT1: [AtomicU32; TABLE.len()] = [const { AtomicU32::new(UNKNOWN) }; TABLE.len()];

fn cached_support1(format: u32) -> Option<u32> {
    let v = SUPPORT1[index(format)?].load(Ordering::Relaxed);
    (v != UNKNOWN).then_some(v)
}

pub unsafe fn host_support1(real: *mut c_void, format: u32) -> Option<u32> {
    let i = index(format)?;
    if real.is_null() {
        return cached_support1(format);
    }
    let mut data = [format, 0, 0];
    let f: unsafe extern "system" fn(*mut c_void, u32, *mut u32, u32) -> i32 = unsafe { core::mem::transmute(slot(real, DEV_CHECK_FEATURE_SUPPORT)) };
    if unsafe { f(real, FEATURE_FORMAT_SUPPORT, data.as_mut_ptr(), 12) } < 0 {
        return cached_support1(format);
    }
    SUPPORT1[i].store(data[1], Ordering::Relaxed);
    Some(data[1])
}

pub unsafe fn probe(real: *mut c_void) {
    for s in TABLE {
        let bits = unsafe { host_support1(real, s.console) };
        let active = bits.is_some_and(|b| b & SUPPORT1_TYPED_UAV == 0);
        record("d3d12_xs", "uav_subst", &format!(
            "format {} support1={} -> {}", s.console,
            bits.map_or("unknown".to_string(), |b| format!("{b:#x}")),
            if active { format!("no typed UAV on this host; ALLOW_UNORDERED_ACCESS textures are stored as format {}", s.host) } else { "native".to_string() }));
    }
}

pub unsafe fn substitute_desc(real: *mut c_void, desc: &mut [u8]) -> Option<Subst> {
    let format = rd32(desc, 0x20);
    entry(format)?;
    let s = decide(rd32(desc, 0), format, rd32(desc, 0x30), unsafe { host_support1(real, format) })?;
    desc[0x20..0x24].copy_from_slice(&s.host.to_le_bytes());
    Some(s)
}

pub fn heap_is_gpu_only(heap_type: u32, cpu_page: u32) -> bool {
    heap_type == 1 || (heap_type == 4 && cpu_page == 1)
}

pub unsafe fn heap_object_is_gpu_only(heap: *mut c_void) -> bool {
    if heap.is_null() {
        return false;
    }
    let mut d = [0u8; 0x30];
    let f: unsafe extern "system" fn(*mut c_void, *mut u8) -> *mut u8 = unsafe { core::mem::transmute(slot(heap, HEAP_GET_DESC)) };
    unsafe { f(heap, d.as_mut_ptr()) };
    heap_is_gpu_only(rd32(&d, 8), rd32(&d, 0xc))
}

pub fn note_cpu_heap(what: &str, desc: &[u8]) {
    static N: AtomicUsize = AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 8 {
        record("d3d12_xs", what, &format!(
            "format {} ALLOW_UNORDERED_ACCESS texture in a CPU-visible heap: not substituted (CPU access would see {}-byte texels); the host rejects it",
            rd32(desc, 0x20), entry(rd32(desc, 0x20)).map_or(0, |s| s.host_bpe())));
    }
}

pub fn note_substituted(what: &str, s: Subst, desc: &[u8]) {
    static N: AtomicUsize = AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 16 {
        record("d3d12_xs", what, &format!(
            "format {} {}x{} ALLOW_UNORDERED_ACCESS texture stored as format {} (host has no typed UAV for {})",
            s.console, u64::from_le_bytes(desc[0x10..0x18].try_into().unwrap()), rd32(desc, 0x18), s.host, s.console));
    }
}


static LIVE: AtomicUsize = AtomicUsize::new(0);
static REG: Mutex<Option<HashMap<usize, Subst>>> = Mutex::new(None);

pub unsafe fn register(real: *mut c_void, res: *mut c_void, s: Subst) {
    if res.is_null() {
        return;
    }
    if REG.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(res as usize, s).is_none() {
        LIVE.fetch_add(1, Ordering::AcqRel);
    }
    unsafe { hook_views(real) };
}

pub unsafe fn lookup(res: *mut c_void) -> Option<Subst> {
    if LIVE.load(Ordering::Acquire) == 0 || res.is_null() {
        return None;
    }
    let s = REG.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&(res as usize)).copied()?;
    let mut d = [0u8; 0x38];
    let f: unsafe extern "system" fn(*mut c_void, *mut u8) -> *mut u8 = unsafe { core::mem::transmute(slot(res, RES_GET_DESC)) };
    unsafe { f(res, d.as_mut_ptr()) };
    if rd32(&d, 0x20) == s.host && rd32(&d, 0x30) & FLAG_ALLOW_UAV != 0 {
        return Some(s);
    }
    if REG.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&(res as usize))).is_some() {
        LIVE.fetch_sub(1, Ordering::AcqRel);
    }
    None
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Srv,
    Uav,
    Rtv,
}

impl View {
    pub fn desc_len(self) -> usize {
        match self {
            View::Srv | View::Uav => 40,
            View::Rtv => 24,
        }
    }
}

pub fn force_alpha_one(mapping: u32) -> u32 {
    let mut m = mapping;
    for j in 0..4 {
        let sh = 3 * j;
        if (m >> sh) & 7 == 3 {
            m = (m & !(7 << sh)) | (5 << sh);
        }
    }
    m
}

pub fn rewrite_view(kind: View, desc: &mut [u8], s: Subst) -> bool {
    if rd32(desc, 0) != s.console {
        return false;
    }
    desc[0..4].copy_from_slice(&s.host.to_le_bytes());
    if kind == View::Srv {
        let m = force_alpha_one(rd32(desc, 8));
        desc[8..12].copy_from_slice(&m.to_le_bytes());
    }
    true
}

unsafe fn fixed_view(kind: View, res: *mut c_void, pdesc: *const u8) -> Option<[u8; 40]> {
    if pdesc.is_null() || res.is_null() || LIVE.load(Ordering::Acquire) == 0 {
        return None;
    }
    let fmt = unsafe { core::ptr::read_unaligned(pdesc as *const u32) };
    entry(fmt)?;
    let s = unsafe { lookup(res) }?;
    let mut d = [0u8; 40];
    unsafe { core::ptr::copy_nonoverlapping(pdesc, d.as_mut_ptr(), kind.desc_len()) };
    rewrite_view(kind, &mut d, s).then_some(d)
}

const MAX_VT: usize = 4;
static HOOK_VT: [AtomicUsize; MAX_VT] = [const { AtomicUsize::new(0) }; MAX_VT];
static HOOK_PREV: [[AtomicUsize; 3]; MAX_VT] = [const { [const { AtomicUsize::new(0) }; 3] }; MAX_VT];
static HOOK_LOCK: Mutex<()> = Mutex::new(());

fn prev(dev: *mut c_void, k: usize) -> usize {
    let vt = unsafe { *(dev as *const usize) };
    HOOK_VT.iter().position(|v| v.load(Ordering::Acquire) == vt).map_or(0, |i| HOOK_PREV[i][k].load(Ordering::Acquire))
}

unsafe extern "system" fn srv_hook(dev: *mut c_void, res: *mut c_void, pdesc: *const u8, dest: usize) {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(prev(dev, 0)) };
    match unsafe { fixed_view(View::Srv, res, pdesc) } {
        Some(d) => unsafe { f(dev, res, d.as_ptr(), dest) },
        None => unsafe { f(dev, res, pdesc, dest) },
    }
}

unsafe extern "system" fn uav_hook(dev: *mut c_void, res: *mut c_void, counter: *mut c_void, pdesc: *const u8, dest: usize) {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(prev(dev, 1)) };
    match unsafe { fixed_view(View::Uav, res, pdesc) } {
        Some(d) => unsafe { f(dev, res, counter, d.as_ptr(), dest) },
        None => unsafe { f(dev, res, counter, pdesc, dest) },
    }
}

unsafe extern "system" fn rtv_hook(dev: *mut c_void, res: *mut c_void, pdesc: *const u8, dest: usize) {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(prev(dev, 2)) };
    match unsafe { fixed_view(View::Rtv, res, pdesc) } {
        Some(d) => unsafe { f(dev, res, d.as_ptr(), dest) },
        None => unsafe { f(dev, res, pdesc, dest) },
    }
}

unsafe fn hook_views(real: *mut c_void) {
    extern "system" {
        fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old_protect: *mut u32) -> i32;
    }
    if real.is_null() {
        return;
    }
    let vt = unsafe { *(real as *const *mut usize) };
    let _g = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if HOOK_VT.iter().any(|v| v.load(Ordering::Acquire) == vt as usize) {
        return;
    }
    let Some(i) = HOOK_VT.iter().position(|v| v.load(Ordering::Acquire) == 0) else {
        record("d3d12_xs", "uav_subst", "more than 4 device method tables; views of substituted textures on this device are not rewritten");
        return;
    };
    let mut old = 0u32;
    if unsafe { VirtualProtect(vt.add(DEV_CREATE_SRV) as *mut c_void, 3 * 8, 0x04, &mut old) } == 0 {
        record("d3d12_xs", "uav_subst", "could not unprotect the device method table; views of substituted textures are not rewritten");
        return;
    }
    let hooks = [(DEV_CREATE_SRV, srv_hook as usize), (DEV_CREATE_UAV, uav_hook as usize), (DEV_CREATE_RTV, rtv_hook as usize)];
    for (k, (s, _)) in hooks.iter().enumerate() {
        HOOK_PREV[i][k].store(unsafe { *vt.add(*s) }, Ordering::Release);
    }
    HOOK_VT[i].store(vt as usize, Ordering::Release);
    for (s, f) in hooks {
        unsafe { *vt.add(s) = f };
    }
    let mut tmp = 0u32;
    unsafe { VirtualProtect(vt.add(DEV_CREATE_SRV) as *mut c_void, 3 * 8, old, &mut tmp) };
    record("d3d12_xs", "uav_subst", &format!("view creation hooked on device method table {vt:p}"));
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Texture(Option<Subst>),
    Footprint(u32),
}

pub fn copy_conflict(dst: Side, src: Side) -> Option<&'static str> {
    use Side::*;
    match (dst, src) {
        (Texture(Some(a)), Texture(Some(b))) if a == b => None,
        (Texture(Some(_)), Texture(_)) | (Texture(_), Texture(Some(_))) => {
            Some("between a substituted texture and one that is not: their texels differ in format and size")
        }
        (Texture(Some(s)), Footprint(f)) | (Footprint(f), Texture(Some(s))) if f != s.host => {
            Some("between a substituted texture and a buffer footprint in the console format: the console layout has 4-byte texels, the host texture 8-byte ones; no conversion is implemented")
        }
        _ => None,
    }
}

unsafe fn side(loc: *const u8) -> Side {
    let res = unsafe { core::ptr::read_unaligned(loc as *const usize) } as *mut c_void;
    if unsafe { core::ptr::read_unaligned(loc.add(8) as *const u32) } == 1 {
        Side::Footprint(unsafe { core::ptr::read_unaligned(loc.add(0x18) as *const u32) })
    } else {
        Side::Texture(unsafe { lookup(res) })
    }
}

fn refuse(what: &str, why: &str) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    if n < 32 || n.is_power_of_two() {
        record("d3d12_xs", "uav_subst", &format!("{what} REFUSED (#{n}): copy {why}"));
    }
}

static PREV_CL: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

fn cl_prev(k: usize) -> usize {
    PREV_CL[k].load(Ordering::Acquire)
}

unsafe fn texture_conflict(dst: *const u8, src: *const u8) -> Option<&'static str> {
    if LIVE.load(Ordering::Acquire) == 0 || dst.is_null() || src.is_null() {
        return None;
    }
    copy_conflict(unsafe { side(dst) }, unsafe { side(src) })
}

unsafe fn resource_conflict(dst: *mut c_void, src: *mut c_void) -> Option<&'static str> {
    if LIVE.load(Ordering::Acquire) == 0 {
        return None;
    }
    copy_conflict(Side::Texture(unsafe { lookup(dst) }), Side::Texture(unsafe { lookup(src) }))
}

unsafe extern "system" fn copy_texture(this: *mut c_void, dst: *const u8, x: u32, y: u32, z: u32, src: *const u8, src_box: *const c_void) {
    if let Some(why) = unsafe { texture_conflict(dst, src) } {
        return refuse("CopyTextureRegion", why);
    }
    let f: unsafe extern "system" fn(*mut c_void, *const u8, u32, u32, u32, *const u8, *const c_void) = unsafe { core::mem::transmute(cl_prev(0)) };
    unsafe { f(this, dst, x, y, z, src, src_box) }
}

unsafe extern "system" fn copy_resource(this: *mut c_void, dst: *mut c_void, src: *mut c_void) {
    if let Some(why) = unsafe { resource_conflict(dst, src) } {
        return refuse("CopyResource", why);
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) = unsafe { core::mem::transmute(cl_prev(1)) };
    unsafe { f(this, dst, src) }
}

unsafe extern "system" fn copy_texture_x(this: *mut c_void, dst: *const u8, x: u32, y: u32, z: u32, src: *const u8, src_box: *const c_void, flags: u32) {
    if let Some(why) = unsafe { texture_conflict(dst, src) } {
        return refuse("CopyTextureRegionX", why);
    }
    let f: unsafe extern "system" fn(*mut c_void, *const u8, u32, u32, u32, *const u8, *const c_void, u32) = unsafe { core::mem::transmute(cl_prev(2)) };
    unsafe { f(this, dst, x, y, z, src, src_box, flags) }
}

unsafe extern "system" fn copy_resource_x(this: *mut c_void, dst: *mut c_void, src: *mut c_void, flags: u32) {
    if let Some(why) = unsafe { resource_conflict(dst, src) } {
        return refuse("CopyResourceX", why);
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, u32) = unsafe { core::mem::transmute(cl_prev(3)) };
    unsafe { f(this, dst, src, flags) }
}

pub fn install_cmdlist(vt: &[AtomicUsize]) -> usize {
    let hooks = [
        (CL_COPY_TEXTURE, copy_texture as usize),
        (CL_COPY_RESOURCE, copy_resource as usize),
        (CL_COPY_TEXTURE_X, copy_texture_x as usize),
        (CL_COPY_RESOURCE_X, copy_resource_x as usize),
    ];
    let mut n = 0;
    for (k, (s, f)) in hooks.into_iter().enumerate() {
        let p = match vt.get(s) {
            Some(p) => p.load(Ordering::Acquire),
            None => continue,
        };
        if p == 0 || p == f {
            continue;
        }
        PREV_CL[k].store(p, Ordering::Release);
        vt[s].store(f, Ordering::Release);
        n += 1;
    }
    n
}


pub fn rgb9e5_channel(mantissa: u32, exponent: u32) -> f32 {
    mantissa as f32 * 2f32.powi(exponent as i32 - 24)
}

pub fn rgb9e5_decode(v: u32) -> [f32; 3] {
    let e = v >> 27;
    [rgb9e5_channel(v & 0x1ff, e), rgb9e5_channel((v >> 9) & 0x1ff, e), rgb9e5_channel((v >> 18) & 0x1ff, e)]
}

pub fn f32_to_f16(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rest = m & ((1 << shift) - 1);
        let mut h = m >> shift;
        if rest > half || (rest == half && h & 1 == 1) {
            h += 1;
        }
        return sign | h as u16;
    }
    let mut h = ((e as u32) << 10) | (man >> 13);
    let rest = man & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && h & 1 == 1) {
        h += 1;
    }
    sign | h as u16
}

pub fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    sign * match e {
        0 => m * 2f32.powi(-24),
        0x1f => if m == 0.0 { f32::INFINITY } else { f32::NAN },
        _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

pub fn rgb9e5_to_rgba16f(v: u32) -> u64 {
    let [r, g, b] = rgb9e5_decode(v);
    f32_to_f16(r) as u64 | (f32_to_f16(g) as u64) << 16 | (f32_to_f16(b) as u64) << 32 | 0x3c00u64 << 48
}

pub fn rgb9e5_encode(c: [f32; 3]) -> u32 {
    const MAX: f32 = 65408.0;
    let c = c.map(|x| if x.is_nan() || x <= 0.0 { 0.0 } else { x.min(MAX) });
    let max = c[0].max(c[1]).max(c[2]);
    if max == 0.0 {
        return 0;
    }
    let mut e = (max.log2().floor() as i32).max(-16) + 1 + 15;
    let mut step = 2f32.powi(e - 24);
    if (max / step + 0.5).floor() as u32 == 512 {
        e += 1;
        step *= 2.0;
    }
    let e = e.clamp(0, 31) as u32;
    let m = c.map(|x| ((x / step + 0.5).floor() as u32).min(511));
    m[0] | m[1] << 9 | m[2] << 18 | e << 27
}

pub fn rgb9e5_step(c: [f32; 3]) -> f32 {
    2f32.powi((rgb9e5_encode(c) >> 27) as i32 - 24)
}


fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

unsafe fn slot(obj: *mut c_void, i: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(i) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGB9E5: Subst = TABLE[0];

    #[test]
    fn decision_table() {
        let none = 0x8013f0; // RTX 5090 / UHD 770 Support1 for R9G9B9E5
        let warp = 0x2ecd3f1; // WARP Support1 for R9G9B9E5
        assert_eq!(decide(3, 67, 4, Some(none)), Some(RGB9E5));
        assert_eq!(decide(4, 67, 4 | 0x1000, Some(none)), Some(RGB9E5)); // 3D, extra flags
        assert_eq!(decide(3, 67, 4, Some(warp)), None); // host binds it natively
        assert_eq!(decide(3, 67, 0, Some(none)), None); // no UAV flag: host supports SRV
        assert_eq!(decide(1, 67, 4, Some(none)), None); // buffer
        assert_eq!(decide(3, 67, 4, None), None); // unknown host support never substitutes
        for fmt in [10, 28, 85, 86, 115, 191] {
            assert_eq!(decide(3, fmt, 4, Some(0)), None, "format {fmt} has no substitute");
        }
    }

    #[test]
    fn desc_substitution_uses_the_cached_probe() {
        SUPPORT1[0].store(0x8013f0, Ordering::Relaxed);
        let mut d = [0u8; 0x38];
        d[0] = 3;
        d[0x20] = 67;
        d[0x30] = 4;
        assert_eq!(unsafe { substitute_desc(core::ptr::null_mut(), &mut d) }, Some(RGB9E5));
        assert_eq!(rd32(&d, 0x20), 10);
        let mut plain = [0u8; 0x38];
        plain[0] = 3;
        plain[0x20] = 28;
        plain[0x30] = 4;
        assert_eq!(unsafe { substitute_desc(core::ptr::null_mut(), &mut plain) }, None);
        assert_eq!(rd32(&plain, 0x20), 28);
    }

    #[test]
    fn heap_kinds() {
        assert!(heap_is_gpu_only(1, 0));
        assert!(heap_is_gpu_only(4, 1));
        assert!(!heap_is_gpu_only(4, 3));
        assert!(!heap_is_gpu_only(2, 0));
        assert!(!heap_is_gpu_only(3, 0));
        assert!(!heap_is_gpu_only(5, 0));
    }

    #[test]
    fn view_rewrite() {
        let mut srv = [0u8; 40];
        srv[0] = 67;
        srv[4] = 4;
        srv[8..12].copy_from_slice(&0x1688u32.to_le_bytes());
        assert!(rewrite_view(View::Srv, &mut srv, RGB9E5));
        assert_eq!(rd32(&srv, 0), 10);
        assert_eq!(rd32(&srv, 8), 0x1a88); // R,G,B kept; A forced to 1
        assert_eq!(rd32(&srv, 4), 4);
        let mut uav = [0u8; 40];
        uav[0] = 67;
        uav[4] = 4;
        uav[8] = 2;
        assert!(rewrite_view(View::Uav, &mut uav, RGB9E5));
        assert_eq!((rd32(&uav, 0), rd32(&uav, 4), rd32(&uav, 8)), (10, 4, 2));
        let mut unknown = [0u8; 40];
        unknown[4] = 4;
        assert!(!rewrite_view(View::Uav, &mut unknown, RGB9E5));
        let mut other = [0u8; 24];
        other[0] = 2;
        assert!(!rewrite_view(View::Rtv, &mut other, RGB9E5));
        assert_eq!(rd32(&other, 0), 2);
    }

    #[test]
    fn alpha_forcing_keeps_other_selectors() {
        assert_eq!(force_alpha_one(0x1688), 0x1a88);
        let all_alpha = 3 | 3 << 3 | 3 << 6 | 3 << 9 | 1 << 12;
        assert_eq!(force_alpha_one(all_alpha), 5 | 5 << 3 | 5 << 6 | 5 << 9 | 1 << 12);
        let consts = 4 | 5 << 3 | 0 << 6 | 1 << 9 | 1 << 12;
        assert_eq!(force_alpha_one(consts), consts);
    }

    #[test]
    fn copy_rules() {
        use Side::*;
        let s = Some(RGB9E5);
        assert_eq!(copy_conflict(Texture(None), Texture(None)), None);
        assert_eq!(copy_conflict(Texture(None), Footprint(67)), None);
        assert_eq!(copy_conflict(Texture(s), Texture(s)), None);
        assert!(copy_conflict(Texture(s), Texture(None)).is_some());
        assert!(copy_conflict(Texture(None), Texture(s)).is_some());
        assert!(copy_conflict(Texture(s), Footprint(67)).is_some());
        assert!(copy_conflict(Footprint(67), Texture(s)).is_some());
        assert_eq!(copy_conflict(Footprint(10), Texture(s)), None);
        assert_eq!(copy_conflict(Texture(s), Footprint(10)), None);
    }

    #[test]
    fn rgb9e5_channels_are_exact_f16() {
        for e in 0..32 {
            for m in 0..512 {
                let v = rgb9e5_channel(m, e);
                assert_eq!(f16_to_f32(f32_to_f16(v)), v, "m={m} e={e}");
            }
        }
        assert_eq!(rgb9e5_channel(511, 31), 65408.0);
    }

    #[test]
    fn widen_texels() {
        let s = RGB9E5;
        let v: u32 = 0x1ff | 0x100 << 9 | 1 << 18 | 15 << 27; // e=15: m * 2^-9
        let mut out = [0u8; 8];
        s.widen(&v.to_le_bytes(), &mut out);
        let h = |i: usize| f16_to_f32(u16::from_le_bytes([out[2 * i], out[2 * i + 1]]));
        assert_eq!([h(0), h(1), h(2), h(3)], [511.0 / 512.0, 0.5, 1.0 / 512.0, 1.0]);
        assert_eq!(rgb9e5_to_rgba16f(0), 0x3c00u64 << 48);
        assert_eq!(rgb9e5_decode(0x1ff | 31 << 27), [65408.0, 0.0, 0.0]);
    }

    #[test]
    fn reference_encoding() {
        assert_eq!(rgb9e5_decode(rgb9e5_encode([1.0, 0.5, 0.25])), [1.0, 0.5, 0.25]);
        assert_eq!(rgb9e5_decode(rgb9e5_encode([-1.0, f32::NAN, 1e9])), [0.0, 0.0, 65408.0]);
        let q = rgb9e5_decode(rgb9e5_encode([1000.0, 1.3, 0.0]));
        assert_eq!(q[0], 1000.0);
        assert_eq!(q[1], 2.0);
        assert_eq!(rgb9e5_step([1000.0, 1.3, 0.0]), 2.0);
        let top = rgb9e5_encode([1.0 - 1.0 / 2048.0, 0.0, 0.0]);
        assert_eq!(rgb9e5_decode(top)[0], 1.0);
        for x in [0.0f32, 1e-8, 3.0e-5, 0.1, 0.7, 1.0, 3.14159, 1234.5, 60000.0] {
            assert!((rgb9e5_decode(rgb9e5_encode([x, 0.0, 0.0]))[0] - x).abs() <= rgb9e5_step([x, 0.0, 0.0]) / 2.0, "{x}");
        }
    }

    #[test]
    fn f16_rounding() {
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert_eq!(f32_to_f16(65504.0), 0x7bff);
        assert_eq!(f32_to_f16(65520.0), 0x7c00);
        assert_eq!(f32_to_f16(2f32.powi(-24)), 1);
        assert_eq!(f32_to_f16(2f32.powi(-25)), 0); // tie to even
        assert_eq!(f32_to_f16(1.0 + 2f32.powi(-11)), 0x3c00); // tie to even
        assert_eq!(f32_to_f16(1.0 + 3.0 * 2f32.powi(-11)), 0x3c02);
        assert_eq!(f32_to_f16(-2.0), 0xc000);
    }
}
