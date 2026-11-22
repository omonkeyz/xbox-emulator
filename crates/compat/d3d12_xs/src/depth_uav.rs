
use compat_common::record;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use crate::device_wrap::Guid;

const M: &str = "d3d12_xs";
const TAG: &str = "depth_uav";

pub const FLAG_DSV: u32 = 0x2;
pub const FLAG_UAV: u32 = 0x4;
pub const ST_UAV: u32 = 0x8;
pub const ST_DEPTH_WRITE: u32 = 0x10;
pub const ST_DEPTH_READ: u32 = 0x20;
pub const ST_NON_PIXEL_SR: u32 = 0x40;
pub const ST_PIXEL_SR: u32 = 0x80;
pub const D_UAV_STATE: u32 = ST_DEPTH_READ | ST_NON_PIXEL_SR;
pub const S_READ_STATE: u32 = ST_NON_PIXEL_SR | ST_PIXEL_SR;
pub const ALL_SUBRESOURCES: u32 = u32::MAX;
const BARRIER_BEGIN_ONLY: u32 = 0x1;

const FMT_R32_TYPELESS: u32 = 39;
const FMT_R32_FLOAT: u32 = 41;
const FMT_R32_UINT: u32 = 42;
const FMT_R32_SINT: u32 = 43;

const DEV_CREATE_GFX_PSO: usize = 10;
const DEV_CREATE_COMPUTE_PSO: usize = 11;
const DEV_CREATE_HEAP: usize = 14;
const DEV_INCREMENT: usize = 15;
const DEV_CREATE_RS: usize = 16;
const DEV_CREATE_SRV: usize = 18;
const DEV_CREATE_UAV: usize = 19;
const DEV_CREATE_DSV: usize = 21;
const DEV_CREATE_COMMITTED: usize = 27;
const OBJ_SET_PRIVATE_IFACE: usize = 5;
const HEAP_CPU_START: usize = 9;
const HEAP_GPU_START: usize = 10;

const PC_DRAW: usize = 12;
const PC_DISPATCH: usize = 14;
const PC_TOPOLOGY: usize = 20;
const PC_VIEWPORTS: usize = 21;
const PC_SCISSORS: usize = 22;
const PC_SET_PSO: usize = 25;
const PC_HEAPS: usize = 28;
const PC_CROOT: usize = 29;
const PC_GROOT: usize = 30;
const PC_CTABLE: usize = 31;
const PC_GTABLE: usize = 32;
const PC_SET_TARGETS: usize = 46;
const PC_SET_PREDICATION: usize = 55;
const XBOX_CLEAR_UAV_UINT: usize = 53;
const XBOX_CLEAR_UAV_FLOAT: usize = 54;

const MAX_PAIRS: u32 = 4096;

const IID_RESOURCE: Guid = Guid { d1: 0x6964_42be, d2: 0xa72e, d3: 0x4059, d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad] };
const IID_ROOT_SIGNATURE: Guid = Guid { d1: 0xc54a_6b66, d2: 0x72df, d3: 0x4ee8, d4: [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14] };
const IID_PSO: Guid = Guid { d1: 0x765a_30f3, d2: 0xf624, d3: 0x4c6f, d4: [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45] };
const IID_DESCRIPTOR_HEAP: Guid = Guid { d1: 0x8efb_471d, d2: 0x616c, d3: 0x4f49, d4: [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51] };
const KEY_SHADOW: Guid = Guid { d1: 0x5d0b_7a11, d2: 0x3e2c, d3: 0x4f6d, d4: [0x9a, 0x51, 0x0c, 0x7e, 0x22, 0xd4, 0x8b, 0x13] };

pub fn d_real(title: u32) -> u32 {
    if title & ST_UAV != 0 { D_UAV_STATE } else { title }
}

pub fn s_real(title: u32) -> u32 {
    if title & ST_UAV != 0 { ST_UAV } else { S_READ_STATE }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Formats {
    pub dsv: u32,
    pub d_srv: u32,
    pub planes: u32,
}

pub fn formats(fmt: u32) -> Option<Formats> {
    match fmt {
        19 | 20 => Some(Formats { dsv: 20, d_srv: 21, planes: 2 }),
        39 | 40 => Some(Formats { dsv: 40, d_srv: 41, planes: 1 }),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    None,
    Pair(Formats),
    Refuse(String),
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(d[o..o + 2].try_into().unwrap())
}

pub fn plan(desc: &[u8]) -> Plan {
    if desc.len() < 0x38 || u32_at(desc, 0x30) & (FLAG_DSV | FLAG_UAV) != FLAG_DSV | FLAG_UAV {
        return Plan::None;
    }
    let fmt = u32_at(desc, 0x20);
    let Some(f) = formats(fmt) else {
        return Plan::Refuse(format!("format {fmt} is not a D32_FLOAT or D32_FLOAT_S8X24_UINT family"));
    };
    let (dim, depth, mips, samples) = (u32_at(desc, 0), u16_at(desc, 0x1c), u16_at(desc, 0x1e), u32_at(desc, 0x24));
    if dim != 3 || depth != 1 || mips != 1 || samples != 1 {
        return Plan::Refuse(format!("dimension {dim}, {depth} slice(s), {mips} mip(s), {samples} sample(s): only 2D with one slice, one mip and one sample is shadowed"));
    }
    Plan::Pair(f)
}

pub fn depth_desc(desc: &[u8; 0x38]) -> [u8; 0x38] {
    let mut d = *desc;
    let flags = u32_at(&d, 0x30) & !FLAG_UAV;
    d[0x30..0x34].copy_from_slice(&flags.to_le_bytes());
    d
}

pub fn shadow_desc(desc: &[u8; 0x38]) -> [u8; 0x38] {
    let mut d = *desc;
    d[8..16].copy_from_slice(&0u64.to_le_bytes());
    d[0x20..0x24].copy_from_slice(&FMT_R32_TYPELESS.to_le_bytes());
    d[0x28..0x2c].copy_from_slice(&0u32.to_le_bytes());
    d[0x2c..0x30].copy_from_slice(&0u32.to_le_bytes());
    d[0x30..0x34].copy_from_slice(&FLAG_UAV.to_le_bytes());
    d
}

pub fn shadow_uav_desc(title: Option<&[u8; 0x28]>) -> Result<[u8; 0x28], String> {
    let mut d = [0u8; 0x28];
    d[0..4].copy_from_slice(&FMT_R32_FLOAT.to_le_bytes());
    d[4..8].copy_from_slice(&4u32.to_le_bytes());
    let Some(t) = title else { return Ok(d) };
    let fmt = u32_at(t, 0);
    let dim = u32_at(t, 4);
    if dim != 4 && dim != 5 {
        return Err(format!("UAV view dimension {dim} on a shadowed depth texture (only TEXTURE2D / TEXTURE2DARRAY)"));
    }
    if u32_at(t, 8) != 0 {
        return Err(format!("UAV view of mip {} on a one-mip shadowed depth texture", u32_at(t, 8)));
    }
    d.copy_from_slice(t);
    let view_fmt = match fmt {
        FMT_R32_FLOAT | FMT_R32_UINT | FMT_R32_SINT => fmt,
        0 | 19 | 20 | 21 | 39 | 40 => FMT_R32_FLOAT,
        other => return Err(format!("UAV view format {other} on a 32-bit depth texture")),
    };
    d[0..4].copy_from_slice(&view_fmt.to_le_bytes());
    if dim == 4 {
        d[12..16].copy_from_slice(&0u32.to_le_bytes());
    } else {
        d[20..24].copy_from_slice(&0u32.to_le_bytes());
    }
    Ok(d)
}

#[derive(Clone, Copy, Debug)]
struct Pair {
    shadow: usize,
    slot: u32,
    width: u32,
    height: u32,
    f: Formats,
}

static PAIRS: Mutex<Option<HashMap<usize, Pair>>> = Mutex::new(None);
static REFUSED: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
static NEXT_SLOT: AtomicU32 = AtomicU32::new(0);

fn pair_of(res: usize) -> Option<Pair> {
    if res == 0 {
        return None;
    }
    PAIRS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&res).copied())
}

fn is_refused(res: usize) -> bool {
    res != 0 && REFUSED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&res))
}

pub fn forget(res: usize) {
    if let Some(m) = PAIRS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(&res);
    }
    if let Some(s) = REFUSED.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.remove(&res);
    }
}

pub fn refuse(res: usize, why: &str) {
    REFUSED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(res);
    static N: AtomicUsize = AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 8 {
        record(M, TAG, &format!("depth texture {res:#x} with UAV access created without it ({why}); its UAV views are refused"));
    }
}

unsafe fn vslot(o: *mut c_void, i: usize) -> usize {
    unsafe { *(*(o as *const *const usize)).add(i) }
}

unsafe fn release(o: *mut c_void) {
    if !o.is_null() {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(vslot(o, 2)) };
        unsafe { f(o) };
    }
}

unsafe fn pc(list: *mut c_void, slot: usize) -> usize {
    unsafe { crate::cmdlist_wrap::pc_method(list, slot) }
}

struct Pass {
    rs: usize,
    cs: usize,
    gfx: [usize; 2],
    heap: usize,
    cpu: u64,
    gpu: u64,
    inc: u64,
    dsv_cpu: u64,
    dsv_inc: u64,
}

unsafe impl Send for Pass {}
unsafe impl Sync for Pass {}

const CS_COPY_IN: &str = "Texture2D<float> src : register(t0);\nRWTexture2D<uint> dst : register(u0);\n[numthreads(8, 8, 1)]\nvoid main(uint3 id : SV_DispatchThreadID) {\n    uint w, h;\n    dst.GetDimensions(w, h);\n    if (id.x < w && id.y < h) dst[id.xy] = asuint(src.Load(int3(id.xy, 0)));\n}\n";
const VS_FULL: &str = "float4 main(uint id : SV_VertexID) : SV_Position {\n    float2 uv = float2((id << 1) & 2, id & 2);\n    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);\n}\n";
const PS_COPY_OUT: &str = "Texture2D<uint> src : register(t0);\nfloat main(float4 p : SV_Position) : SV_Depth {\n    return asfloat(src.Load(int3(p.xy, 0)));\n}\n";

unsafe fn blob_bytes(b: *mut c_void) -> Vec<u8> {
    let ptr: unsafe extern "system" fn(*mut c_void) -> *const u8 = unsafe { core::mem::transmute(vslot(b, 3)) };
    let size: unsafe extern "system" fn(*mut c_void) -> usize = unsafe { core::mem::transmute(vslot(b, 4)) };
    unsafe { core::slice::from_raw_parts(ptr(b), size(b)) }.to_vec()
}

unsafe fn make_root_signature(dev: *mut c_void) -> Result<usize, String> {
    let ser = crate::host_proc(b"D3D12SerializeRootSignature\0");
    if ser == 0 {
        return Err("host d3d12.dll has no D3D12SerializeRootSignature".into());
    }
    let mut ranges = [0u8; 40];
    for (i, ty) in [0u32, 1].iter().enumerate() {
        let r = &mut ranges[i * 20..i * 20 + 20];
        r[0..4].copy_from_slice(&ty.to_le_bytes());
        r[4..8].copy_from_slice(&1u32.to_le_bytes());
        r[16..20].copy_from_slice(&(i as u32).to_le_bytes());
    }
    let mut param = [0u8; 32];
    param[8..12].copy_from_slice(&2u32.to_le_bytes());
    param[16..24].copy_from_slice(&(ranges.as_ptr() as u64).to_le_bytes());
    let mut desc = [0u8; 40];
    desc[0..4].copy_from_slice(&1u32.to_le_bytes());
    desc[8..16].copy_from_slice(&(param.as_ptr() as u64).to_le_bytes());
    let ser: unsafe extern "system" fn(*const u8, u32, *mut *mut c_void, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(ser) };
    let (mut b, mut e): (*mut c_void, *mut c_void) = (core::ptr::null_mut(), core::ptr::null_mut());
    let hr = unsafe { ser(desc.as_ptr(), 1, &mut b, &mut e) };
    unsafe { release(e) };
    if hr < 0 || b.is_null() {
        return Err(format!("D3D12SerializeRootSignature hr={hr:#010x}"));
    }
    let bytes = unsafe { blob_bytes(b) };
    unsafe { release(b) };
    let mk: unsafe extern "system" fn(*mut c_void, u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_RS)) };
    let mut rs: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk(dev, 0, bytes.as_ptr(), bytes.len(), &IID_ROOT_SIGNATURE, &mut rs) };
    if hr < 0 || rs.is_null() {
        return Err(format!("CreateRootSignature hr={hr:#010x}"));
    }
    Ok(rs as usize)
}

fn compile(src: &str, profile: &str) -> Result<Vec<u8>, String> {
    tombox_xshader::dxc::compile(src, profile).map_err(|e| format!("DXC {profile}: {e}"))
}

unsafe fn make_compute_pso(dev: *mut c_void, rs: usize, cs: &[u8]) -> Result<usize, String> {
    let mut d = [0u8; 56];
    d[0..8].copy_from_slice(&(rs as u64).to_le_bytes());
    d[8..16].copy_from_slice(&(cs.as_ptr() as u64).to_le_bytes());
    d[16..24].copy_from_slice(&(cs.len() as u64).to_le_bytes());
    let mk: unsafe extern "system" fn(*mut c_void, *const u8, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_COMPUTE_PSO)) };
    let mut p: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk(dev, d.as_ptr(), &IID_PSO, &mut p) };
    if hr < 0 || p.is_null() {
        return Err(format!("CreateComputePipelineState hr={hr:#010x}"));
    }
    Ok(p as usize)
}

pub fn gfx_desc(rs: usize, vs: &[u8], ps: &[u8], dsv_format: u32) -> Vec<u8> {
    let mut d = vec![0u8; 0x290];
    let w32 = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
    let w64 = |d: &mut Vec<u8>, o: usize, v: u64| d[o..o + 8].copy_from_slice(&v.to_le_bytes());
    w64(&mut d, 0x00, rs as u64);
    w64(&mut d, 0x08, vs.as_ptr() as u64);
    w64(&mut d, 0x10, vs.len() as u64);
    w64(&mut d, 0x18, ps.as_ptr() as u64);
    w64(&mut d, 0x20, ps.len() as u64);
    w32(&mut d, 0x1c0, u32::MAX);
    w32(&mut d, 0x1c4, 3);
    w32(&mut d, 0x1c8, 1);
    w32(&mut d, 0x1dc, 1);
    w32(&mut d, 0x1f0, 1);
    w32(&mut d, 0x1f4, 1);
    w32(&mut d, 0x1f8, 8);
    w32(&mut d, 0x1fc, 0);
    w32(&mut d, 0x23c, 3);
    w32(&mut d, 0x240, 0);
    w32(&mut d, 0x264, dsv_format);
    w32(&mut d, 0x268, 1);
    d
}

unsafe fn make_gfx_pso(dev: *mut c_void, rs: usize, vs: &[u8], ps: &[u8], dsv_format: u32) -> Result<usize, String> {
    let d = gfx_desc(rs, vs, ps, dsv_format);
    let mk: unsafe extern "system" fn(*mut c_void, *const u8, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_GFX_PSO)) };
    let mut p: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk(dev, d.as_ptr(), &IID_PSO, &mut p) };
    if hr < 0 || p.is_null() {
        return Err(format!("CreateGraphicsPipelineState (DSV format {dsv_format}) hr={hr:#010x}"));
    }
    Ok(p as usize)
}

unsafe fn make_heap(dev: *mut c_void, ty: u32, n: u32, visible: bool) -> Result<usize, String> {
    let desc: [u32; 4] = [ty, n, visible as u32, 0];
    let mk: unsafe extern "system" fn(*mut c_void, *const u32, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_HEAP)) };
    let mut h: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk(dev, desc.as_ptr(), &IID_DESCRIPTOR_HEAP, &mut h) };
    if hr < 0 || h.is_null() {
        return Err(format!("CreateDescriptorHeap type {ty} x{n} hr={hr:#010x}"));
    }
    Ok(h as usize)
}

pub fn heap_start_result(returned: u64, out_ptr: u64, out: u64) -> u64 {
    if returned == out_ptr { out } else { returned }
}

unsafe fn heap_start(heap: usize, slot: usize) -> u64 {
    let f: unsafe extern "system" fn(*mut c_void, *mut u64) -> u64 = unsafe { core::mem::transmute(vslot(heap as *mut c_void, slot)) };
    let mut v = 0u64;
    let r = unsafe { f(heap as *mut c_void, &mut v) };
    heap_start_result(r, &v as *const u64 as u64, v)
}

unsafe fn make_pass(dev: *mut c_void) -> Result<Pass, String> {
    if !tombox_xshader::dxc::available() {
        return Err("DXC unavailable (check TOMBOX_DXC_DIR)".into());
    }
    let cs = compile(CS_COPY_IN, "cs_6_0")?;
    let vs = compile(VS_FULL, "vs_6_0")?;
    let ps = compile(PS_COPY_OUT, "ps_6_0")?;
    let rs = unsafe { make_root_signature(dev) }?;
    let cs = unsafe { make_compute_pso(dev, rs, &cs) }?;
    let g32 = unsafe { make_gfx_pso(dev, rs, &vs, &ps, 40) }?;
    let g32s8 = unsafe { make_gfx_pso(dev, rs, &vs, &ps, 20) }?;
    let heap = unsafe { make_heap(dev, 0, MAX_PAIRS * 4, true) }?;
    let dsv_heap = unsafe { make_heap(dev, 3, MAX_PAIRS, false) }?;
    let inc_f: unsafe extern "system" fn(*mut c_void, u32) -> u32 = unsafe { core::mem::transmute(vslot(dev, DEV_INCREMENT)) };
    Ok(Pass {
        rs,
        cs,
        gfx: [g32, g32s8],
        heap,
        cpu: unsafe { heap_start(heap, HEAP_CPU_START) },
        gpu: unsafe { heap_start(heap, HEAP_GPU_START) },
        inc: unsafe { inc_f(dev, 0) } as u64,
        dsv_cpu: unsafe { heap_start(dsv_heap, HEAP_CPU_START) },
        dsv_inc: unsafe { inc_f(dev, 3) } as u64,
    })
}

static PASS: OnceLock<Result<Pass, String>> = OnceLock::new();

fn pass(dev: *mut c_void) -> Result<&'static Pass, String> {
    let p = PASS.get_or_init(|| {
        let r = unsafe { make_pass(dev) };
        match &r {
            Ok(_) => record(M, TAG, "depth/UAV shadow passes ready (copy-in compute, SV_Depth write-back)"),
            Err(e) => record(M, TAG, &format!("depth/UAV shadow passes unavailable: {e}")),
        }
        r
    });
    p.as_ref().map_err(|e| e.clone())
}

fn srv_desc(fmt: u32) -> [u8; 40] {
    let mut d = [0u8; 40];
    d[0..4].copy_from_slice(&fmt.to_le_bytes());
    d[4..8].copy_from_slice(&4u32.to_le_bytes());
    d[8..12].copy_from_slice(&0x1688u32.to_le_bytes());
    d[20..24].copy_from_slice(&1u32.to_le_bytes());
    d
}

fn uav_desc(fmt: u32) -> [u8; 40] {
    let mut d = [0u8; 40];
    d[0..4].copy_from_slice(&fmt.to_le_bytes());
    d[4..8].copy_from_slice(&4u32.to_le_bytes());
    d
}

pub unsafe fn register(dev: *mut c_void, d: *mut c_void, desc: &[u8; 0x38], heap: &[u32; 5], title_state: u32, f: Formats) -> Result<(), String> {
    let p = pass(dev)?;
    let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
    if slot >= MAX_PAIRS {
        return Err(format!("more than {MAX_PAIRS} depth/UAV textures"));
    }
    let sd = shadow_desc(desc);
    let mk: unsafe extern "system" fn(*mut c_void, *const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_COMMITTED)) };
    let mut s: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk(dev, heap.as_ptr(), 0, sd.as_ptr(), s_real(title_state), core::ptr::null(), &IID_RESOURCE, &mut s) };
    if hr < 0 || s.is_null() {
        return Err(format!("shadow CreateCommittedResource hr={hr:#010x}"));
    }
    let set: unsafe extern "system" fn(*mut c_void, *const Guid, *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(d, OBJ_SET_PRIVATE_IFACE)) };
    let hr = unsafe { set(d, &KEY_SHADOW, s) };
    unsafe { release(s) };
    if hr < 0 {
        return Err(format!("SetPrivateDataInterface hr={hr:#010x}"));
    }
    let srv: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, u64) = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_SRV)) };
    let uav: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u8, u64) = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_UAV)) };
    let dsv: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, u64) = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_DSV)) };
    let at = |k: u64| p.cpu + (slot as u64 * 4 + k) * p.inc;
    unsafe {
        srv(dev, d, srv_desc(f.d_srv).as_ptr(), at(0));
        uav(dev, s, core::ptr::null_mut(), uav_desc(FMT_R32_UINT).as_ptr(), at(1));
        srv(dev, s, srv_desc(FMT_R32_UINT).as_ptr(), at(2));
        uav(dev, s, core::ptr::null_mut(), uav_desc(FMT_R32_UINT).as_ptr(), at(3));
        let mut dd = [0u8; 24];
        dd[0..4].copy_from_slice(&f.dsv.to_le_bytes());
        dd[4..8].copy_from_slice(&3u32.to_le_bytes());
        dsv(dev, d, dd.as_ptr(), p.dsv_cpu + slot as u64 * p.dsv_inc);
    }
    let pair = Pair {
        shadow: s as usize,
        slot,
        width: u64::from_le_bytes(desc[0x10..0x18].try_into().unwrap()) as u32,
        height: u32_at(desc, 0x18),
        f,
    };
    PAIRS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(d as usize, pair);
    static N: AtomicUsize = AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 8 {
        record(M, TAG, &format!("depth texture {:#x} {}x{} fmt {} has UAV access on the console: PC depth texture without UAV + R32 UAV shadow {s:p}", d as usize, pair.width, pair.height, u32_at(desc, 0x20)));
    }
    Ok(())
}


static PREV_UAV: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn create_uav(this: *mut c_void, res: *mut c_void, counter: *mut c_void, desc: *const u8, dest: u64) {
    let prev: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u8, u64) = unsafe { core::mem::transmute(PREV_UAV.load(Ordering::Relaxed)) };
    static N: AtomicUsize = AtomicUsize::new(0);
    if let Some(p) = pair_of(res as usize) {
        let title = if desc.is_null() { None } else { Some(unsafe { core::ptr::read_unaligned(desc as *const [u8; 0x28]) }) };
        match shadow_uav_desc(title.as_ref()) {
            Ok(d) => return unsafe { prev(this, p.shadow as *mut c_void, counter, d.as_ptr(), dest) },
            Err(e) => {
                if N.fetch_add(1, Ordering::Relaxed) < 8 {
                    record(M, TAG, &format!("CreateUnorderedAccessView on depth texture {res:p}: {e}; null view written"));
                }
                return unsafe { null_uav(this, prev, dest) };
            }
        }
    }
    if is_refused(res as usize) {
        if N.fetch_add(1, Ordering::Relaxed) < 8 {
            record(M, TAG, &format!("CreateUnorderedAccessView on depth texture {res:p} without a shadow: refused, null view written"));
        }
        return unsafe { null_uav(this, prev, dest) };
    }
    unsafe { prev(this, res, counter, desc, dest) }
}

unsafe fn null_uav(this: *mut c_void, prev: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u8, u64), dest: u64) {
    let d = uav_desc(FMT_R32_FLOAT);
    unsafe { prev(this, core::ptr::null_mut(), core::ptr::null_mut(), d.as_ptr(), dest) };
}

pub fn hook_device(v: &mut [usize]) {
    if v.len() <= DEV_CREATE_UAV || v[DEV_CREATE_UAV] == create_uav as usize {
        return;
    }
    PREV_UAV.store(v[DEV_CREATE_UAV], Ordering::Relaxed);
    v[DEV_CREATE_UAV] = create_uav as usize;
}


static PREV_CLEAR: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];

unsafe extern "system" fn clear_uav_uint(this: *mut c_void, gpu: u64, cpu: u64, res: *mut c_void, values: *const u32, n: u32, rects: *const u8) {
    let prev: unsafe extern "system" fn(*mut c_void, u64, u64, *mut c_void, *const u32, u32, *const u8) = unsafe { core::mem::transmute(PREV_CLEAR[0].load(Ordering::Relaxed)) };
    let res = pair_of(res as usize).map_or(res, |p| p.shadow as *mut c_void);
    unsafe { prev(this, gpu, cpu, res, values, n, rects) }
}

unsafe extern "system" fn clear_uav_float(this: *mut c_void, gpu: u64, cpu: u64, res: *mut c_void, values: *const f32, n: u32, rects: *const u8) {
    let prev: unsafe extern "system" fn(*mut c_void, u64, u64, *mut c_void, *const f32, u32, *const u8) = unsafe { core::mem::transmute(PREV_CLEAR[1].load(Ordering::Relaxed)) };
    let res = pair_of(res as usize).map_or(res, |p| p.shadow as *mut c_void);
    unsafe { prev(this, gpu, cpu, res, values, n, rects) }
}

pub fn install_cmdlist(vt: &[AtomicUsize]) {
    if vt.len() <= XBOX_CLEAR_UAV_FLOAT {
        return;
    }
    for (i, (slot, f)) in [(XBOX_CLEAR_UAV_UINT, clear_uav_uint as usize), (XBOX_CLEAR_UAV_FLOAT, clear_uav_float as usize)].into_iter().enumerate() {
        let cur = vt[slot].load(Ordering::Relaxed);
        if cur != f {
            PREV_CLEAR[i].store(cur, Ordering::Relaxed);
            vt[slot].store(f, Ordering::Relaxed);
        }
    }
}


fn transition(res: usize, sub: u32, before: u32, after: u32) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[8..16].copy_from_slice(&(res as u64).to_le_bytes());
    b[16..20].copy_from_slice(&sub.to_le_bytes());
    b[20..24].copy_from_slice(&before.to_le_bytes());
    b[24..28].copy_from_slice(&after.to_le_bytes());
    b
}

fn uav_barrier(res: usize) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[0..4].copy_from_slice(&2u32.to_le_bytes());
    b[8..16].copy_from_slice(&(res as u64).to_le_bytes());
    b
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Barrier([u8; 32]),
    CopyIn(usize),
    CopyOut(usize),
}

pub fn plan_barriers(src: &[u8], shadow_of: &dyn Fn(usize) -> Option<(usize, u32)>, note: &mut dyn FnMut(String)) -> Option<Vec<Step>> {
    let ptr = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize;
    let touches = src.chunks_exact(32).any(|b| matches!(u32_at(b, 0), 0 | 2) && shadow_of(ptr(b, 8)).is_some());
    if !touches {
        return None;
    }
    let mut out = Vec::new();
    let push_t = |out: &mut Vec<Step>, res: usize, sub: u32, before: u32, after: u32| {
        if before != after {
            out.push(Step::Barrier(transition(res, sub, before, after)));
        }
    };
    for b in src.chunks_exact(32) {
        let b: [u8; 32] = b.try_into().unwrap();
        let res = ptr(&b, 8);
        let Some((s, planes)) = (if matches!(u32_at(&b, 0), 0 | 2) { shadow_of(res) } else { None }) else {
            out.push(Step::Barrier(b));
            continue;
        };
        if u32_at(&b, 0) == 2 {
            out.push(Step::Barrier(uav_barrier(s)));
            continue;
        }
        let flags = u32_at(&b, 4);
        if flags & BARRIER_BEGIN_ONLY != 0 {
            continue;
        }
        let (sub, before, after) = (u32_at(&b, 16), u32_at(&b, 20), u32_at(&b, 24));
        let whole = sub == ALL_SUBRESOURCES || (planes == 1 && sub == 0);
        let enter = after & ST_UAV != 0 && before & ST_UAV == 0;
        let leave = before & ST_UAV != 0 && after & ST_UAV == 0;
        if !whole {
            if enter || leave {
                note(format!("depth texture {res:#x}: UAV transition of subresource {sub} only; contents are not converted between depth and the UAV shadow"));
            }
            push_t(&mut out, res, sub, d_real(before), d_real(after));
            if sub == 0 {
                push_t(&mut out, s, ALL_SUBRESOURCES, s_real(before), s_real(after));
            }
            continue;
        }
        if enter {
            push_t(&mut out, res, ALL_SUBRESOURCES, d_real(before), D_UAV_STATE);
            push_t(&mut out, s, ALL_SUBRESOURCES, s_real(before), ST_UAV);
            out.push(Step::CopyIn(res));
            out.push(Step::Barrier(uav_barrier(s)));
        } else if leave {
            push_t(&mut out, s, ALL_SUBRESOURCES, ST_UAV, S_READ_STATE);
            push_t(&mut out, res, ALL_SUBRESOURCES, D_UAV_STATE, ST_DEPTH_WRITE);
            out.push(Step::CopyOut(res));
            push_t(&mut out, res, ALL_SUBRESOURCES, ST_DEPTH_WRITE, after);
        } else {
            push_t(&mut out, res, ALL_SUBRESOURCES, d_real(before), d_real(after));
            push_t(&mut out, s, ALL_SUBRESOURCES, s_real(before), s_real(after));
        }
    }
    Some(out)
}

pub unsafe fn barriers(list: *mut c_void, n: u32, barriers: *const u8, real: unsafe extern "system" fn(*mut c_void, u32, *const u8)) -> bool {
    if n == 0 || barriers.is_null() || PAIRS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(true, |m| m.is_empty()) {
        return false;
    }
    let src = unsafe { core::slice::from_raw_parts(barriers, n as usize * 32) };
    static NOTES: AtomicUsize = AtomicUsize::new(0);
    let mut note = |s: String| {
        if NOTES.fetch_add(1, Ordering::Relaxed) < 8 {
            record(M, TAG, &s);
        }
    };
    let Some(steps) = plan_barriers(src, &|r| pair_of(r).map(|p| (p.shadow, p.f.planes)), &mut note) else { return false };
    let mut pending: Vec<u8> = Vec::new();
    let flush = |pending: &mut Vec<u8>| {
        if !pending.is_empty() {
            unsafe { real(list, (pending.len() / 32) as u32, pending.as_ptr()) };
            pending.clear();
        }
    };
    for step in steps {
        match step {
            Step::Barrier(b) => pending.extend_from_slice(&b),
            Step::CopyIn(d) => {
                flush(&mut pending);
                if let Some(p) = pair_of(d) {
                    unsafe { run_pass(list, &p, true) };
                }
            }
            Step::CopyOut(d) => {
                flush(&mut pending);
                if let Some(p) = pair_of(d) {
                    unsafe { run_pass(list, &p, false) };
                }
            }
        }
    }
    flush(&mut pending);
    true
}

unsafe fn run_pass(list: *mut c_void, p: &Pair, copy_in: bool) {
    let Some(ps) = PASS.get().and_then(|r| r.as_ref().ok()) else { return };
    static LOG: AtomicUsize = AtomicUsize::new(0);
    if crate::cl2_state::active_queries(list) > 0 && LOG.fetch_add(1, Ordering::Relaxed) < 4 {
        record(M, TAG, "depth/UAV conversion pass recorded while queries are active: an occlusion query would count its samples");
    }
    unsafe {
        let set_pred: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, u32) = core::mem::transmute(pc(list, PC_SET_PREDICATION));
        set_pred(list, core::ptr::null_mut(), 0, 0);
        let heaps = [ps.heap];
        let set_heaps: unsafe extern "system" fn(*mut c_void, u32, *const usize) = core::mem::transmute(pc(list, PC_HEAPS));
        set_heaps(list, 1, heaps.as_ptr());
        let table = |k: u64| ps.gpu + (p.slot as u64 * 4 + k) * ps.inc;
        let set_pso: unsafe extern "system" fn(*mut c_void, *mut c_void) = core::mem::transmute(pc(list, PC_SET_PSO));
        if copy_in {
            let set_rs: unsafe extern "system" fn(*mut c_void, *mut c_void) = core::mem::transmute(pc(list, PC_CROOT));
            let set_t: unsafe extern "system" fn(*mut c_void, u32, u64) = core::mem::transmute(pc(list, PC_CTABLE));
            let dispatch: unsafe extern "system" fn(*mut c_void, u32, u32, u32) = core::mem::transmute(pc(list, PC_DISPATCH));
            set_rs(list, ps.rs as *mut c_void);
            set_pso(list, ps.cs as *mut c_void);
            set_t(list, 0, table(0));
            dispatch(list, p.width.div_ceil(8), p.height.div_ceil(8), 1);
        } else {
            let set_rs: unsafe extern "system" fn(*mut c_void, *mut c_void) = core::mem::transmute(pc(list, PC_GROOT));
            let set_t: unsafe extern "system" fn(*mut c_void, u32, u64) = core::mem::transmute(pc(list, PC_GTABLE));
            let topo: unsafe extern "system" fn(*mut c_void, u32) = core::mem::transmute(pc(list, PC_TOPOLOGY));
            let vps: unsafe extern "system" fn(*mut c_void, u32, *const f32) = core::mem::transmute(pc(list, PC_VIEWPORTS));
            let scs: unsafe extern "system" fn(*mut c_void, u32, *const i32) = core::mem::transmute(pc(list, PC_SCISSORS));
            let targets: unsafe extern "system" fn(*mut c_void, u32, *const u64, i32, *const u64) = core::mem::transmute(pc(list, PC_SET_TARGETS));
            let draw: unsafe extern "system" fn(*mut c_void, u32, u32, u32, u32) = core::mem::transmute(pc(list, PC_DRAW));
            set_rs(list, ps.rs as *mut c_void);
            set_pso(list, ps.gfx[(p.f.planes == 2) as usize] as *mut c_void);
            set_t(list, 0, table(2));
            topo(list, 4);
            let vp = [0.0f32, 0.0, p.width as f32, p.height as f32, 0.0, 1.0];
            vps(list, 1, vp.as_ptr());
            let sc = [0i32, 0, p.width as i32, p.height as i32];
            scs(list, 1, sc.as_ptr());
            let dsv = ps.dsv_cpu + p.slot as u64 * ps.dsv_inc;
            targets(list, 0, core::ptr::null(), 0, &dsv);
            draw(list, 3, 1, 0, 0);
        }
        crate::cl2_state::reapply(list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(fmt: u32, flags: u32, mips: u16, depth: u16, samples: u32) -> [u8; 0x38] {
        let mut d = [0u8; 0x38];
        d[0..4].copy_from_slice(&3u32.to_le_bytes());
        d[0x10..0x18].copy_from_slice(&1024u64.to_le_bytes());
        d[0x18..0x1c].copy_from_slice(&1024u32.to_le_bytes());
        d[0x1c..0x1e].copy_from_slice(&depth.to_le_bytes());
        d[0x1e..0x20].copy_from_slice(&mips.to_le_bytes());
        d[0x20..0x24].copy_from_slice(&fmt.to_le_bytes());
        d[0x24..0x28].copy_from_slice(&samples.to_le_bytes());
        d[0x30..0x34].copy_from_slice(&flags.to_le_bytes());
        d
    }

    #[test]
    fn plan_pairs_supported_depth_uav_textures_only() {
        assert_eq!(plan(&tex(19, 0x6, 1, 1, 1)), Plan::Pair(Formats { dsv: 20, d_srv: 21, planes: 2 }));
        assert_eq!(plan(&tex(40, 0x1000006, 1, 1, 1)), Plan::Pair(Formats { dsv: 40, d_srv: 41, planes: 1 }));
        assert_eq!(plan(&tex(19, 0x2, 1, 1, 1)), Plan::None);
        assert_eq!(plan(&tex(19, 0x4, 1, 1, 1)), Plan::None);
        assert!(matches!(plan(&tex(45, 0x6, 1, 1, 1)), Plan::Refuse(_)));
        assert!(matches!(plan(&tex(19, 0x6, 2, 1, 1)), Plan::Refuse(_)));
        assert!(matches!(plan(&tex(19, 0x6, 1, 6, 1)), Plan::Refuse(_)));
        assert!(matches!(plan(&tex(19, 0x6, 1, 1, 4)), Plan::Refuse(_)));
    }

    #[test]
    fn depth_and_shadow_descs() {
        let t = tex(19, 0x1000006, 1, 1, 1);
        assert_eq!(u32_at(&depth_desc(&t), 0x30), 0x1000002);
        let s = shadow_desc(&t);
        assert_eq!(u32_at(&s, 0x20), FMT_R32_TYPELESS);
        assert_eq!(u32_at(&s, 0x30), FLAG_UAV);
        assert_eq!(u32_at(&s, 0x18), 1024);
    }

    #[test]
    fn shadow_uav_views_are_32_bit_single_plane() {
        let d = shadow_uav_desc(None).unwrap();
        assert_eq!((u32_at(&d, 0), u32_at(&d, 4)), (FMT_R32_FLOAT, 4));
        let mut t = [0u8; 0x28];
        t[0..4].copy_from_slice(&21u32.to_le_bytes());
        t[4..8].copy_from_slice(&4u32.to_le_bytes());
        t[12..16].copy_from_slice(&1u32.to_le_bytes());
        let d = shadow_uav_desc(Some(&t)).unwrap();
        assert_eq!((u32_at(&d, 0), u32_at(&d, 12)), (FMT_R32_FLOAT, 0));
        t[0..4].copy_from_slice(&FMT_R32_UINT.to_le_bytes());
        assert_eq!(u32_at(&shadow_uav_desc(Some(&t)).unwrap(), 0), FMT_R32_UINT);
        t[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(shadow_uav_desc(Some(&t)).is_err());
    }

    #[test]
    fn real_states_follow_the_title_state() {
        assert_eq!((d_real(ST_UAV), s_real(ST_UAV)), (D_UAV_STATE, ST_UAV));
        assert_eq!((d_real(ST_DEPTH_WRITE), s_real(ST_DEPTH_WRITE)), (ST_DEPTH_WRITE, S_READ_STATE));
        assert_eq!((d_real(0xc0), s_real(0xc0)), (0xc0, S_READ_STATE));
    }

    fn t(res: usize, sub: u32, before: u32, after: u32) -> [u8; 32] {
        transition(res, sub, before, after)
    }

    const D: usize = 0xd000;
    const S: usize = 0x5000;
    const OTHER: usize = 0x7000;

    fn run(src: &[[u8; 32]]) -> (Option<Vec<Step>>, Vec<String>) {
        let bytes: Vec<u8> = src.iter().flatten().copied().collect();
        let mut notes = Vec::new();
        let r = plan_barriers(&bytes, &|r| (r == D).then_some((S, 2)), &mut |s| notes.push(s));
        (r, notes)
    }

    #[test]
    fn barriers_without_pairs_are_left_alone() {
        assert_eq!(run(&[t(OTHER, ALL_SUBRESOURCES, 0x10, 0x8)]).0, None);
    }

    #[test]
    fn entering_uav_copies_depth_into_the_shadow() {
        let (steps, _) = run(&[t(OTHER, ALL_SUBRESOURCES, 0x4, 0x80), t(D, ALL_SUBRESOURCES, ST_DEPTH_WRITE, ST_UAV)]);
        assert_eq!(steps.unwrap(), vec![
            Step::Barrier(t(OTHER, ALL_SUBRESOURCES, 0x4, 0x80)),
            Step::Barrier(t(D, ALL_SUBRESOURCES, ST_DEPTH_WRITE, D_UAV_STATE)),
            Step::Barrier(t(S, ALL_SUBRESOURCES, S_READ_STATE, ST_UAV)),
            Step::CopyIn(D),
            Step::Barrier(uav_barrier(S)),
        ]);
    }

    #[test]
    fn leaving_uav_writes_the_shadow_back_as_depth() {
        let (steps, _) = run(&[t(D, ALL_SUBRESOURCES, ST_UAV, 0xc0)]);
        assert_eq!(steps.unwrap(), vec![
            Step::Barrier(t(S, ALL_SUBRESOURCES, ST_UAV, S_READ_STATE)),
            Step::Barrier(t(D, ALL_SUBRESOURCES, D_UAV_STATE, ST_DEPTH_WRITE)),
            Step::CopyOut(D),
            Step::Barrier(t(D, ALL_SUBRESOURCES, ST_DEPTH_WRITE, 0xc0)),
        ]);
        let (steps, _) = run(&[t(D, ALL_SUBRESOURCES, ST_UAV, ST_DEPTH_WRITE)]);
        assert_eq!(steps.unwrap().last(), Some(&Step::CopyOut(D)));
    }

    #[test]
    fn non_uav_transitions_and_uav_barriers_map_to_the_real_resources() {
        let (steps, _) = run(&[t(D, ALL_SUBRESOURCES, ST_DEPTH_WRITE, ST_DEPTH_READ), uav_barrier(D)]);
        assert_eq!(steps.unwrap(), vec![
            Step::Barrier(t(D, ALL_SUBRESOURCES, ST_DEPTH_WRITE, ST_DEPTH_READ)),
            Step::Barrier(uav_barrier(S)),
        ]);
    }

    #[test]
    fn split_begin_is_dropped_and_plane_transitions_are_noted() {
        let mut begin = t(D, ALL_SUBRESOURCES, ST_DEPTH_WRITE, ST_UAV);
        begin[4..8].copy_from_slice(&BARRIER_BEGIN_ONLY.to_le_bytes());
        assert_eq!(run(&[begin]).0.unwrap(), vec![]);
        let (steps, notes) = run(&[t(D, 1, ST_DEPTH_WRITE, ST_UAV)]);
        assert_eq!(steps.unwrap(), vec![Step::Barrier(t(D, 1, ST_DEPTH_WRITE, D_UAV_STATE))]);
        assert_eq!(notes.len(), 1);
    }

    const IID_ALLOCATOR: Guid = Guid { d1: 0x6102_dee4, d2: 0xaf59, d3: 0x4b09, d4: [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24] };
    const IID_GFX_LIST: Guid = Guid { d1: 0x5b16_0d0f, d2: 0xac1b, d3: 0x4185, d4: [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55] };
    const IID_FENCE: Guid = Guid { d1: 0x0a75_3dcf, d2: 0xc4d8, d3: 0x4b91, d4: [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76] };
    const IID_QUEUE: Guid = Guid { d1: 0x0ec8_70a6, d2: 0x5d7e, d3: 0x4c22, d4: [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed] };
    const IID_DEV: Guid = Guid { d1: 0x1898_19f1, d2: 0x1db6, d3: 0x4b57, d4: [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7] };

    macro_rules! call {
        ($o:expr, $i:expr, ($($t:ty),*) -> $r:ty $(, $a:expr)*) => {{
            let o = $o as *mut c_void;
            let f: unsafe extern "system" fn(*mut c_void $(, $t)*) -> $r = core::mem::transmute(vslot(o, $i));
            f(o $(, $a)*)
        }};
    }

    struct Gpu {
        dev: *mut c_void,
        queue: *mut c_void,
        alloc: *mut c_void,
        list: *mut c_void,
        fence: *mut c_void,
        value: u64,
    }

    impl Gpu {
        unsafe fn new() -> Gpu {
            let p = crate::host_proc(b"D3D12CreateDevice\0");
            assert!(p != 0);
            let mk: unsafe extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(p);
            if std::env::var("TOMBOX_D3D12_DEBUG").is_ok() {
                let get: unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(crate::host_proc(b"D3D12GetDebugInterface\0"));
                let mut dbg: *mut c_void = core::ptr::null_mut();
                assert!(get(&crate::IID_ID3D12DEBUG, &mut dbg) >= 0 && !dbg.is_null());
                call!(dbg, 3, () -> ());
                release(dbg);
            }
            let mut g = Gpu { dev: core::ptr::null_mut(), queue: core::ptr::null_mut(), alloc: core::ptr::null_mut(), list: core::ptr::null_mut(), fence: core::ptr::null_mut(), value: 0 };
            assert!(mk(core::ptr::null_mut(), 0xc000, &IID_DEV, &mut g.dev) >= 0);
            let qd = [0u32; 4];
            assert!(call!(g.dev, 8, (*const u32, *const Guid, *mut *mut c_void) -> i32, qd.as_ptr(), &IID_QUEUE, &mut g.queue) >= 0);
            assert!(call!(g.dev, 9, (u32, *const Guid, *mut *mut c_void) -> i32, 0, &IID_ALLOCATOR, &mut g.alloc) >= 0);
            assert!(call!(g.dev, 12, (u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32, 0, 0, g.alloc, core::ptr::null_mut(), &IID_GFX_LIST, &mut g.list) >= 0);
            assert!(call!(g.dev, 36, (u64, u32, *const Guid, *mut *mut c_void) -> i32, 0, 0, &IID_FENCE, &mut g.fence) >= 0);
            call!(g.list, 9, () -> i32);
            g
        }

        unsafe fn run(&mut self, rec: impl FnOnce(*mut c_void)) {
            call!(self.alloc, 8, () -> i32);
            call!(self.list, 10, (*mut c_void, *mut c_void) -> i32, self.alloc, core::ptr::null_mut());
            rec(self.list);
            assert!(call!(self.list, 9, () -> i32) >= 0, "Close failed");
            let l = [self.list];
            call!(self.queue, 10, (u32, *const *mut c_void) -> (), 1, l.as_ptr());
            self.value += 1;
            call!(self.queue, 14, (*mut c_void, u64) -> i32, self.fence, self.value);
            let mut n = 0;
            while call!(self.fence, 8, () -> u64) < self.value {
                std::thread::sleep(std::time::Duration::from_millis(1));
                n += 1;
                assert!(n < 5000, "GPU timeout");
            }
            let removed = call!(self.dev, 37, () -> i32);
            assert_eq!(removed, 0, "device removed: {removed:#010x}");
        }

        unsafe fn buffer(&self, heap_type: u32, size: u64, state: u32) -> *mut c_void {
            let heap: [u32; 5] = [heap_type, 0, 0, 1, 1];
            let mut d = [0u8; 0x38];
            d[0..4].copy_from_slice(&1u32.to_le_bytes());
            d[0x10..0x18].copy_from_slice(&size.to_le_bytes());
            d[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
            d[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
            d[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
            d[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
            d[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
            let mut r: *mut c_void = core::ptr::null_mut();
            assert!(call!(self.dev, 27, (*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32, heap.as_ptr(), 0, d.as_ptr(), state, core::ptr::null(), &IID_RESOURCE, &mut r) >= 0);
            r
        }

        unsafe fn footprint(&self, desc: &[u8; 0x38]) -> ([u32; 5], u64) {
            let mut layout = [0u8; 32];
            let (mut rows, mut row_size, mut total) = (0u32, 0u64, 0u64);
            call!(self.dev, 38, (*const u8, u32, u32, u64, *mut u8, *mut u32, *mut u64, *mut u64) -> (), desc.as_ptr(), 0, 1, 0, layout.as_mut_ptr(), &mut rows, &mut row_size, &mut total);
            (core::array::from_fn(|i| u32_at(&layout, 8 + i * 4)), total)
        }
    }

    fn sub_loc(res: *mut c_void) -> [u8; 48] {
        let mut l = [0u8; 48];
        l[0..8].copy_from_slice(&(res as u64).to_le_bytes());
        l
    }

    fn fp_loc(res: *mut c_void, fp: &[u32; 5]) -> [u8; 48] {
        let mut l = [0u8; 48];
        l[0..8].copy_from_slice(&(res as u64).to_le_bytes());
        l[8..12].copy_from_slice(&1u32.to_le_bytes());
        for (i, w) in fp.iter().enumerate() {
            l[24 + i * 4..28 + i * 4].copy_from_slice(&w.to_le_bytes());
        }
        l
    }

    unsafe fn round_trip(g: &mut Gpu, fmt: u32) {
        const N: u32 = 64;
        let mut desc = tex(fmt, FLAG_DSV | FLAG_UAV, 1, 1, 1);
        desc[0x10..0x18].copy_from_slice(&(N as u64).to_le_bytes());
        desc[0x18..0x1c].copy_from_slice(&N.to_le_bytes());
        let Plan::Pair(f) = plan(&desc) else { panic!("format {fmt} not paired") };
        let dd = depth_desc(&desc);
        let heap = [1u32, 0, 0, 1, 1];
        let mut d: *mut c_void = core::ptr::null_mut();
        assert!(call!(g.dev, 27, (*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32, heap.as_ptr(), 0, dd.as_ptr(), ST_DEPTH_WRITE, core::ptr::null(), &IID_RESOURCE, &mut d) >= 0);
        register(g.dev, d, &dd, &heap, ST_DEPTH_WRITE, f).expect("register pair");
        let p = pair_of(d as usize).unwrap();
        let s = p.shadow as *mut c_void;
        let ps = PASS.get().unwrap().as_ref().unwrap();
        let dsv = ps.dsv_cpu + p.slot as u64 * ps.dsv_inc;
        let (fp_s, total_s) = g.footprint(&shadow_desc(&dd));
        let (fp_d, total_d) = g.footprint(&dd);
        let rb_s = g.buffer(3, total_s, 0x400);
        let rb_d = g.buffer(3, total_d, 0x400);
        let up = g.buffer(2, total_s, 0xac3);
        let mut m: *mut u8 = core::ptr::null_mut();
        assert!(call!(up, 8, (u32, *const c_void, *mut *mut u8) -> i32, 0, core::ptr::null(), &mut m) >= 0);
        for y in 0..N as usize {
            for x in 0..N as usize {
                core::ptr::write_unaligned(m.add(y * fp_s[4] as usize + x * 4) as *mut u32, 0.75f32.to_bits());
            }
        }
        let real: unsafe extern "system" fn(*mut c_void, u32, *const u8) = core::mem::transmute(vslot(g.list, 26));
        let clear_flags = if f.planes == 2 { 3 } else { 1 };
        g.run(|l| {
            call!(l, 47, (u64, u32, f32, u8, u32, *const u8) -> (), dsv, clear_flags, 0.25, 0, 0, core::ptr::null());
            assert!(barriers(l, 1, transition(d as usize, ALL_SUBRESOURCES, ST_DEPTH_WRITE, ST_UAV).as_ptr(), real));
            real(l, 1, transition(s as usize, ALL_SUBRESOURCES, ST_UAV, 0x800).as_ptr());
            call!(l, 16, (*const u8, u32, u32, u32, *const u8, *const u8) -> (), fp_loc(rb_s, &fp_s).as_ptr(), 0, 0, 0, sub_loc(s).as_ptr(), core::ptr::null());
            real(l, 1, transition(s as usize, ALL_SUBRESOURCES, 0x800, 0x400).as_ptr());
            call!(l, 16, (*const u8, u32, u32, u32, *const u8, *const u8) -> (), sub_loc(s).as_ptr(), 0, 0, 0, fp_loc(up, &fp_s).as_ptr(), core::ptr::null());
            real(l, 1, transition(s as usize, ALL_SUBRESOURCES, 0x400, ST_UAV).as_ptr());
            assert!(barriers(l, 1, transition(d as usize, ALL_SUBRESOURCES, ST_UAV, S_READ_STATE).as_ptr(), real));
            assert!(barriers(l, 1, transition(d as usize, ALL_SUBRESOURCES, S_READ_STATE, 0x800).as_ptr(), real));
            call!(l, 16, (*const u8, u32, u32, u32, *const u8, *const u8) -> (), fp_loc(rb_d, &fp_d).as_ptr(), 0, 0, 0, sub_loc(d).as_ptr(), core::ptr::null());
        });
        let read = |rb: *mut c_void, pitch: u32| -> Vec<u32> {
            let mut m: *mut u8 = core::ptr::null_mut();
            assert!(call!(rb, 8, (u32, *const c_void, *mut *mut u8) -> i32, 0, core::ptr::null(), &mut m) >= 0);
            (0..N as usize).flat_map(|y| (0..N as usize).map(move |x| (y, x))).map(|(y, x)| core::ptr::read_unaligned(m.add(y * pitch as usize + x * 4) as *const u32)).collect()
        };
        let in_s = read(rb_s, fp_s[4]);
        assert!(in_s.iter().all(|&v| v == 0.25f32.to_bits()), "fmt {fmt}: copy-in gave {:#x}", in_s[0]);
        let in_d = read(rb_d, fp_d[4]);
        assert!(in_d.iter().all(|&v| v == 0.75f32.to_bits()), "fmt {fmt}: write-back gave {:#x}", in_d[0]);
        for o in [rb_s, rb_d, up, d] {
            release(o);
        }
    }

    #[test]
    fn depth_round_trips_through_the_uav_shadow_on_a_real_device() {
        if !tombox_xshader::dxc::available() {
            eprintln!("skipped: DXC unavailable");
            return;
        }
        let mut g = unsafe { Gpu::new() };
        for fmt in [40, 20] {
            unsafe { round_trip(&mut g, fmt) };
        }
        unsafe { crate::cmdlist_x::dump_device_messages(g.dev, TAG) };
    }

    #[test]
    fn heap_start_accepts_both_return_conventions() {
        assert_eq!(heap_start_result(0x1000, 0x1000, 0xabc0), 0xabc0);
        assert_eq!(heap_start_result(0xabc0, 0x1000, 0), 0xabc0);
    }

    #[test]
    fn write_back_pso_desc_layout() {
        let vs = [1u8; 4];
        let ps = [2u8; 8];
        let d = gfx_desc(0x1234, &vs, &ps, 20);
        assert_eq!(d.len(), 0x290);
        assert_eq!(u64::from_le_bytes(d[0..8].try_into().unwrap()), 0x1234);
        assert_eq!(u64::from_le_bytes(d[0x20..0x28].try_into().unwrap()), 8);
        assert_eq!(u32_at(&d, 0x1f8), 8);
        assert_eq!(u32_at(&d, 0x264), 20);
    }
}
