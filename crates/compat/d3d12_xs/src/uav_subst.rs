use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use compat_common::record;

use crate::device_wrap::Guid;

pub const R9G9B9E5_SHAREDEXP: u32 = 67;
pub const R16G16B16A16_FLOAT: u32 = 10;
pub const SUPPORT1_TYPED_UAV: u32 = 0x0200_0000;
const FLAG_ALLOW_RENDER_TARGET: u32 = 0x1;
const FLAG_ALLOW_DEPTH_STENCIL: u32 = 0x2;
const FLAG_ALLOW_UAV: u32 = 0x4;
const FEATURE_FORMAT_SUPPORT: u32 = 3;
const HEAP_DEFAULT: u32 = 1;
const HEAP_CUSTOM: u32 = 4;
const CPU_PAGE_NOT_AVAILABLE: u32 = 1;
const DEFAULT_SRV_MAPPING: u32 = 0x1688;
const E_FAIL: i32 = 0x8000_4005u32 as i32;

const OBJ_RELEASE: usize = 2;
const OBJ_GET_PRIVATE_DATA: usize = 3;
const OBJ_SET_PRIVATE_DATA: usize = 4;
const DEV_CHECK_FEATURE_SUPPORT: usize = 13;
const DEV_CREATE_SRV: usize = 18;
const DEV_CREATE_UAV: usize = 19;
const DEV_CREATE_RTV: usize = 20;
const DEV_CREATE_COMMITTED: usize = 27;
const RES_GET_DESC: usize = 10;
const HEAP_GET_DESC: usize = 8;
const CL_COPY_TEXTURE: usize = 16;
const CL_COPY_RESOURCE: usize = 17;
const CL_COPY_TEXTURE_X: usize = 83;
const CL_COPY_RESOURCE_X: usize = 84;

static IID_RESOURCE: Guid = Guid { d1: 0x6964_42be, d2: 0xa72e, d3: 0x4059, d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad] };
static MARK: Guid = Guid { d1: 0xd0fe_71cf, d2: 0x2a8c, d3: 0x4fc6, d4: [0xa4, 0xfa, 0x24, 0xd4, 0x6d, 0x45, 0x77, 0x9e] };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subst {
    pub console: u32,
    pub host: u32,
}

pub const TABLE: [Subst; 1] = [Subst { console: R9G9B9E5_SHAREDEXP, host: R16G16B16A16_FLOAT }];

pub fn entry(console: u32) -> Option<Subst> {
    TABLE.iter().copied().find(|s| s.console == console)
}

pub fn decide(desc: &[u8], host_support1: Option<u32>) -> Option<Subst> {
    let (dim, flags, samples) = (rd32(desc, 0), rd32(desc, 0x30), rd32(desc, 0x24));
    if !(2..=4).contains(&dim) || flags & FLAG_ALLOW_UAV == 0 || flags & (FLAG_ALLOW_RENDER_TARGET | FLAG_ALLOW_DEPTH_STENCIL) != 0 || samples > 1 {
        return None;
    }
    let s = entry(rd32(desc, 0x20))?;
    match host_support1 {
        Some(bits) if bits & SUPPORT1_TYPED_UAV == 0 => Some(s),
        _ => None,
    }
}

pub unsafe fn host_support1(real: *mut c_void, format: u32) -> Option<u32> {
    if real.is_null() {
        return None;
    }
    let mut data = [format, 0, 0];
    let f: unsafe extern "system" fn(*mut c_void, u32, *mut u32, u32) -> i32 = unsafe { core::mem::transmute(slot(real, DEV_CHECK_FEATURE_SUPPORT)) };
    (unsafe { f(real, FEATURE_FORMAT_SUPPORT, data.as_mut_ptr(), 12) } >= 0).then_some(masked(data[1]))
}

#[cfg(test)]
pub static FORCE_NO_TYPED_UAV: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn masked(bits: u32) -> u32 {
    #[cfg(test)]
    if FORCE_NO_TYPED_UAV.load(Ordering::Relaxed) {
        return bits & !SUPPORT1_TYPED_UAV;
    }
    bits
}

pub fn heap_is_gpu_only(heap_type: u32, cpu_page: u32) -> bool {
    heap_type == HEAP_DEFAULT || (heap_type == HEAP_CUSTOM && cpu_page == CPU_PAGE_NOT_AVAILABLE)
}

unsafe fn heap_desc(heap: *mut c_void) -> [u8; 0x30] {
    let mut d = [0u8; 0x30];
    let f: unsafe extern "system" fn(*mut c_void, *mut u8) -> *mut u8 = unsafe { core::mem::transmute(slot(heap, HEAP_GET_DESC)) };
    unsafe { f(heap, d.as_mut_ptr()) };
    d
}

pub unsafe fn heap_object_is_gpu_only(heap: *mut c_void) -> bool {
    if heap.is_null() {
        return false;
    }
    let d = unsafe { heap_desc(heap) };
    heap_is_gpu_only(rd32(&d, 8), rd32(&d, 0xc))
}

pub unsafe fn substitute(real: *mut c_void, desc: &mut [u8], gpu_only_heap: bool, what: &str) -> Option<Subst> {
    decide(desc, Some(0))?;
    let support = unsafe { host_support1(real, rd32(desc, 0x20)) };
    let s = decide(desc, support)?;
    static N: AtomicUsize = AtomicUsize::new(0);
    let log = N.fetch_add(1, Ordering::Relaxed) < 16;
    let size = format!("{}x{}x{}", u64::from_le_bytes(desc[0x10..0x18].try_into().unwrap()), rd32(desc, 0x18), u16::from_le_bytes([desc[0x1c], desc[0x1d]]));
    if !gpu_only_heap {
        if log {
            record("d3d12_xs", what, &format!("format {} {size} ALLOW_UNORDERED_ACCESS texture in a CPU-visible heap: not stored as format {}, because CPU access would see its 8-byte texels; created as the title asked (host Support1={:#x}, no typed UAV)", s.console, s.host, support.unwrap_or(0)));
        }
        return None;
    }
    if log {
        record("d3d12_xs", what, &format!("format {} {size} ALLOW_UNORDERED_ACCESS texture stored as format {} (host Support1={:#x}, no typed UAV for format {})", s.console, s.host, support.unwrap_or(0), s.console));
    }
    desc[0x20..0x24].copy_from_slice(&s.host.to_le_bytes());
    Some(s)
}

pub unsafe fn committed_for_heap(real: *mut c_void, heap: *mut c_void, offset: u64, desc: &[u8], init_state: u32, pclear: *const c_void, ppv: *mut *mut c_void) -> i32 {
    let hd = unsafe { heap_desc(heap) };
    let props: [u32; 5] = core::array::from_fn(|i| rd32(&hd, 8 + i * 4));
    let f: unsafe extern "system" fn(*mut c_void, *const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(slot(real, DEV_CREATE_COMMITTED)) };
    let hr = unsafe { f(real, props.as_ptr(), 0, desc.as_ptr(), init_state, pclear, &IID_RESOURCE, ppv) };
    static N: AtomicUsize = AtomicUsize::new(0);
    if hr < 0 || N.fetch_add(1, Ordering::Relaxed) < 8 {
        record("d3d12_xs", "CreatePlacedResource", &format!(
            "heap {heap:p}+{offset:#x}: the texture is stored as format {} and outgrows the console layout the title reserved there; created committed on heap type {}, not in the title heap -> hr={hr:#010x}",
            rd32(desc, 0x20), props[0]));
    }
    hr
}

static SUBSTITUTED: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn finish(real: *mut c_void, hr: i32, ppv: *mut *mut c_void, s: Option<Subst>) -> i32 {
    let Some(s) = s else { return hr };
    if hr < 0 || ppv.is_null() || unsafe { *ppv }.is_null() {
        return hr;
    }
    let res = unsafe { *ppv };
    if let Err(why) = unsafe { register(real, res, s) } {
        record("d3d12_xs", "uav_subst", &format!("format {} texture {res:p} stored as format {}: {why}; creation refused (E_FAIL) so no view of it reads the wrong format", s.console, s.host));
        let release: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(slot(res, OBJ_RELEASE)) };
        unsafe { release(res) };
        unsafe { *ppv = core::ptr::null_mut() };
        return E_FAIL;
    }
    hr
}

unsafe fn register(real: *mut c_void, res: *mut c_void, s: Subst) -> Result<(), &'static str> {
    let i = TABLE.iter().position(|t| *t == s).ok_or("not a table entry")? as u32;
    unsafe { hook_views(real) }?;
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, u32, *const c_void) -> i32 = unsafe { core::mem::transmute(slot(res, OBJ_SET_PRIVATE_DATA)) };
    if unsafe { f(res, &MARK, 4, &i as *const u32 as *const c_void) } < 0 {
        return Err("SetPrivateData failed");
    }
    SUBSTITUTED.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

pub unsafe fn lookup(res: *mut c_void) -> Option<Subst> {
    if res.is_null() || SUBSTITUTED.load(Ordering::Acquire) == 0 {
        return None;
    }
    let mut i = 0u32;
    let mut n = 4u32;
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, *mut u32, *mut c_void) -> i32 = unsafe { core::mem::transmute(slot(res, OBJ_GET_PRIVATE_DATA)) };
    if unsafe { f(res, &MARK, &mut n, &mut i as *mut u32 as *mut c_void) } < 0 || n != 4 {
        return None;
    }
    TABLE.get(i as usize).copied()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Srv,
    Uav,
    Rtv,
}

impl View {
    fn desc_len(self) -> usize {
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
    let fmt = rd32(desc, 0);
    if fmt != s.console && !(kind == View::Srv && fmt == 0) {
        return false;
    }
    desc[0..4].copy_from_slice(&s.host.to_le_bytes());
    if kind == View::Srv {
        let m = force_alpha_one(rd32(desc, 8));
        desc[8..12].copy_from_slice(&m.to_le_bytes());
    }
    true
}

pub fn default_srv(res_desc: &[u8]) -> Option<[u8; 40]> {
    let (dim, array) = (rd32(res_desc, 0), u16::from_le_bytes([res_desc[0x1c], res_desc[0x1d]]) as u32);
    let mut d = [0u8; 40];
    let mut put = |o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
    put(8, DEFAULT_SRV_MAPPING);
    put(20, u32::MAX);
    match (dim, array) {
        (2, 0 | 1) => put(4, 2),
        (2, n) => {
            put(4, 3);
            put(28, n);
        }
        (3, 0 | 1) => put(4, 4),
        (3, n) => {
            put(4, 5);
            put(28, n);
        }
        (4, _) => put(4, 8),
        _ => return None,
    }
    Some(d)
}

unsafe fn fixed_view(kind: View, res: *mut c_void, pdesc: *const u8) -> Option<[u8; 40]> {
    if res.is_null() {
        return None;
    }
    let fmt = if pdesc.is_null() { None } else { Some(unsafe { core::ptr::read_unaligned(pdesc as *const u32) }) };
    let wanted = match (kind, fmt) {
        (View::Srv, None | Some(0)) => true,
        (_, Some(f)) => entry(f).is_some(),
        (_, None) => false,
    };
    if !wanted {
        return None;
    }
    let s = unsafe { lookup(res) }?;
    let mut d = if pdesc.is_null() {
        let mut rd = [0u8; 0x38];
        let f: unsafe extern "system" fn(*mut c_void, *mut u8) -> *mut u8 = unsafe { core::mem::transmute(slot(res, RES_GET_DESC)) };
        unsafe { f(res, rd.as_mut_ptr()) };
        default_srv(&rd)?
    } else {
        let mut d = [0u8; 40];
        unsafe { core::ptr::copy_nonoverlapping(pdesc, d.as_mut_ptr(), kind.desc_len()) };
        d
    };
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

#[cfg(test)]
pub fn unhooked(dev: *mut c_void, kind: View) -> Option<usize> {
    let p = prev(dev, kind as usize);
    (p != 0).then_some(p)
}

unsafe extern "system" fn srv_hook(dev: *mut c_void, res: *mut c_void, pdesc: *const u8, dest: usize) {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(prev(dev, View::Srv as usize)) };
    match unsafe { fixed_view(View::Srv, res, pdesc) } {
        Some(d) => unsafe { f(dev, res, d.as_ptr(), dest) },
        None => unsafe { f(dev, res, pdesc, dest) },
    }
}

unsafe extern "system" fn uav_hook(dev: *mut c_void, res: *mut c_void, counter: *mut c_void, pdesc: *const u8, dest: usize) {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(prev(dev, View::Uav as usize)) };
    match unsafe { fixed_view(View::Uav, res, pdesc) } {
        Some(d) => unsafe { f(dev, res, counter, d.as_ptr(), dest) },
        None => unsafe { f(dev, res, counter, pdesc, dest) },
    }
}

unsafe extern "system" fn rtv_hook(dev: *mut c_void, res: *mut c_void, pdesc: *const u8, dest: usize) {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(prev(dev, View::Rtv as usize)) };
    match unsafe { fixed_view(View::Rtv, res, pdesc) } {
        Some(d) => unsafe { f(dev, res, d.as_ptr(), dest) },
        None => unsafe { f(dev, res, pdesc, dest) },
    }
}

unsafe fn hook_views(real: *mut c_void) -> Result<(), &'static str> {
    extern "system" {
        fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old_protect: *mut u32) -> i32;
    }
    if real.is_null() {
        return Err("no host device to hook view creation on");
    }
    let vt = unsafe { *(real as *const *mut usize) };
    let _g = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if HOOK_VT.iter().any(|v| v.load(Ordering::Acquire) == vt as usize) {
        return Ok(());
    }
    let Some(i) = HOOK_VT.iter().position(|v| v.load(Ordering::Acquire) == 0) else {
        return Err("more than 4 device method tables, view creation is not hooked on this one");
    };
    let mut old = 0u32;
    if unsafe { VirtualProtect(vt.add(DEV_CREATE_SRV) as *mut c_void, 3 * 8, 0x04, &mut old) } == 0 {
        return Err("the device method table could not be unprotected, view creation is not hooked");
    }
    let hooks = [(DEV_CREATE_SRV, srv_hook as *const () as usize), (DEV_CREATE_UAV, uav_hook as *const () as usize), (DEV_CREATE_RTV, rtv_hook as *const () as usize)];
    for (k, (s, _)) in hooks.iter().enumerate() {
        HOOK_PREV[i][k].store(unsafe { *vt.add(*s) }, Ordering::Release);
    }
    HOOK_VT[i].store(vt as usize, Ordering::Release);
    for (s, f) in hooks {
        unsafe { *vt.add(s) = f };
    }
    let mut tmp = 0u32;
    unsafe { VirtualProtect(vt.add(DEV_CREATE_SRV) as *mut c_void, 3 * 8, old, &mut tmp) };
    record("d3d12_xs", "uav_subst", &format!("SRV/UAV/RTV creation hooked on device method table {vt:p}"));
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Encode,
    Decode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Texture { subst: Option<Subst>, format: u32 },
    Footprint(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Forward,
    Convert(Dir, Subst),
    Refuse(&'static str),
}

fn format_of_side(side: Side) -> u32 {
    match side {
        Side::Texture { format, .. } | Side::Footprint(format) => format,
    }
}

pub fn copy_plan(dst: Side, src: Side) -> Plan {
    match (dst, src) {
        (Side::Texture { subst: Some(a), .. }, Side::Texture { subst: Some(b), .. }) if a == b => Plan::Forward,
        (Side::Texture { subst: Some(_), .. }, Side::Texture { subst: Some(_), .. }) => Plan::Refuse("copies between textures stored in different substitute formats"),
        (other, Side::Texture { subst: Some(s), .. }) if format_of_side(other) == s.console => Plan::Convert(Dir::Encode, s),
        (_, Side::Texture { subst: Some(_), .. }) => Plan::Refuse("the destination format is neither the substituted texture's console format nor a texture with the same substitute"),
        (Side::Texture { subst: Some(s), .. }, other) if format_of_side(other) == s.console => Plan::Convert(Dir::Decode, s),
        (Side::Texture { subst: Some(_), .. }, _) => Plan::Refuse("the source format is neither the substituted texture's console format nor a texture with the same substitute"),
        _ => Plan::Forward,
    }
}

pub fn resource_plan(dst: Option<Subst>, dd: &[u8; 0x38], src: Option<Subst>, sd: &[u8; 0x38]) -> Plan {
    let (d, s) = (crate::console_slots_cl2::Desc::from_bytes(dd), crate::console_slots_cl2::Desc::from_bytes(sd));
    let (dir, sub, other) = match (dst, src) {
        (None, None) => return Plan::Forward,
        (Some(a), Some(b)) if a == b => return Plan::Forward,
        (Some(_), Some(_)) => return Plan::Refuse("copies between textures stored in different substitute formats"),
        (None, Some(x)) => (Dir::Encode, x, d),
        (Some(x), None) => (Dir::Decode, x, s),
    };
    if other.dim < 2 {
        return Plan::Refuse("CopyResource between a texture and a buffer");
    }
    if other.format != sub.console {
        return Plan::Refuse("the other resource's format is not the substituted texture's console format");
    }
    if (d.dim, d.width, d.height, d.depth_or_array, d.mips) != (s.dim, s.width, s.height, s.depth_or_array, s.mips) {
        return Plan::Refuse("the two resources differ in dimension, size, array size or mip count");
    }
    Plan::Convert(dir, sub)
}

unsafe fn side(loc: *const u8) -> (Side, *mut c_void) {
    let res = unsafe { core::ptr::read_unaligned(loc as *const usize) } as *mut c_void;
    if unsafe { core::ptr::read_unaligned(loc.add(8) as *const u32) } == 1 {
        return (Side::Footprint(unsafe { core::ptr::read_unaligned(loc.add(0x18) as *const u32) }), res);
    }
    (Side::Texture { subst: unsafe { lookup(res) }, format: 0 }, res)
}

unsafe fn with_format(side: Side, res: *mut c_void) -> Side {
    match side {
        Side::Texture { subst: None, .. } if !res.is_null() => Side::Texture { subst: None, format: rd32(&unsafe { crate::uav_subst_copy::res_desc(res) }, 0x20) },
        other => other,
    }
}

static REFUSED: AtomicUsize = AtomicUsize::new(0);
static CONVERTED: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub fn refused_copies() -> usize {
    REFUSED.load(Ordering::Relaxed)
}

#[cfg(test)]
pub fn converted_copies() -> usize {
    CONVERTED.load(Ordering::Relaxed)
}

fn refuse(what: &str, why: &str) {
    let n = REFUSED.fetch_add(1, Ordering::Relaxed);
    if n < 32 || n.is_power_of_two() {
        record("d3d12_xs", "uav_subst", &format!("{what} REFUSED (#{n}), nothing copied: {why}"));
    }
}

fn converted(what: &str, dir: Dir, s: Subst) {
    let n = CONVERTED.fetch_add(1, Ordering::Relaxed);
    if n < 8 || n.is_power_of_two() {
        let (a, b) = if dir == Dir::Encode { (s.host, s.console) } else { (s.console, s.host) };
        record("d3d12_xs", "uav_subst", &format!("{what} (#{n}) converted format {a} texels to format {b} through a compute pass"));
    }
}

static PREV_CL: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

fn cl_prev(k: usize) -> usize {
    PREV_CL[k].load(Ordering::Acquire)
}

unsafe fn texture_plan(dst: *const u8, src: *const u8) -> (Plan, *mut c_void) {
    if SUBSTITUTED.load(Ordering::Acquire) == 0 || dst.is_null() || src.is_null() {
        return (Plan::Forward, core::ptr::null_mut());
    }
    let ((d, dres), (s, sres)) = (unsafe { side(dst) }, unsafe { side(src) });
    if !matches!(d, Side::Texture { subst: Some(_), .. }) && !matches!(s, Side::Texture { subst: Some(_), .. }) {
        return (Plan::Forward, sres);
    }
    (copy_plan(unsafe { with_format(d, dres) }, unsafe { with_format(s, sres) }), sres)
}

fn rd_box(src_box: *const c_void) -> Option<[u32; 6]> {
    (!src_box.is_null()).then(|| unsafe { core::ptr::read_unaligned(src_box as *const [u32; 6]) })
}

unsafe fn read_loc(p: *const u8) -> [u8; 0x30] {
    unsafe { core::ptr::read_unaligned(p as *const [u8; 0x30]) }
}

pub fn region_of(src: &[u8; 0x30], src_desc: Option<&[u8; 0x38]>, src_box: Option<[u32; 6]>, at: [u32; 3]) -> crate::uav_subst_copy::Region {
    let full = match src_desc {
        Some(d) => crate::uav_subst_copy::subresource_size(d, rd32(src, 0x10)),
        None => [rd32(src, 0x1c), rd32(src, 0x20), rd32(src, 0x24)],
    };
    let size = match src_box {
        Some(b) => [b[3].saturating_sub(b[0]), b[4].saturating_sub(b[1]), b[5].saturating_sub(b[2])],
        None => full,
    };
    crate::uav_subst_copy::Region { at, size }
}

pub fn end_of(loc: &[u8; 0x30], origin: [u32; 3], bpp: u64) -> crate::uav_subst_copy::End {
    if rd32(loc, 8) != 1 {
        return crate::uav_subst_copy::End::Tex(*loc);
    }
    let pitch = rd32(loc, 0x28) as u64;
    let slice = pitch * rd32(loc, 0x20) as u64;
    let offset = u64::from_le_bytes(loc[0x10..0x18].try_into().unwrap()) + origin[2] as u64 * slice + origin[1] as u64 * pitch + origin[0] as u64 * bpp;
    crate::uav_subst_copy::End::Buf { res: u64::from_le_bytes(loc[0..8].try_into().unwrap()) as usize, offset, pitch, slice }
}

unsafe fn convert_region(list: *mut c_void, dir: Dir, s: Subst, dst: *const u8, at: [u32; 3], src: *const u8, src_res: *mut c_void, src_box: *const c_void) -> Result<(), String> {
    let (dl, sl) = (unsafe { read_loc(dst) }, unsafe { read_loc(src) });
    let b = rd_box(src_box);
    let src_is_texture = rd32(&sl, 8) != 1;
    let src_desc = src_is_texture.then(|| unsafe { crate::uav_subst_copy::res_desc(src_res) });
    let r = region_of(&sl, src_desc.as_ref(), b, at);
    let bin = crate::uav_subst_copy::bytes_per_texel(dir, true);
    let bout = crate::uav_subst_copy::bytes_per_texel(dir, false);
    let src_end = end_of(&sl, if src_is_texture { [0; 3] } else { b.map_or([0; 3], |b| [b[0], b[1], b[2]]) }, bin);
    let dst_end = end_of(&dl, at, bout);
    unsafe { crate::uav_subst_copy::convert(list, dir, s, src_end, dst_end, r, if src_is_texture { b } else { None }) }
}

unsafe fn texture_copy(list: *mut c_void, what: &str, dst: *const u8, at: [u32; 3], src: *const u8, src_box: *const c_void) -> bool {
    let (plan, sres) = unsafe { texture_plan(dst, src) };
    match plan {
        Plan::Forward => false,
        Plan::Refuse(why) => {
            refuse(what, why);
            true
        }
        Plan::Convert(dir, s) => {
            match unsafe { convert_region(list, dir, s, dst, at, src, sres, src_box) } {
                Ok(()) => converted(what, dir, s),
                Err(why) => refuse(what, &why),
            }
            true
        }
    }
}

unsafe fn resource_copy(list: *mut c_void, what: &str, dst: *mut c_void, src: *mut c_void, flags: Option<u32>) -> bool {
    if SUBSTITUTED.load(Ordering::Acquire) == 0 || dst.is_null() || src.is_null() {
        return false;
    }
    let (ds, ss) = (unsafe { lookup(dst) }, unsafe { lookup(src) });
    if ds.is_none() && ss.is_none() {
        return false;
    }
    let (dd, sd) = (unsafe { crate::uav_subst_copy::res_desc(dst) }, unsafe { crate::uav_subst_copy::res_desc(src) });
    let console = |d: &[u8; 0x38]| Some(crate::console_slots_cl2::Desc::from_bytes(d));
    let plan = match (resource_plan(ds, &dd, ss, &sd), flags) {
        (Plan::Convert(dir, s), Some(f)) => match crate::console_slots_cl2::copy_resource_x_plan(console(&dd), console(&sd), false, f) {
            Err(why) => Plan::Refuse(why),
            Ok(_) => Plan::Convert(dir, s),
        },
        (p, _) => p,
    };
    match plan {
        Plan::Forward => false,
        Plan::Refuse(why) => {
            refuse(what, why);
            true
        }
        Plan::Convert(dir, s) => {
            let subs = crate::console_slots_cl2::Desc::from_bytes(&dd).subresources();
            for sub in 0..subs {
                let (dl, sl) = (crate::uav_subst_copy::subresource(dst as usize, sub), crate::uav_subst_copy::subresource(src as usize, sub));
                let r = crate::uav_subst_copy::Region { at: [0; 3], size: crate::uav_subst_copy::subresource_size(&sd, sub) };
                let (se, de) = (crate::uav_subst_copy::End::Tex(sl), crate::uav_subst_copy::End::Tex(dl));
                if let Err(why) = unsafe { crate::uav_subst_copy::convert(list, dir, s, se, de, r, None) } {
                    refuse(what, &format!("subresource {sub} of {subs} (earlier subresources were converted): {why}"));
                    return true;
                }
            }
            converted(what, dir, s);
            true
        }
    }
}

unsafe extern "system" fn copy_texture(this: *mut c_void, dst: *const u8, x: u32, y: u32, z: u32, src: *const u8, src_box: *const c_void) {
    if unsafe { texture_copy(this, "CopyTextureRegion", dst, [x, y, z], src, src_box) } {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, *const u8, u32, u32, u32, *const u8, *const c_void) = unsafe { core::mem::transmute(cl_prev(0)) };
    unsafe { f(this, dst, x, y, z, src, src_box) }
}

unsafe extern "system" fn copy_resource(this: *mut c_void, dst: *mut c_void, src: *mut c_void) {
    if unsafe { resource_copy(this, "CopyResource", dst, src, None) } {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) = unsafe { core::mem::transmute(cl_prev(1)) };
    unsafe { f(this, dst, src) }
}

unsafe extern "system" fn copy_texture_x(this: *mut c_void, dst: *const u8, x: u32, y: u32, z: u32, src: *const u8, src_box: *const c_void, flags: u32) {
    if unsafe { texture_copy(this, "CopyTextureRegionX", dst, [x, y, z], src, src_box) } {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, *const u8, u32, u32, u32, *const u8, *const c_void, u32) = unsafe { core::mem::transmute(cl_prev(2)) };
    unsafe { f(this, dst, x, y, z, src, src_box, flags) }
}

unsafe extern "system" fn copy_resource_x(this: *mut c_void, dst: *mut c_void, src: *mut c_void, flags: u32) {
    if unsafe { resource_copy(this, "CopyResourceX", dst, src, Some(flags)) } {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, u32) = unsafe { core::mem::transmute(cl_prev(3)) };
    unsafe { f(this, dst, src, flags) }
}

pub fn install_cmdlist(vt: &[AtomicUsize]) -> usize {
    let hooks = [
        (CL_COPY_TEXTURE, copy_texture as *const () as usize),
        (CL_COPY_RESOURCE, copy_resource as *const () as usize),
        (CL_COPY_TEXTURE_X, copy_texture_x as *const () as usize),
        (CL_COPY_RESOURCE_X, copy_resource_x as *const () as usize),
    ];
    let mut n = 0;
    for (k, (s, f)) in hooks.into_iter().enumerate() {
        let Some(p) = vt.get(s).map(|p| p.load(Ordering::Acquire)) else { continue };
        if p == 0 || p == f {
            continue;
        }
        PREV_CL[k].store(p, Ordering::Release);
        vt[s].store(f, Ordering::Release);
        n += 1;
    }
    n
}

pub fn rgb9e5_half(m: u32, e: u32) -> u16 {
    if m == 0 {
        return 0;
    }
    let p = 31 - m.leading_zeros();
    if p + e < 10 {
        return (m << e) as u16;
    }
    (((p + e - 9) << 10) | ((m << (10 - p)) & 0x3ff)) as u16
}

pub fn rgb9e5_to_rgba16f(v: u32) -> [u16; 4] {
    let e = v >> 27;
    [rgb9e5_half(v & 0x1ff, e), rgb9e5_half((v >> 9) & 0x1ff, e), rgb9e5_half((v >> 18) & 0x1ff, e), 0x3c00]
}

#[cfg(test)]
pub fn rgb9e5_channel(mantissa: u32, exponent: u32) -> f32 {
    mantissa as f32 * 2f32.powi(exponent as i32 - 24)
}

#[cfg(test)]
pub fn rgb9e5_decode(v: u32) -> [f32; 3] {
    let e = v >> 27;
    [rgb9e5_channel(v & 0x1ff, e), rgb9e5_channel((v >> 9) & 0x1ff, e), rgb9e5_channel((v >> 18) & 0x1ff, e)]
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

    fn desc(dim: u32, format: u32, flags: u32, samples: u32) -> [u8; 0x38] {
        let mut d = [0u8; 0x38];
        d[0..4].copy_from_slice(&dim.to_le_bytes());
        d[0x10..0x18].copy_from_slice(&1920u64.to_le_bytes());
        d[0x18..0x1c].copy_from_slice(&1080u32.to_le_bytes());
        d[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
        d[0x20..0x24].copy_from_slice(&format.to_le_bytes());
        d[0x24..0x28].copy_from_slice(&samples.to_le_bytes());
        d[0x30..0x34].copy_from_slice(&flags.to_le_bytes());
        d
    }

    #[test]
    fn decision_table() {
        let rtx_5090_and_uhd_770 = 0x8013f0;
        let warp = 0x2ecd3f1;
        assert_eq!(decide(&desc(3, 67, 4, 1), Some(rtx_5090_and_uhd_770)), Some(RGB9E5));
        assert_eq!(decide(&desc(4, 67, 4 | 0x20, 1), Some(rtx_5090_and_uhd_770)), Some(RGB9E5));
        assert_eq!(decide(&desc(2, 67, 4, 0), Some(rtx_5090_and_uhd_770)), Some(RGB9E5));
        assert_eq!(decide(&desc(3, 67, 4, 1), Some(warp)), None);
        assert_eq!(decide(&desc(3, 67, 0, 1), Some(rtx_5090_and_uhd_770)), None);
        assert_eq!(decide(&desc(1, 67, 4, 1), Some(rtx_5090_and_uhd_770)), None);
        assert_eq!(decide(&desc(3, 67, 4 | 1, 1), Some(rtx_5090_and_uhd_770)), None);
        assert_eq!(decide(&desc(3, 67, 4 | 2, 1), Some(rtx_5090_and_uhd_770)), None);
        assert_eq!(decide(&desc(3, 67, 4, 4), Some(rtx_5090_and_uhd_770)), None);
        assert_eq!(decide(&desc(3, 67, 4, 1), None), None);
        for fmt in [10, 28, 85, 86, 115, 191] {
            assert_eq!(decide(&desc(3, fmt, 4, 1), Some(0)), None, "format {fmt} has no substitute");
        }
    }

    #[test]
    fn substitution_needs_a_host_answer_and_a_gpu_only_heap() {
        let mut d = desc(3, 67, 4, 1);
        assert_eq!(unsafe { substitute(core::ptr::null_mut(), &mut d, true, "test") }, None);
        assert_eq!(rd32(&d, 0x20), 67);
        let mut plain = desc(3, 28, 4, 1);
        assert_eq!(unsafe { substitute(core::ptr::null_mut(), &mut plain, true, "test") }, None);
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
        assert_eq!(rd32(&srv, 8), 0x1a88);
        assert_eq!(rd32(&srv, 4), 4);
        let mut unknown_srv = [0u8; 40];
        unknown_srv[4] = 4;
        unknown_srv[8..12].copy_from_slice(&0x1688u32.to_le_bytes());
        assert!(rewrite_view(View::Srv, &mut unknown_srv, RGB9E5));
        assert_eq!((rd32(&unknown_srv, 0), rd32(&unknown_srv, 8)), (10, 0x1a88));
        let mut uav = [0u8; 40];
        uav[0] = 67;
        uav[4] = 4;
        uav[8] = 2;
        assert!(rewrite_view(View::Uav, &mut uav, RGB9E5));
        assert_eq!((rd32(&uav, 0), rd32(&uav, 4), rd32(&uav, 8)), (10, 4, 2));
        let mut unknown_uav = [0u8; 40];
        unknown_uav[4] = 4;
        assert!(!rewrite_view(View::Uav, &mut unknown_uav, RGB9E5));
        let mut host = [0u8; 40];
        host[0] = 10;
        host[8..12].copy_from_slice(&0x1688u32.to_le_bytes());
        assert!(!rewrite_view(View::Srv, &mut host, RGB9E5));
        assert_eq!(rd32(&host, 8), 0x1688);
        let mut other = [0u8; 24];
        other[0] = 2;
        assert!(!rewrite_view(View::Rtv, &mut other, RGB9E5));
        assert_eq!(rd32(&other, 0), 2);
    }

    #[test]
    fn default_srv_covers_the_whole_resource() {
        let view = |dim: u32, array: u16| {
            let mut d = desc(dim, 10, 4, 1);
            d[0x1c..0x1e].copy_from_slice(&array.to_le_bytes());
            default_srv(&d).map(|v| (rd32(&v, 0), rd32(&v, 4), rd32(&v, 8), rd32(&v, 16), rd32(&v, 20), rd32(&v, 24), rd32(&v, 28)))
        };
        assert_eq!(view(3, 1), Some((0, 4, 0x1688, 0, u32::MAX, 0, 0)));
        assert_eq!(view(3, 6), Some((0, 5, 0x1688, 0, u32::MAX, 0, 6)));
        assert_eq!(view(2, 1), Some((0, 2, 0x1688, 0, u32::MAX, 0, 0)));
        assert_eq!(view(2, 3), Some((0, 3, 0x1688, 0, u32::MAX, 0, 3)));
        assert_eq!(view(4, 32), Some((0, 8, 0x1688, 0, u32::MAX, 0, 0)));
        assert_eq!(view(1, 1), None);
        let mut d = default_srv(&desc(3, 10, 4, 1)).unwrap();
        assert!(rewrite_view(View::Srv, &mut d, RGB9E5));
        assert_eq!((rd32(&d, 0), rd32(&d, 8)), (10, 0x1a88));
    }

    #[test]
    fn alpha_forcing_keeps_other_selectors() {
        assert_eq!(force_alpha_one(0x1688), 0x1a88);
        let all_alpha = 3 | 3 << 3 | 3 << 6 | 3 << 9 | 1 << 12;
        assert_eq!(force_alpha_one(all_alpha), 5 | 5 << 3 | 5 << 6 | 5 << 9 | 1 << 12);
        let consts = 4 | 5 << 3 | 1 << 9 | 1 << 12;
        assert_eq!(force_alpha_one(consts), consts);
    }

    #[test]
    fn copy_rules() {
        use Side::*;
        let sub = Texture { subst: Some(RGB9E5), format: 0 };
        let other = Subst { console: 2, host: 10 };
        let tex = |format| Texture { subst: None, format };
        assert_eq!(copy_plan(tex(67), tex(67)), Plan::Forward);
        assert_eq!(copy_plan(tex(10), Footprint(67)), Plan::Forward);
        assert_eq!(copy_plan(sub, sub), Plan::Forward);
        assert!(matches!(copy_plan(sub, Texture { subst: Some(other), format: 0 }), Plan::Refuse(_)));
        assert_eq!(copy_plan(Footprint(67), sub), Plan::Convert(Dir::Encode, RGB9E5));
        assert_eq!(copy_plan(tex(67), sub), Plan::Convert(Dir::Encode, RGB9E5));
        assert_eq!(copy_plan(sub, Footprint(67)), Plan::Convert(Dir::Decode, RGB9E5));
        assert_eq!(copy_plan(sub, tex(67)), Plan::Convert(Dir::Decode, RGB9E5));
        for f in [0, 10, 42, 28] {
            assert!(matches!(copy_plan(Footprint(f), sub), Plan::Refuse(_)), "footprint {f}");
            assert!(matches!(copy_plan(sub, Footprint(f)), Plan::Refuse(_)), "footprint {f}");
            assert!(matches!(copy_plan(tex(f), sub), Plan::Refuse(_)), "texture {f}");
            assert!(matches!(copy_plan(sub, tex(f)), Plan::Refuse(_)), "texture {f}");
        }
    }

    #[test]
    fn resource_copy_rules() {
        let t = desc(3, 67, 0, 1);
        let host = desc(3, 10, 4, 1);
        assert_eq!(resource_plan(None, &t, None, &t), Plan::Forward);
        assert_eq!(resource_plan(Some(RGB9E5), &host, Some(RGB9E5), &host), Plan::Forward);
        assert_eq!(resource_plan(None, &t, Some(RGB9E5), &host), Plan::Convert(Dir::Encode, RGB9E5));
        assert_eq!(resource_plan(Some(RGB9E5), &host, None, &t), Plan::Convert(Dir::Decode, RGB9E5));
        assert!(matches!(resource_plan(None, &desc(3, 10, 0, 1), Some(RGB9E5), &host), Plan::Refuse(_)));
        let mut small = t;
        small[0x18..0x1c].copy_from_slice(&540u32.to_le_bytes());
        assert!(matches!(resource_plan(None, &small, Some(RGB9E5), &host), Plan::Refuse(_)));
        let mut mips = t;
        mips[0x1e..0x20].copy_from_slice(&2u16.to_le_bytes());
        assert!(matches!(resource_plan(Some(RGB9E5), &host, None, &mips), Plan::Refuse(_)));
        assert!(matches!(resource_plan(None, &desc(1, 0, 0, 1), Some(RGB9E5), &host), Plan::Refuse(_)));
    }

    #[test]
    fn copy_regions_and_ends() {
        let mut fp = [0u8; 0x30];
        fp[8] = 1;
        fp[0x10..0x18].copy_from_slice(&0x400u64.to_le_bytes());
        fp[0x18..0x1c].copy_from_slice(&67u32.to_le_bytes());
        fp[0x1c..0x20].copy_from_slice(&64u32.to_le_bytes());
        fp[0x20..0x24].copy_from_slice(&16u32.to_le_bytes());
        fp[0x24..0x28].copy_from_slice(&2u32.to_le_bytes());
        fp[0x28..0x2c].copy_from_slice(&256u32.to_le_bytes());
        assert_eq!(region_of(&fp, None, None, [1, 2, 0]).size, [64, 16, 2]);
        assert_eq!(region_of(&fp, None, Some([4, 2, 1, 20, 7, 2]), [0; 3]).size, [16, 5, 1]);
        assert_eq!(end_of(&fp, [4, 2, 1], 4), crate::uav_subst_copy::End::Buf { res: 0, offset: 0x400 + 4096 + 512 + 16, pitch: 256, slice: 4096 });
        let tex = crate::uav_subst_copy::subresource(7, 3);
        assert_eq!(end_of(&tex, [9, 9, 9], 8), crate::uav_subst_copy::End::Tex(tex));
        let mut d = desc(3, 10, 4, 1);
        d[0x1e..0x20].copy_from_slice(&4u16.to_le_bytes());
        assert_eq!(region_of(&tex, Some(&d), None, [0; 3]).size, [240, 135, 1]);
    }

    #[test]
    fn decode_matches_the_exact_value_of_every_channel() {
        for e in 0..32 {
            for m in 0..512 {
                assert_eq!(rgb9e5_half(m, e), f32_to_f16(rgb9e5_channel(m, e)), "m={m} e={e}");
            }
        }
        let mut seed = 0x9e37_79b9u32;
        for _ in 0..100_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let got = rgb9e5_to_rgba16f(seed);
            let want = rgb9e5_decode(seed).map(f32_to_f16);
            assert_eq!(got, [want[0], want[1], want[2], 0x3c00], "{seed:#010x}");
        }
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
        assert_eq!(f32_to_f16(2f32.powi(-25)), 0);
        assert_eq!(f32_to_f16(1.0 + 2f32.powi(-11)), 0x3c00);
        assert_eq!(f32_to_f16(1.0 + 3.0 * 2f32.powi(-11)), 0x3c02);
        assert_eq!(f32_to_f16(-2.0), 0xc000);
    }
}
