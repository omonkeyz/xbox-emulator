use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::Mutex;

use compat_common::record;

use crate::device_wrap::Guid;
use crate::uav_subst::{Dir, Subst};

const PC_GET_DEVICE: usize = 7;
const PC_DISPATCH: usize = 14;
const PC_COPY_BUFFER: usize = 15;
const PC_COPY_TEXTURE: usize = 16;
const PC_SET_PSO: usize = 25;
const PC_CROOT: usize = 29;
const PC_CROOT_CONSTANTS: usize = 35;
const PC_CROOT_UAV: usize = 41;
const DEV_CREATE_COMPUTE_PSO: usize = 11;
const DEV_CREATE_RS: usize = 16;
const DEV_CREATE_COMMITTED: usize = 27;
const RES_GET_DESC: usize = 10;
const RES_GET_VA: usize = 11;
const OBJ_RELEASE: usize = 2;

const ST_COMMON: u32 = 0;
const ST_UAV: u32 = 0x8;
const ST_COPY_DEST: u32 = 0x400;
const ST_COPY_SOURCE: u32 = 0x800;
const LOC_SUBRESOURCE: u32 = 0;
const LOC_FOOTPRINT: u32 = 1;
const PITCH_ALIGN: u64 = 256;
const PLACEMENT_ALIGN: u64 = 512;

const IID_DEVICE: Guid = Guid { d1: 0x1898_19f1, d2: 0x1db6, d3: 0x4b57, d4: [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7] };
const IID_RESOURCE: Guid = Guid { d1: 0x6964_42be, d2: 0xa72e, d3: 0x4059, d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad] };
const IID_ROOT_SIGNATURE: Guid = Guid { d1: 0xc54a_6b66, d2: 0x72df, d3: 0x4ee8, d4: [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14] };
const IID_PSO: Guid = Guid { d1: 0x765a_30f3, d2: 0xf624, d3: 0x4c6f, d4: [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45] };

pub const CS_RGB9E5: &str = r#"#define RS "RootConstants(num32BitConstants=9, b0), UAV(u0)"
cbuffer C : register(b0) { uint mode; uint w; uint h; uint in_base; uint in_pitch; uint in_slice; uint out_base; uint out_pitch; uint out_slice; };
RWByteAddressBuffer buf : register(u0);

uint half_of(uint m, uint e) {
    if (m == 0) return 0;
    uint p = firstbithigh(m);
    if (p + e < 10) return m << e;
    return ((p + e - 9) << 10) | ((m << (10 - p)) & 0x3ff);
}

uint2 decode(uint v) {
    uint e = v >> 27;
    uint r = half_of(v & 0x1ff, e);
    uint g = half_of((v >> 9) & 0x1ff, e);
    uint b = half_of((v >> 18) & 0x1ff, e);
    return uint2(r | (g << 16), b | (0x3c00 << 16));
}

uint clamped(uint h) {
    if ((h & 0x8000) != 0 || h > 0x7c00) return 0;
    return min(h, 0x7bfc);
}

int2 split(uint h) {
    uint ex = h >> 10;
    if (ex == 0) return int2(int(h & 0x3ff), -24);
    return int2(int((h & 0x3ff) | 0x400), int(ex) - 25);
}

uint round_scaled(int2 v, int s) {
    int k = v.y - s;
    if (k >= 0) return uint(v.x) << uint(k);
    uint sh = uint(-k);
    if (sh > 31) return 0;
    return (uint(v.x) + (1u << (sh - 1))) >> sh;
}

uint encode(uint2 q) {
    uint hr = clamped(q.x & 0xffff);
    uint hg = clamped(q.x >> 16);
    uint hb = clamped(q.y & 0xffff);
    uint hm = max(hr, max(hg, hb));
    if (hm == 0) return 0;
    int2 m = split(hm);
    int ep = max(-16, int(firstbithigh(uint(m.x))) + m.y) + 16;
    if (round_scaled(m, ep - 24) == 512) ep += 1;
    int s = ep - 24;
    return round_scaled(split(hr), s) | (round_scaled(split(hg), s) << 9) | (round_scaled(split(hb), s) << 18) | (uint(ep) << 27);
}

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= w || id.y >= h) return;
    uint i = in_base + id.z * in_slice + id.y * in_pitch;
    uint o = out_base + id.z * out_slice + id.y * out_pitch;
    if (mode == 0) {
        buf.Store2(o + id.x * 8, decode(buf.Load(i + id.x * 4)));
    } else {
        buf.Store(o + id.x * 4, encode(buf.Load2(i + id.x * 8)));
    }
}
"#;

unsafe fn vslot(o: *mut c_void, i: usize) -> usize {
    unsafe { *(*(o as *const *const usize)).add(i) }
}

unsafe fn release(o: *mut c_void) {
    if !o.is_null() {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(vslot(o, OBJ_RELEASE)) };
        unsafe { f(o) };
    }
}

unsafe fn pc(list: *mut c_void, slot: usize) -> usize {
    unsafe { crate::cmdlist_wrap::pc_method(list, slot) }
}

unsafe fn device_of(list: *mut c_void) -> *mut c_void {
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(list, PC_GET_DEVICE)) };
    let mut dev: *mut c_void = core::ptr::null_mut();
    if unsafe { f(list, &IID_DEVICE, &mut dev) } < 0 || dev.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { release(dev) };
    dev
}

#[derive(Clone, Copy)]
struct Pass {
    rs: usize,
    pso: usize,
}

unsafe fn make_pass(dev: *mut c_void) -> Result<Pass, String> {
    if !tombox_xshader::dxc::available() {
        return Err("DXC is unavailable (check TOMBOX_DXC_DIR), so the conversion shader cannot be built".into());
    }
    let cs = tombox_xshader::dxc::compile(CS_RGB9E5, "cs_6_0").map_err(|e| format!("DXC cs_6_0: {e}"))?;
    let mk_rs: unsafe extern "system" fn(*mut c_void, u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_RS)) };
    let mut rs: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk_rs(dev, 0, cs.as_ptr(), cs.len(), &IID_ROOT_SIGNATURE, &mut rs) };
    if hr < 0 || rs.is_null() {
        return Err(format!("CreateRootSignature hr={hr:#010x}"));
    }
    let mut d = [0u8; 56];
    d[0..8].copy_from_slice(&(rs as u64).to_le_bytes());
    d[8..16].copy_from_slice(&(cs.as_ptr() as u64).to_le_bytes());
    d[16..24].copy_from_slice(&(cs.len() as u64).to_le_bytes());
    let mk: unsafe extern "system" fn(*mut c_void, *const u8, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_COMPUTE_PSO)) };
    let mut pso: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk(dev, d.as_ptr(), &IID_PSO, &mut pso) };
    if hr < 0 || pso.is_null() {
        unsafe { release(rs) };
        return Err(format!("CreateComputePipelineState hr={hr:#010x}"));
    }
    Ok(Pass { rs: rs as usize, pso: pso as usize })
}

static PASSES: Mutex<Vec<(usize, Result<Pass, String>)>> = Mutex::new(Vec::new());

unsafe fn pass(dev: *mut c_void) -> Result<Pass, String> {
    let mut g = PASSES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, r)) = g.iter().find(|(d, _)| *d == dev as usize) {
        return r.clone();
    }
    let r = unsafe { make_pass(dev) };
    match &r {
        Ok(_) => record("d3d12_xs", "uav_subst", &format!("RGB9E5 <-> RGBA16F conversion pass ready on device {dev:p}")),
        Err(e) => record("d3d12_xs", "uav_subst", &format!("RGB9E5 <-> RGBA16F conversion pass unavailable on device {dev:p}: {e}")),
    }
    g.push((dev as usize, r.clone()));
    r
}

static SCRATCH: Mutex<Option<HashMap<usize, (usize, u64)>>> = Mutex::new(None);
static RETIRED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

pub fn scratch_cap(need: u64) -> u64 {
    need.max(1 << 16).next_power_of_two()
}

unsafe fn scratch_for(list: *mut c_void, dev: *mut c_void, need: u64) -> Result<usize, String> {
    let key = crate::list_split::origin_of(list);
    let mut g = SCRATCH.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if let Some(&(res, cap)) = m.get(&key) {
        if cap >= need {
            return Ok(res);
        }
    }
    let cap = scratch_cap(need);
    let heap: [u32; 5] = [1, 0, 0, 1, 1];
    let mut desc = [0u8; 0x38];
    desc[0..4].copy_from_slice(&1u32.to_le_bytes());
    desc[0x10..0x18].copy_from_slice(&cap.to_le_bytes());
    desc[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
    desc[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
    desc[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
    desc[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
    desc[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
    desc[0x30..0x34].copy_from_slice(&4u32.to_le_bytes());
    let create: unsafe extern "system" fn(*mut c_void, *const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_COMMITTED)) };
    let mut res: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { create(dev, heap.as_ptr(), 0, desc.as_ptr(), ST_COMMON, core::ptr::null(), &IID_RESOURCE, &mut res) };
    if hr < 0 || res.is_null() {
        return Err(format!("the {cap}-byte scratch buffer could not be created (hr={hr:#010x})"));
    }
    if let Some(old) = m.insert(key, (res as usize, cap)) {
        RETIRED.lock().unwrap_or_else(|e| e.into_inner()).push(old.0);
    }
    Ok(res as usize)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Tex([u8; 0x30]),
    Buf { res: usize, offset: u64, pitch: u64, slice: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub at: [u32; 3],
    pub size: [u32; 3],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub in_fp: Option<u64>,
    pub in_span: Option<(u64, u64)>,
    pub in_pitch: u64,
    pub in_slice: u64,
    pub out_base: u64,
    pub out_pitch: u64,
    pub out_slice: u64,
    pub out_fp: bool,
    pub out_runs: Vec<(u64, u64, u64)>,
    pub total: u64,
}

pub fn bytes_per_texel(dir: Dir, input: bool) -> u64 {
    match (dir, input) {
        (Dir::Encode, true) | (Dir::Decode, false) => 8,
        _ => 4,
    }
}

pub fn host_format(dir: Dir, input: bool, s: Subst) -> u32 {
    if bytes_per_texel(dir, input) == 8 { s.host } else { s.console }
}

fn span(size: [u32; 3], bpp: u64, pitch: u64, slice: u64) -> u64 {
    let [w, h, d] = size.map(u64::from);
    (d - 1) * slice + (h - 1) * pitch + w * bpp
}

pub fn merge_runs(runs: impl IntoIterator<Item = (u64, u64, u64)>) -> Vec<(u64, u64, u64)> {
    let mut out: Vec<(u64, u64, u64)> = Vec::new();
    for (s, d, n) in runs {
        match out.last_mut() {
            Some(l) if l.0 + l.2 == s && l.1 + l.2 == d => l.2 += n,
            _ => out.push((s, d, n)),
        }
    }
    out
}

pub fn plan_layout(dir: Dir, src: &End, dst: &End, r: Region) -> Layout {
    let (bin, bout) = (bytes_per_texel(dir, true), bytes_per_texel(dir, false));
    let [w, h, d] = r.size.map(u64::from);
    let (in_fp, in_span, in_pitch, in_slice, in_len) = match *src {
        End::Tex(_) => {
            let p = (w * bin).next_multiple_of(PITCH_ALIGN);
            (Some(p), None, p, p * h, p * h * d)
        }
        End::Buf { offset, pitch, slice, .. } => {
            let n = span(r.size, bin, pitch, slice);
            (None, Some((offset, n)), pitch, slice, n)
        }
    };
    let out_base = in_len.next_multiple_of(PLACEMENT_ALIGN);
    let (out_pitch, out_slice, out_len, out_fp, out_runs) = match *dst {
        End::Tex(_) => {
            let p = (w * bout).next_multiple_of(PITCH_ALIGN);
            (p, p * h, p * h * d, true, Vec::new())
        }
        End::Buf { offset, pitch, slice, .. } => {
            let rows = (0..d).flat_map(|k| (0..h).map(move |j| (k, j)));
            let runs = merge_runs(rows.map(|(k, j)| (out_base + k * slice + j * pitch, offset + k * slice + j * pitch, w * bout)));
            (pitch, slice, span(r.size, bout, pitch, slice), false, runs)
        }
    };
    Layout { in_fp, in_span, in_pitch, in_slice, out_base, out_pitch, out_slice, out_fp, out_runs, total: out_base + out_len }
}

fn footprint(res: usize, offset: u64, format: u32, size: [u32; 3], pitch: u64) -> [u8; 0x30] {
    let mut l = [0u8; 0x30];
    l[0..8].copy_from_slice(&(res as u64).to_le_bytes());
    l[8..12].copy_from_slice(&LOC_FOOTPRINT.to_le_bytes());
    l[0x10..0x18].copy_from_slice(&offset.to_le_bytes());
    l[0x18..0x1c].copy_from_slice(&format.to_le_bytes());
    l[0x1c..0x20].copy_from_slice(&size[0].to_le_bytes());
    l[0x20..0x24].copy_from_slice(&size[1].to_le_bytes());
    l[0x24..0x28].copy_from_slice(&size[2].to_le_bytes());
    l[0x28..0x2c].copy_from_slice(&(pitch as u32).to_le_bytes());
    l
}

pub fn subresource(res: usize, sub: u32) -> [u8; 0x30] {
    let mut l = [0u8; 0x30];
    l[0..8].copy_from_slice(&(res as u64).to_le_bytes());
    l[8..12].copy_from_slice(&LOC_SUBRESOURCE.to_le_bytes());
    l[0x10..0x14].copy_from_slice(&sub.to_le_bytes());
    l
}

pub unsafe fn res_desc(res: *mut c_void) -> [u8; 0x38] {
    let mut d = [0u8; 0x38];
    let f: unsafe extern "system" fn(*mut c_void, *mut u8) -> *mut u8 = unsafe { core::mem::transmute(vslot(res, RES_GET_DESC)) };
    unsafe { f(res, d.as_mut_ptr()) };
    d
}

pub fn subresource_size(desc: &[u8; 0x38], sub: u32) -> [u32; 3] {
    let mips = u16::from_le_bytes([desc[0x1e], desc[0x1f]]).max(1) as u32;
    let mip = sub % mips;
    let w = u64::from_le_bytes(desc[0x10..0x18].try_into().unwrap());
    let h = u32::from_le_bytes(desc[0x18..0x1c].try_into().unwrap());
    let depth = u16::from_le_bytes([desc[0x1c], desc[0x1d]]) as u32;
    let dim = u32::from_le_bytes(desc[0..4].try_into().unwrap());
    let d = if dim == 4 { (depth >> mip).max(1) } else { 1 };
    [((w >> mip).max(1)) as u32, (h >> mip).max(1), d]
}

unsafe fn barrier(list: *mut c_void, res: usize, before: u32, after: u32) {
    unsafe { crate::fill::barrier_pub(list, res, before, after) };
}

pub unsafe fn convert(list: *mut c_void, dir: Dir, s: Subst, src: End, dst: End, r: Region, src_box: Option<[u32; 6]>) -> Result<(), String> {
    if r.size.contains(&0) {
        return Ok(());
    }
    let ty = unsafe { crate::fill::list_type(list) };
    if ty == crate::fill::LIST_TYPE_COPY {
        return Err("it was recorded on a copy-queue list, which cannot run the conversion compute pass".into());
    }
    let dev = unsafe { device_of(list) };
    if dev.is_null() {
        return Err("the command list has no device".into());
    }
    let p = unsafe { pass(dev) }?;
    let lay = plan_layout(dir, &src, &dst, r);
    if lay.total > u32::MAX as u64 || r.size[2] > 65535 {
        return Err(format!("the region {:?} needs {:#x} scratch bytes, past the 32-bit offsets of the conversion pass", r.size, lay.total));
    }
    let scratch = unsafe { scratch_for(list, dev, lay.total) }?;
    let _ij = unsafe { crate::inject::scope(list, crate::inject::COPY) };
    let va: u64 = unsafe {
        let f: unsafe extern "system" fn(*mut c_void) -> u64 = core::mem::transmute(vslot(scratch as *mut c_void, RES_GET_VA));
        f(scratch as *mut c_void)
    };
    let copy_tex: unsafe extern "system" fn(*mut c_void, *const u8, u32, u32, u32, *const u8, *const u32) = unsafe { core::mem::transmute(pc(list, PC_COPY_TEXTURE)) };
    let copy_buf: unsafe extern "system" fn(*mut c_void, usize, u64, usize, u64, u64) = unsafe { core::mem::transmute(pc(list, PC_COPY_BUFFER)) };
    let (fin, fout) = (host_format(dir, true, s), host_format(dir, false, s));
    crate::placed_offset::with_true_desc(|| unsafe {
        barrier(list, scratch, ST_COMMON, ST_COPY_DEST);
        match (src, lay.in_fp, lay.in_span) {
            (End::Tex(loc), Some(pitch), _) => {
                let fp = footprint(scratch, 0, fin, r.size, pitch);
                copy_tex(list, fp.as_ptr(), 0, 0, 0, loc.as_ptr(), src_box.as_ref().map_or(core::ptr::null(), |b| b.as_ptr()));
            }
            (End::Buf { res, .. }, _, Some((start, n))) => {
                copy_buf(list, scratch, 0, res, start + crate::placed_offset::bytes(res), n);
            }
            _ => {}
        }
        barrier(list, scratch, ST_COPY_DEST, ST_UAV);
        let consts: [u32; 9] = [
            (dir == Dir::Encode) as u32, r.size[0], r.size[1],
            0, lay.in_pitch as u32, lay.in_slice as u32,
            lay.out_base as u32, lay.out_pitch as u32, lay.out_slice as u32,
        ];
        let set_rs: unsafe extern "system" fn(*mut c_void, *mut c_void) = core::mem::transmute(pc(list, PC_CROOT));
        let set_pso: unsafe extern "system" fn(*mut c_void, *mut c_void) = core::mem::transmute(pc(list, PC_SET_PSO));
        let set_consts: unsafe extern "system" fn(*mut c_void, u32, u32, *const u32, u32) = core::mem::transmute(pc(list, PC_CROOT_CONSTANTS));
        let set_uav: unsafe extern "system" fn(*mut c_void, u32, u64) = core::mem::transmute(pc(list, PC_CROOT_UAV));
        let dispatch: unsafe extern "system" fn(*mut c_void, u32, u32, u32) = core::mem::transmute(pc(list, PC_DISPATCH));
        set_rs(list, p.rs as *mut c_void);
        set_pso(list, p.pso as *mut c_void);
        set_consts(list, 0, 9, consts.as_ptr(), 0);
        set_uav(list, 1, va);
        dispatch(list, r.size[0].div_ceil(8), r.size[1].div_ceil(8), r.size[2]);
        crate::cl2_state::reapply(list);
        barrier(list, scratch, ST_UAV, ST_COPY_SOURCE);
        match dst {
            End::Tex(loc) => {
                let fp = footprint(scratch, lay.out_base, fout, r.size, lay.out_pitch);
                copy_tex(list, loc.as_ptr(), r.at[0], r.at[1], r.at[2], fp.as_ptr(), core::ptr::null());
            }
            End::Buf { res, .. } => {
                let base = crate::placed_offset::bytes(res);
                for &(from, to, n) in &lay.out_runs {
                    copy_buf(list, res, to + base, scratch, from, n);
                }
            }
        }
        barrier(list, scratch, ST_COPY_SOURCE, ST_COMMON);
    });
    let dst_res = match dst {
        End::Tex(loc) => u64::from_le_bytes(loc[0..8].try_into().unwrap()) as usize,
        End::Buf { res, .. } => res,
    };
    crate::placed::note_gpu_write(dst_res as *mut c_void);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGB9E5: Subst = crate::uav_subst::TABLE[0];

    #[test]
    fn formats_and_texel_sizes_follow_the_direction() {
        assert_eq!((bytes_per_texel(Dir::Encode, true), bytes_per_texel(Dir::Encode, false)), (8, 4));
        assert_eq!((bytes_per_texel(Dir::Decode, true), bytes_per_texel(Dir::Decode, false)), (4, 8));
        assert_eq!((host_format(Dir::Encode, true, RGB9E5), host_format(Dir::Encode, false, RGB9E5)), (10, 67));
        assert_eq!((host_format(Dir::Decode, true, RGB9E5), host_format(Dir::Decode, false, RGB9E5)), (67, 10));
    }

    #[test]
    fn texture_to_texture_layout_uses_aligned_footprints() {
        let r = Region { at: [3, 4, 0], size: [100, 7, 1] };
        let l = plan_layout(Dir::Encode, &End::Tex([0; 0x30]), &End::Tex([0; 0x30]), r);
        assert_eq!(l.in_fp, Some(1024));
        assert_eq!((l.in_pitch, l.in_slice), (1024, 7 * 1024));
        assert_eq!(l.out_base, 7 * 1024);
        assert_eq!((l.out_pitch, l.out_slice), (512, 7 * 512));
        assert!(l.out_fp && l.out_runs.is_empty());
        assert_eq!(l.total, 7 * 1024 + 7 * 512);
        let l = plan_layout(Dir::Decode, &End::Tex([0; 0x30]), &End::Tex([0; 0x30]), Region { at: [0; 3], size: [65, 3, 2] });
        assert_eq!((l.in_pitch, l.in_slice, l.out_base), (512, 1536, 3072));
        assert_eq!((l.out_pitch, l.out_slice, l.total), (768, 2304, 3072 + 4608));
    }

    #[test]
    fn buffer_destination_rows_are_copied_without_touching_padding() {
        let dst = End::Buf { res: 1, offset: 0x1000 + 2 * 256 + 5 * 4, pitch: 256, slice: 256 * 16 };
        let l = plan_layout(Dir::Encode, &End::Tex([0; 0x30]), &dst, Region { at: [5, 2, 0], size: [10, 3, 1] });
        assert_eq!(l.out_runs, vec![(l.out_base, 0x1000 + 512 + 20, 40), (l.out_base + 256, 0x1000 + 768 + 20, 40), (l.out_base + 512, 0x1000 + 1024 + 20, 40)]);
        assert_eq!(l.total, l.out_base + 2 * 256 + 40);
        let full = End::Buf { res: 1, offset: 0, pitch: 256, slice: 256 * 4 };
        let l = plan_layout(Dir::Encode, &End::Tex([0; 0x30]), &full, Region { at: [0; 3], size: [64, 4, 3] });
        assert_eq!(l.out_runs, vec![(l.out_base, 0, 3 * 1024)]);
    }

    #[test]
    fn buffer_source_is_read_as_one_span() {
        let src = End::Buf { res: 1, offset: 0x200 + 256 + 8, pitch: 256, slice: 256 * 8 };
        let l = plan_layout(Dir::Decode, &src, &End::Tex([0; 0x30]), Region { at: [0; 3], size: [4, 2, 2] });
        assert_eq!(l.in_span, Some((0x200 + 256 + 8, 2048 + 256 + 16)));
        assert_eq!((l.in_pitch, l.in_slice, l.in_fp), (256, 2048, None));
        assert_eq!(l.out_base, (2048u64 + 256 + 16).next_multiple_of(512));
    }

    #[test]
    fn merge_only_joins_runs_contiguous_on_both_sides() {
        assert_eq!(merge_runs([(0, 100, 8), (8, 108, 8), (16, 200, 8), (24, 208, 8)]), vec![(0, 100, 16), (16, 200, 16)]);
        assert_eq!(merge_runs([(0, 0, 4), (8, 4, 4)]), vec![(0, 0, 4), (8, 4, 4)]);
    }

    #[test]
    fn subresource_sizes_follow_mips() {
        let mut d = [0u8; 0x38];
        d[0..4].copy_from_slice(&4u32.to_le_bytes());
        d[0x10..0x18].copy_from_slice(&64u64.to_le_bytes());
        d[0x18..0x1c].copy_from_slice(&16u32.to_le_bytes());
        d[0x1c..0x1e].copy_from_slice(&8u16.to_le_bytes());
        d[0x1e..0x20].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(subresource_size(&d, 0), [64, 16, 8]);
        assert_eq!(subresource_size(&d, 2), [16, 4, 2]);
        d[0..4].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(subresource_size(&d, 4), [32, 8, 1]);
    }
}
