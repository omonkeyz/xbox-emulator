
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use compat_common::record;
use tombox_xshader::rt_cps::{self, ShaderId, XDXR_INTERNAL_SPACE};
use tombox_xshader::state_obj;

use crate::device_wrap::{real_device, Guid};
use crate::global_bind::GlobalWindow;
use crate::rt_bvh::{
    self, Aabb, InstanceInput, PrebuildInfo, TriangleInput, AS_TYPE_BOTTOM_LEVEL, AS_TYPE_TOP_LEVEL,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R32G32B32_FLOAT,
    DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32_UINT, GEOM_TRIANGLES,
};

const M: &str = "d3d12_xs";
const TAG: &str = "rt_x";

const S_OK: i32 = 0;
const E_POINTER: i32 = 0x8000_4003u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const E_FAIL: i32 = 0x8000_4005u32 as i32;

pub const DEV_SLOT_CREATE_STATE_OBJECT: usize = 101;
pub const DEV_SLOT_GET_PREBUILD_INFO: usize = 102;
pub const DEV_SLOT_DESERIALIZE_STATE_OBJECT_X: usize = 115;

pub const CL_SLOT_BUILD_AS: usize = 117;
pub const CL_SLOT_EMIT_POSTBUILD_INFO: usize = 118;
pub const CL_SLOT_COPY_AS: usize = 119;
pub const CL_SLOT_SET_PIPELINE_STATE1: usize = 120;

const BUILD_FLAG_ALLOW_COMPACTION: u32 = 0x2;
const COPY_MODE_CLONE: u32 = 0;
const COPY_MODE_COMPACT: u32 = 1;
const COPY_MODE_DESERIALIZE: u32 = 4;
const SERIALIZED_AS_HEADER_SIZE: u64 = 56;
const POSTBUILD_COMPACTED_SIZE: u32 = 0;
const POSTBUILD_CURRENT_SIZE: u32 = 3;
pub const CL_SLOT_DISPATCH_RAYS: usize = 121;
pub const CL_SLOT_BUILD_AS_BATCHED_X: usize = 131;

pub const D3D12_FEATURE_OPTIONS5: u32 = 27;
pub const RAYTRACING_TIER_NOT_SUPPORTED: u32 = 0;
pub const RAYTRACING_TIER_1_0: u32 = 10;
pub const RAYTRACING_TIER_1_1: u32 = 20;

pub const IID_XBOX_DEVICE8: Guid = Guid {
    d1: 0x728f_1fa2,
    d2: 0xea11,
    d3: 0x4a9f,
    d4: [0x9b, 0x52, 0x9b, 0x42, 0xa4, 0x84, 0xd5, 0xc9],
};

const IID_IUNKNOWN: Guid = Guid {
    d1: 0,
    d2: 0,
    d3: 0,
    d4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46],
};

pub const IID_ID3D12_STATE_OBJECT: Guid = Guid {
    d1: 0x4701_6943,
    d2: 0xfca8,
    d3: 0x4594,
    d4: [0x93, 0xea, 0xaf, 0x25, 0x8b, 0x55, 0x34, 0x6d],
};

pub const IID_ID3D12_STATE_OBJECT_PROPERTIES: Guid = Guid {
    d1: 0xde5f_a827,
    d2: 0x9bf9,
    d3: 0x4f26,
    d4: [0x89, 0xff, 0xd7, 0xf5, 0x6f, 0xde, 0x38, 0x60],
};

const IID_ROOT_SIGNATURE: Guid = Guid {
    d1: 0xc54a_6b66,
    d2: 0x72df,
    d3: 0x4ee8,
    d4: [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14],
};
const IID_PSO: Guid = Guid {
    d1: 0x765a_30f3,
    d2: 0xf624,
    d3: 0x4c6f,
    d4: [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45],
};
const IID_DEVICE: Guid = Guid {
    d1: 0x1898_19f1,
    d2: 0x1db6,
    d3: 0x4b57,
    d4: [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7],
};
const IID_RESOURCE: Guid = Guid {
    d1: 0x6964_42be,
    d2: 0xa72e,
    d3: 0x4059,
    d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad],
};
const IID_FENCE: Guid = Guid {
    d1: 0x0a75_3dcf,
    d2: 0xc4d8,
    d3: 0x4b91,
    d4: [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76],
};

const PC_CREATE_COMPUTE_PSO: usize = 11;
const PC_CREATE_ROOT_SIGNATURE: usize = 16;
const PC_CREATE_COMMITTED: usize = 27;
const PC_CREATE_FENCE: usize = 36;
const PC_CL_DISPATCH: usize = 14;
const PC_CL_COPY_BUFFER_REGION: usize = 15;
const PC_CL_SET_PSO: usize = 25;
const PC_CL_RESOURCE_BARRIER: usize = 26;
const PC_CL_SET_COMPUTE_RS: usize = 29;
const PC_CL_SET_COMPUTE_CONSTS: usize = 35;

pub fn parse_raytracing_tier(val: Option<&str>, global_enabled: bool) -> u32 {
    if !global_enabled {
        return RAYTRACING_TIER_NOT_SUPPORTED;
    }
    match val.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("0" | "off" | "false" | "none") => RAYTRACING_TIER_NOT_SUPPORTED,
        Some("10" | "1.0" | "tier1_0") => RAYTRACING_TIER_1_0,
        Some("20" | "1.1" | "tier1_1") | None => RAYTRACING_TIER_1_1,
        _ => RAYTRACING_TIER_1_1,
    }
}

pub fn configured_raytracing_tier() -> u32 {
    static TIER: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *TIER.get_or_init(|| {
        let env = std::env::var("TOMBOX_RAYTRACING_TIER").ok();
        let t = parse_raytracing_tier(env.as_deref(), crate::global_bind::enabled());
        record(M, TAG, &format!("configured D3D12 RaytracingTier={t} (env={env:?})"));
        t
    })
}

pub unsafe fn patch_options5(pdata: *mut c_void, size: u32) {
    if !pdata.is_null() && size >= 20 {
        let tier = configured_raytracing_tier();
        unsafe { core::ptr::write_unaligned((pdata as *mut u8).add(16) as *mut u32, tier) };
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct DispatchRaysDesc {
    pub raygen_va: u64,
    pub raygen_size: u64,
    pub miss_va: u64,
    pub miss_size: u64,
    pub miss_stride: u64,
    pub hitgroup_va: u64,
    pub hitgroup_size: u64,
    pub hitgroup_stride: u64,
    pub callable_va: u64,
    pub callable_size: u64,
    pub callable_stride: u64,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

impl DispatchRaysDesc {
    pub const BYTE_SIZE: usize = 104;

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < Self::BYTE_SIZE {
            return None;
        }
        let q = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        Some(Self {
            raygen_va: q(0x00),
            raygen_size: q(0x08),
            miss_va: q(0x10),
            miss_size: q(0x18),
            miss_stride: q(0x20),
            hitgroup_va: q(0x28),
            hitgroup_size: q(0x30),
            hitgroup_stride: q(0x38),
            callable_va: q(0x40),
            callable_size: q(0x48),
            callable_stride: q(0x50),
            width: u(0x58),
            height: u(0x5c),
            depth: u(0x60),
        })
    }

    pub fn dispatch_groups(&self) -> (u32, u32, u32) {
        (self.width.div_ceil(8), self.height.div_ceil(8), self.depth.max(1))
    }

    pub fn dispatch_constants(&self) -> [u32; 13] {
        [
            self.raygen_va as u32,
            (self.raygen_va >> 32) as u32,
            self.miss_va as u32,
            (self.miss_va >> 32) as u32,
            self.miss_size as u32,
            self.miss_stride as u32,
            self.hitgroup_va as u32,
            (self.hitgroup_va >> 32) as u32,
            self.hitgroup_size as u32,
            self.hitgroup_stride as u32,
            self.width,
            self.height,
            self.depth.max(1),
        ]
    }
}

#[repr(C)]
struct RtBlob {
    vtable: *const usize,
    refs: AtomicU32,
    data: Vec<u8>,
}

unsafe extern "system" fn blob_qi(this: *mut RtBlob, riid: *const Guid, ppv: *mut *mut c_void) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return E_POINTER;
    }
    unsafe {
        (*this).refs.fetch_add(1, Ordering::SeqCst);
        *ppv = this as *mut c_void;
    }
    S_OK
}
unsafe extern "system" fn blob_addref(this: *mut RtBlob) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::SeqCst) + 1 }
}
unsafe extern "system" fn blob_release(this: *mut RtBlob) -> u32 {
    let n = unsafe { (*this).refs.fetch_sub(1, Ordering::SeqCst) } - 1;
    if n == 0 {
        drop(unsafe { Box::from_raw(this) });
    }
    n
}
unsafe extern "system" fn blob_ptr(this: *mut RtBlob) -> *mut c_void {
    unsafe { (*this).data.as_mut_ptr() as *mut c_void }
}
unsafe extern "system" fn blob_size(this: *mut RtBlob) -> usize {
    unsafe { (*this).data.len() }
}

fn blob_vtable() -> *const usize {
    static VT: std::sync::OnceLock<[usize; 5]> = std::sync::OnceLock::new();
    VT.get_or_init(|| [
        blob_qi as usize,
        blob_addref as usize,
        blob_release as usize,
        blob_ptr as usize,
        blob_size as usize,
    ])
    .as_ptr()
}

fn alloc_blob(data: Vec<u8>) -> *mut RtBlob {
    Box::into_raw(Box::new(RtBlob {
        vtable: blob_vtable(),
        refs: AtomicU32::new(1),
        data,
    }))
}

struct CompiledProgram {
    root_sig: usize,
    pso: usize,
    b0_param_idx: Option<u32>,
    b1_param_idx: Option<u32>,
    exports: Vec<String>,
}

impl Drop for CompiledProgram {
    fn drop(&mut self) {
        unsafe {
            if self.pso != 0 {
                com_release(self.pso as *mut c_void);
            }
            if self.root_sig != 0 {
                com_release(self.root_sig as *mut c_void);
            }
        }
    }
}

struct StateObjectData {
    real_device: usize,
    exports: HashMap<String, Box<ShaderId>>,
    programs: Arc<ProgramBuild>,
    pipeline_stack_size: AtomicU64,
    sym_records: Vec<[u32; 6]>,
    stats_blob: *mut RtBlob,
}

#[repr(C)]
struct RtStateObject {
    vtable: *const usize,
    props_vtable: *const usize,
    meta_vtable: *const usize,
    refs: AtomicU32,
    data: Box<StateObjectData>,
}

#[derive(Clone, Debug)]
struct AsRecord {
    va: u64,
    size: u64,
    res: usize,
    as_type: u32,
    bounds: Aabb,
    blas_vas: Vec<u64>,
    pending_copy: Option<std::sync::Arc<Vec<u8>>>,
}

static AS_REGISTRY: Mutex<Option<HashMap<u64, AsRecord>>> = Mutex::new(None);
static ACTIVE_SO: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);
static LOG_DISPATCH: AtomicUsize = AtomicUsize::new(0);
static LOG_BUILD: AtomicUsize = AtomicUsize::new(0);

fn once(c: &AtomicUsize, n: usize) -> bool {
    c.fetch_add(1, Ordering::Relaxed) < n
}

unsafe fn vfn(obj: *mut c_void, slot: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(slot) }
}

unsafe fn com_release(obj: *mut c_void) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(vfn(obj, 2)) };
        unsafe { f(obj) };
    }
}


unsafe extern "system" fn so_qi(this: *mut RtStateObject, riid: *const Guid, ppv: *mut *mut c_void) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return E_POINTER;
    }
    unsafe { *ppv = core::ptr::null_mut() };
    let g = unsafe { &*riid };
    if *g == IID_IUNKNOWN || *g == IID_ID3D12_STATE_OBJECT {
        unsafe {
            (*this).refs.fetch_add(1, Ordering::SeqCst);
            *ppv = this as *mut c_void;
        }
        return S_OK;
    }
    if *g == IID_ID3D12_STATE_OBJECT_PROPERTIES {
        unsafe {
            (*this).refs.fetch_add(1, Ordering::SeqCst);
            *ppv = core::ptr::addr_of_mut!((*this).props_vtable) as *mut c_void;
        }
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn so_addref(this: *mut RtStateObject) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::SeqCst) + 1 }
}

unsafe extern "system" fn so_release(this: *mut RtStateObject) -> u32 {
    let n = unsafe { (*this).refs.fetch_sub(1, Ordering::SeqCst) } - 1;
    if n == 0 {
        let obj = unsafe { Box::from_raw(this) };
        if !obj.data.stats_blob.is_null() {
            unsafe { blob_release(obj.data.stats_blob) };
        }
    }
    n
}

unsafe extern "system" fn so_get_private_data(_this: *mut RtStateObject, _g: *const Guid, _sz: *mut u32, _out: *mut c_void) -> i32 {
    E_FAIL
}
unsafe extern "system" fn so_set_private_data(_this: *mut RtStateObject, _g: *const Guid, _sz: u32, _data: *const c_void) -> i32 {
    S_OK
}
unsafe extern "system" fn so_set_private_iface(_this: *mut RtStateObject, _g: *const Guid, _unk: *mut c_void) -> i32 {
    S_OK
}
unsafe extern "system" fn so_set_name(_this: *mut RtStateObject, _name: *const u16) -> i32 {
    S_OK
}
unsafe extern "system" fn so_get_device(this: *mut RtStateObject, riid: *const Guid, ppv: *mut *mut c_void) -> i32 {
    let dev = unsafe { (*this).data.real_device } as *mut c_void;
    if dev.is_null() {
        return E_FAIL;
    }
    let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vfn(dev, 0)) };
    unsafe { qi(dev, riid, ppv) }
}

unsafe extern "system" fn so_get_meta_x(this: *mut RtStateObject, out_meta: *mut *mut c_void) -> i32 {
    if out_meta.is_null() {
        return E_POINTER;
    }
    unsafe { *out_meta = core::ptr::addr_of_mut!((*this).meta_vtable) as *mut c_void };
    S_OK
}

fn so_vtable() -> *const usize {
    static VT: std::sync::OnceLock<[usize; 9]> = std::sync::OnceLock::new();
    VT.get_or_init(|| [
        so_qi as usize,
        so_addref as usize,
        so_release as usize,
        so_get_private_data as usize,
        so_set_private_data as usize,
        so_set_private_iface as usize,
        so_set_name as usize,
        so_get_device as usize,
        so_get_meta_x as usize,
    ])
    .as_ptr()
}


unsafe fn meta_to_so(meta: *mut c_void) -> *mut RtStateObject {
    unsafe { (meta as *mut u8).sub(2 * core::mem::size_of::<*const usize>()) as *mut RtStateObject }
}

unsafe extern "system" fn meta_qi(this: *mut c_void, riid: *const Guid, ppv: *mut *mut c_void) -> i32 {
    unsafe { so_qi(meta_to_so(this), riid, ppv) }
}
unsafe extern "system" fn meta_addref(this: *mut c_void) -> u32 {
    unsafe { so_addref(meta_to_so(this)) }
}
unsafe extern "system" fn meta_release(this: *mut c_void) -> u32 {
    unsafe { so_release(meta_to_so(this)) }
}
unsafe extern "system" fn meta_noop(_this: *mut c_void) -> i32 {
    S_OK
}

unsafe extern "system" fn meta_get_blob_x(this: *mut c_void, _kind: u32, _idx: u32, pp_blob: *mut *mut c_void) -> i32 {
    if pp_blob.is_null() {
        return E_POINTER;
    }
    let so = unsafe { &*meta_to_so(this) };
    let mut hdr = vec![0u8; 32];
    let ptr = if so.data.sym_records.is_empty() { 0u64 } else { so.data.sym_records.as_ptr() as u64 };
    hdr[0..8].copy_from_slice(&ptr.to_le_bytes());
    hdr[16..20].copy_from_slice(&(so.data.sym_records.len() as u32).to_le_bytes());
    unsafe { *pp_blob = alloc_blob(hdr) as *mut c_void };
    S_OK
}

fn meta_vtable() -> *const usize {
    static VT: std::sync::OnceLock<[usize; 10]> = std::sync::OnceLock::new();
    VT.get_or_init(|| [
        meta_qi as usize,
        meta_addref as usize,
        meta_release as usize,
        meta_noop as usize,
        meta_noop as usize,
        meta_noop as usize,
        meta_noop as usize,
        meta_noop as usize,
        meta_noop as usize,
        meta_get_blob_x as usize,
    ])
    .as_ptr()
}


unsafe fn props_to_so(props: *mut c_void) -> *mut RtStateObject {
    unsafe { (props as *mut u8).sub(core::mem::size_of::<*const usize>()) as *mut RtStateObject }
}

unsafe extern "system" fn props_qi(this: *mut c_void, riid: *const Guid, ppv: *mut *mut c_void) -> i32 {
    unsafe { so_qi(props_to_so(this), riid, ppv) }
}
unsafe extern "system" fn props_addref(this: *mut c_void) -> u32 {
    unsafe { so_addref(props_to_so(this)) }
}
unsafe extern "system" fn props_release(this: *mut c_void) -> u32 {
    unsafe { so_release(props_to_so(this)) }
}

unsafe fn utf16_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while len < 512 && unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    let slice = unsafe { core::slice::from_raw_parts(p, len) };
    String::from_utf16_lossy(slice)
}

unsafe extern "system" fn props_get_shader_id(this: *mut c_void, export_name: *const u16) -> *mut c_void {
    let so = unsafe { &mut *props_to_so(this) };
    let name = unsafe { utf16_to_string(export_name) };
    if name.is_empty() {
        return core::ptr::null_mut();
    }
    if let Some(id) = so.data.exports.get(&name) {
        return id.as_ref() as *const ShaderId as *mut c_void;
    }
    let id = Box::new(ShaderId::new(0, 1, rt_cps::fnv1a_64(&name)));
    let ptr = id.as_ref() as *const ShaderId as *mut c_void;
    so.data.exports.insert(name, id);
    ptr
}

unsafe extern "system" fn props_get_shader_stack_size(_this: *mut c_void, _export_name: *const u16) -> u64 {
    256
}

unsafe extern "system" fn props_get_pipeline_stack_size(this: *mut c_void) -> u64 {
    unsafe { (*props_to_so(this)).data.pipeline_stack_size.load(Ordering::Relaxed) }
}

unsafe extern "system" fn props_set_pipeline_stack_size(this: *mut c_void, size: u64) {
    unsafe { (*props_to_so(this)).data.pipeline_stack_size.store(size, Ordering::Relaxed) };
}

unsafe extern "system" fn props_get_stats_x(this: *mut c_void, kind: u32, out16: *mut u8) -> i32 {
    if out16.is_null() {
        return E_POINTER;
    }
    let so = unsafe { &*props_to_so(this) };
    unsafe { core::ptr::write_bytes(out16, 0, 16) };
    if kind == 1 {
        unsafe { core::ptr::write_unaligned(out16 as *mut u64, so.data.stats_blob as u64) };
    }
    S_OK
}

fn props_vtable() -> *const usize {
    static VT: std::sync::OnceLock<[usize; 8]> = std::sync::OnceLock::new();
    VT.get_or_init(|| [
        props_qi as usize,
        props_addref as usize,
        props_release as usize,
        props_get_shader_id as usize,
        props_get_shader_stack_size as usize,
        props_get_pipeline_stack_size as usize,
        props_set_pipeline_stack_size as usize,
        props_get_stats_x as usize,
    ])
    .as_ptr()
}

fn alloc_state_object(
    real_dev: *mut c_void,
    exports: HashMap<String, Box<ShaderId>>,
    programs: Arc<ProgramBuild>,
    sym_records: Vec<[u32; 6]>,
) -> *mut c_void {
    let mut sbuf = vec![0u8; 32];
    let count = if sym_records.is_empty() { 0u32 } else { 1u32 };
    sbuf[8..12].copy_from_slice(&count.to_le_bytes());
    let stats_blob = alloc_blob(sbuf);
    let obj = Box::new(RtStateObject {
        vtable: so_vtable(),
        props_vtable: props_vtable(),
        meta_vtable: meta_vtable(),
        refs: AtomicU32::new(1),
        data: Box::new(StateObjectData {
            real_device: real_dev as usize,
            exports,
            programs,
            pipeline_stack_size: AtomicU64::new(4096),
            sym_records,
            stats_blob,
        }),
    });
    Box::into_raw(obj) as *mut c_void
}

unsafe fn create_rt_root_signature(
    real_dev: *mut c_void,
    prog: &state_obj::StateObjectProgram,
) -> Result<(*mut c_void, Option<u32>, Option<u32>), String> {
    if real_dev.is_null() {
        return Err("null real device".into());
    }
    let mut b0_idx = None;
    let mut b1_idx = None;
    for (pi, p) in prog.root_sig.params.iter().enumerate() {
        if let tombox_xshader::meta::RootParamKind::Constants { register, space, .. } = p.kind {
            if space == XDXR_INTERNAL_SPACE {
                if register == 0 {
                    b0_idx = Some(pi as u32);
                } else if register == 1 {
                    b1_idx = Some(pi as u32);
                }
            }
        }
    }
    let stripped = unsafe { crate::rs_xbox::pc_blob_raw(prog.root_sig_dxbc.as_ptr(), prog.root_sig_dxbc.len()) };
    let (base_ptr, base_len) = stripped
        .as_ref()
        .map_or((prog.root_sig_dxbc.as_ptr(), prog.root_sig_dxbc.len()), |b| (b.as_ptr(), b.len()));
    let patched = unsafe { crate::global_bind::patch_root_signature(base_ptr, base_len) };
    let (final_ptr, final_len, gbl_idx) = match &patched {
        Some((b, idx)) => (b.as_ptr(), b.len(), Some(*idx)),
        None => (base_ptr, base_len, None),
    };
    let mk_rs: unsafe extern "system" fn(*mut c_void, u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(vfn(real_dev, PC_CREATE_ROOT_SIGNATURE)) };
    let mut rs: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk_rs(real_dev, 0, final_ptr, final_len, &IID_ROOT_SIGNATURE, &mut rs) };
    if hr < 0 || rs.is_null() {
        return Err(format!("CreateRootSignature hr={hr:#010x}"));
    }
    if let Some(idx) = gbl_idx {
        crate::global_bind::note_root_signature(rs, idx);
    }
    Ok((rs, b0_idx, b1_idx))
}

unsafe fn create_compute_pso(real_dev: *mut c_void, rs: *mut c_void, dxil: &[u8]) -> Result<*mut c_void, String> {
    if real_dev.is_null() || rs.is_null() || dxil.is_empty() {
        return Err("invalid args".into());
    }
    let mut desc = [0u8; 56];
    desc[0..8].copy_from_slice(&(rs as u64).to_le_bytes());
    desc[8..16].copy_from_slice(&(dxil.as_ptr() as u64).to_le_bytes());
    desc[16..24].copy_from_slice(&(dxil.len() as u64).to_le_bytes());
    let mk: unsafe extern "system" fn(*mut c_void, *const u8, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(vfn(real_dev, PC_CREATE_COMPUTE_PSO)) };
    let mut pso: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk(real_dev, desc.as_ptr(), &IID_PSO, &mut pso) };
    if hr < 0 || pso.is_null() {
        Err(format!("CreateComputePipelineState hr={hr:#010x}"))
    } else {
        Ok(pso)
    }
}

fn title_cbv_bindings(rs: &tombox_xshader::meta::RootSig) -> Vec<(u32, u32)> {
    use tombox_xshader::meta::RootParamKind;
    let mut out = Vec::new();
    for p in &rs.params {
        match &p.kind {
            RootParamKind::Descriptor { kind: 2, register, space } if *space != XDXR_INTERNAL_SPACE => out.push((*register, *space)),
            RootParamKind::Table(ranges) => {
                out.extend(ranges.iter().filter(|r| r.range_type == 2 && r.space != XDXR_INTERNAL_SPACE).map(|r| (r.base_register, r.space)))
            }
            _ => {}
        }
    }
    out
}

fn reads_title_cbv(hlsl: &str) -> bool {
    hlsl.lines()
        .filter(|l| !l.trim_start().starts_with("cbuffer "))
        .any(|l| l.contains("tb_rtcb_") || l.contains("tb_rttb_"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BuiltProgram {
    root_sig: usize,
    pso: usize,
    b0_idx: Option<u32>,
    b1_idx: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default)]
struct BuildTimes {
    translate_ms: f64,
    dxil_ms: f64,
    pso_ms: f64,
    from_pack: bool,
    reused: bool,
}

struct ProgramResult {
    exports: Vec<(String, ShaderId)>,
    built: Result<BuiltProgram, String>,
    translated: bool,
    times: BuildTimes,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct PsoKey {
    device: usize,
    hlsl: u128,
    rts0: Vec<u8>,
}

static PSO_CACHE: Mutex<Option<HashMap<PsoKey, BuiltProgram>>> = Mutex::new(None);
static INFLIGHT_PSO: Mutex<Option<HashMap<PsoKey, Arc<InflightSlot>>>> = Mutex::new(None);
static LOG_ASYNC_WAIT: AtomicUsize = AtomicUsize::new(0);
static NEXT_BUILD_ID: AtomicU64 = AtomicU64::new(1);

struct InflightSlot {
    done: Mutex<Option<Result<BuiltProgram, String>>>,
    cv: Condvar,
}

struct InflightGuard {
    key: PsoKey,
    slot: Arc<InflightSlot>,
    finished: bool,
}

impl InflightGuard {
    fn complete(mut self, res: Result<BuiltProgram, String>) -> Result<BuiltProgram, String> {
        self.finished = true;
        let stored = match res {
            Ok(b) => Ok(remember_pso(self.key.clone(), b)),
            Err(e) => Err(e),
        };
        {
            let mut g = INFLIGHT_PSO.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(m) = g.as_mut() {
                m.remove(&self.key);
            }
        }
        {
            let mut g = self.slot.done.lock().unwrap_or_else(|e| e.into_inner());
            *g = Some(stored.clone());
            self.slot.cv.notify_all();
        }
        stored
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        {
            let mut g = INFLIGHT_PSO.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(m) = g.as_mut() {
                m.remove(&self.key);
            }
        }
        let mut g = self.slot.done.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(Err("PSO build aborted".into()));
        self.slot.cv.notify_all();
    }
}

enum PsoClaim {
    Cached(BuiltProgram),
    Wait(Arc<InflightSlot>),
    Build(InflightGuard),
}

fn claim_pso(key: PsoKey) -> PsoClaim {
    let mut g = INFLIGHT_PSO.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(b) = cached_pso(&key) {
        return PsoClaim::Cached(b);
    }
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(slot) = map.get(&key) {
        return PsoClaim::Wait(Arc::clone(slot));
    }
    let slot = Arc::new(InflightSlot { done: Mutex::new(None), cv: Condvar::new() });
    map.insert(key.clone(), Arc::clone(&slot));
    PsoClaim::Build(InflightGuard { key, slot, finished: false })
}

fn wait_inflight_pso(slot: &InflightSlot) -> Result<BuiltProgram, String> {
    let mut g = slot.done.lock().unwrap_or_else(|e| e.into_inner());
    while g.is_none() {
        g = slot.cv.wait(g).unwrap_or_else(|e| e.into_inner());
    }
    match g.as_ref().unwrap() {
        Ok(b) => {
            unsafe {
                com_addref(b.root_sig);
                com_addref(b.pso);
            }
            Ok(*b)
        }
        Err(e) => Err(e.clone()),
    }
}

fn get_or_build_pso<F>(key: PsoKey, times: &mut BuildTimes, build: F) -> Result<BuiltProgram, String>
where
    F: FnOnce(&mut BuildTimes) -> Result<BuiltProgram, String>,
{
    match claim_pso(key) {
        PsoClaim::Cached(b) => {
            times.reused = true;
            Ok(b)
        }
        PsoClaim::Wait(slot) => {
            let res = wait_inflight_pso(&slot);
            if res.is_ok() {
                times.reused = true;
            }
            res
        }
        PsoClaim::Build(guard) => {
            let res = build(times);
            guard.complete(res)
        }
    }
}

const MAX_COMPILE_THREADS: usize = 64;
const COMPILE_STACK: usize = 64 << 20;

fn elapsed_ms(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

unsafe fn com_addref(obj: usize) {
    if obj != 0 {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(vfn(obj as *mut c_void, 1)) };
        unsafe { f(obj as *mut c_void) };
    }
}

fn cached_pso(key: &PsoKey) -> Option<BuiltProgram> {
    let g = PSO_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let b = *g.as_ref()?.get(key)?;
    unsafe {
        com_addref(b.root_sig);
        com_addref(b.pso);
    }
    Some(b)
}

fn remember_pso(key: PsoKey, built: BuiltProgram) -> BuiltProgram {
    let mut g = PSO_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(prev) = map.get(&key).copied() {
        unsafe {
            com_release(built.pso as *mut c_void);
            com_release(built.root_sig as *mut c_void);
            com_addref(prev.root_sig);
            com_addref(prev.pso);
        }
        return prev;
    }
    unsafe {
        com_addref(built.root_sig);
        com_addref(built.pso);
    }
    map.insert(key, built);
    built
}

pub fn parse_compile_threads(val: Option<&str>, cpus: usize) -> Result<usize, String> {
    let auto = cpus.clamp(1, MAX_COMPILE_THREADS);
    match val.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "auto" | "0") => Ok(auto),
        Some(s) => match s.parse::<usize>() {
            Ok(n) if (1..=MAX_COMPILE_THREADS).contains(&n) => Ok(n),
            _ => Err(format!("TOMBOX_RT_COMPILE_THREADS={s:?} is not auto, 0 or 1..={MAX_COMPILE_THREADS}; using auto ({auto})")),
        },
    }
}

fn configured_compile_threads() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        let env = std::env::var("TOMBOX_RT_COMPILE_THREADS").ok();
        let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
        let n = parse_compile_threads(env.as_deref(), cpus).unwrap_or_else(|e| {
            record(M, TAG, &e);
            cpus.clamp(1, MAX_COMPILE_THREADS)
        });
        record(M, TAG, &format!("DXR program compile threads={n} (env={env:?}, cpus={cpus})"));
        n
    })
}

unsafe fn build_program_pso(
    real_dev: *mut c_void,
    prog: &state_obj::StateObjectProgram,
    tr: &rt_cps::TranslatedRtProgram,
    times: &mut BuildTimes,
) -> Result<BuiltProgram, String> {
    use tombox_xshader::shader_cache::{self, Fma, Source};
    if !tombox_xshader::dxc::available() {
        return Err("DXC unavailable (check TOMBOX_DXC_DIR)".into());
    }
    if crate::cbv_raw::enabled() && reads_title_cbv(&tr.hlsl) {
        return Err(format!(
            "refused: cbv_raw is on, so the title's CBV descriptors are raw SRVs, but this program reads title CBVs {:?} as constant buffers",
            title_cbv_bindings(&prog.root_sig)
        ));
    }
    let key = PsoKey {
        device: real_dev as usize,
        hlsl: shader_cache::key(&tr.hlsl, tr.profile, Fma::default(), ""),
        rts0: prog.root_sig_dxbc.clone(),
    };
    get_or_build_pso(key, times, |times| {
        let t = std::time::Instant::now();
        let dxil = shader_cache::compile_cached(&tr.hlsl, tr.profile, Fma::default(), &[]).map_err(|e| format!("DXC FAILED: {e}"))?;
        times.dxil_ms = elapsed_ms(t);
        times.from_pack = dxil.source == Source::Pack;
        let t = std::time::Instant::now();
        let (rs, b0_idx, b1_idx) = unsafe { create_rt_root_signature(real_dev, prog) }.map_err(|e| format!("RS FAILED: {e}"))?;
        match unsafe { create_compute_pso(real_dev, rs, &dxil.code) } {
            Ok(pso) => {
                times.pso_ms = elapsed_ms(t);
                Ok(BuiltProgram { root_sig: rs as usize, pso: pso as usize, b0_idx, b1_idx })
            }
            Err(e) => {
                unsafe { crate::cmdlist_x::dump_device_messages(real_dev, TAG) };
                unsafe { com_release(rs) };
                Err(format!("PSO FAILED: {e}"))
            }
        }
    })
}

fn build_program(real_dev: usize, prog: &state_obj::StateObjectProgram, pi: u32) -> ProgramResult {
    let mut times = BuildTimes::default();
    let t = std::time::Instant::now();
    let tr = rt_cps::translate_state_program(prog, pi);
    times.translate_ms = elapsed_ms(t);
    match tr {
        Ok(tr) => {
            let built = unsafe { build_program_pso(real_dev as *mut c_void, prog, &tr, &mut times) };
            ProgramResult { exports: tr.exports, built, translated: true, times }
        }
        Err(e) => ProgramResult { exports: Vec::new(), built: Err(format!("translate FAILED: {e}")), translated: false, times },
    }
}

fn build_program_guarded(real_dev: usize, prog: &state_obj::StateObjectProgram, pi: u32) -> ProgramResult {
    guarded(|| build_program(real_dev, prog, pi))
}

fn guarded(build: impl FnOnce() -> ProgramResult) -> ProgramResult {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)).unwrap_or_else(|p| {
        let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
        ProgramResult { exports: Vec::new(), built: Err(format!("build panicked: {msg}")), translated: false, times: BuildTimes::default() }
    })
}

fn build_programs<F>(count: usize, threads: usize, build: F) -> Vec<ProgramResult>
where
    F: Fn(usize) -> ProgramResult + Sync,
{
    let threads = threads.min(count);
    if threads <= 1 {
        return (0..count).map(&build).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<ProgramResult>>> = (0..count).map(|_| Mutex::new(None)).collect();
    let work = || loop {
        let i = next.fetch_add(1, Ordering::Relaxed);
        if i >= count {
            break;
        }
        let r = build(i);
        *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
    };
    std::thread::scope(|s| {
        let spawned: Vec<_> = (1..threads)
            .filter_map(|_| std::thread::Builder::new().stack_size(COMPILE_STACK).spawn_scoped(s, &work).ok())
            .collect();
        work();
        for h in spawned {
            let _ = h.join();
        }
    });
    slots
        .into_iter()
        .enumerate()
        .map(|(i, m)| m.into_inner().unwrap_or_else(|e| e.into_inner()).unwrap_or_else(|| build(i)))
        .collect()
}

pub fn parse_rt_async(val: Option<&str>) -> Result<bool, String> {
    match val.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "1" | "on" | "true") => Ok(true),
        Some("0" | "off" | "false") => Ok(false),
        Some(s) => Err(format!("TOMBOX_RT_ASYNC={s:?} is not on, off, 1 or 0; using on")),
    }
}

fn configured_rt_async() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let raw = std::env::var("TOMBOX_RT_ASYNC").ok().or_else(|| compat_common::cfg("rt_async"));
        let on = parse_rt_async(raw.as_deref()).unwrap_or_else(|e| {
            record(M, TAG, &e);
            true
        });
        record(
            M,
            TAG,
            if on {
                "DXR state-object programs compile in the background and wait on first DispatchRays (TOMBOX_RT_ASYNC=0 builds synchronously)"
            } else {
                "TOMBOX_RT_ASYNC=0: DXR state-object programs build synchronously in DeserializeStateObjectX"
            },
        );
        on
    })
}

type PoolJob = (u64, Box<dyn FnOnce() + Send>);

struct PoolState {
    queue: VecDeque<PoolJob>,
    closed: bool,
}

struct RtPool {
    st: Mutex<PoolState>,
    cv: Condvar,
    threads: usize,
}

impl RtPool {
    fn new(threads: usize) -> Arc<Self> {
        let threads = threads.clamp(1, MAX_COMPILE_THREADS);
        let pool = Arc::new(Self {
            st: Mutex::new(PoolState { queue: VecDeque::new(), closed: false }),
            cv: Condvar::new(),
            threads,
        });
        for i in 0..threads {
            let p = Arc::clone(&pool);
            let _ = std::thread::Builder::new()
                .name(format!("tombox-rt-compile-{i}"))
                .stack_size(COMPILE_STACK)
                .spawn(move || loop {
                    let job = {
                        let mut g = p.st.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if let Some((_, j)) = g.queue.pop_front() {
                                break Some(j);
                            }
                            if g.closed {
                                break None;
                            }
                            g = p.cv.wait(g).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    match job {
                        Some(j) => j(),
                        None => break,
                    }
                });
        }
        pool
    }

    fn enqueue(&self, id: u64, jobs: impl IntoIterator<Item = Box<dyn FnOnce() + Send>>) {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        for j in jobs {
            g.queue.push_back((id, j));
        }
        self.cv.notify_all();
    }

    fn prioritize(&self, id: u64) {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        if g.queue.len() <= 1 {
            return;
        }
        let mut front = VecDeque::new();
        let mut rest = VecDeque::new();
        while let Some(item) = g.queue.pop_front() {
            if item.0 == id {
                front.push_back(item);
            } else {
                rest.push_back(item);
            }
        }
        front.append(&mut rest);
        g.queue = front;
        self.cv.notify_all();
    }

    #[cfg(test)]
    fn close(&self) {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        g.closed = true;
        self.cv.notify_all();
    }
}

fn global_rt_pool() -> &'static Arc<RtPool> {
    static POOL: OnceLock<Arc<RtPool>> = OnceLock::new();
    POOL.get_or_init(|| RtPool::new(configured_compile_threads()))
}

struct DeviceRef(usize);

impl DeviceRef {
    unsafe fn acquire(dev: *mut c_void) -> Self {
        let addr = dev as usize;
        if addr != 0 {
            unsafe { com_addref(addr) };
        }
        Self(addr)
    }
}

impl Drop for DeviceRef {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { com_release(self.0 as *mut c_void) };
        }
    }
}

struct ProgramBuild {
    id: u64,
    done: OnceLock<Arc<Vec<CompiledProgram>>>,
    ready_mu: Mutex<()>,
    cv: Condvar,
    urgent: AtomicBool,
    pool: Option<Arc<RtPool>>,
}

impl ProgramBuild {
    fn ready(programs: Vec<CompiledProgram>) -> Arc<Self> {
        let done = OnceLock::new();
        let _ = done.set(Arc::new(programs));
        Arc::new(Self {
            id: 0,
            done,
            ready_mu: Mutex::new(()),
            cv: Condvar::new(),
            urgent: AtomicBool::new(false),
            pool: None,
        })
    }

    fn pending(id: u64, pool: Arc<RtPool>) -> Arc<Self> {
        Arc::new(Self {
            id,
            done: OnceLock::new(),
            ready_mu: Mutex::new(()),
            cv: Condvar::new(),
            urgent: AtomicBool::new(false),
            pool: Some(pool),
        })
    }

    fn is_ready(&self) -> bool {
        self.done.get().is_some()
    }

    fn finish(&self, programs: Vec<CompiledProgram>) {
        let _ = self.done.set(Arc::new(programs));
        let _g = self.ready_mu.lock().unwrap_or_else(|e| e.into_inner());
        self.cv.notify_all();
    }

    fn programs(&self, log_wait: bool) -> Arc<Vec<CompiledProgram>> {
        if let Some(p) = self.done.get() {
            return Arc::clone(p);
        }
        if !self.urgent.swap(true, Ordering::AcqRel) {
            if let Some(pool) = &self.pool {
                pool.prioritize(self.id);
            }
        }
        let wait_start = std::time::Instant::now();
        let mut g = self.ready_mu.lock().unwrap_or_else(|e| e.into_inner());
        while self.done.get().is_none() {
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        drop(g);
        if log_wait {
            let waited_ms = elapsed_ms(wait_start);
            if waited_ms >= 1.0 && once(&LOG_ASYNC_WAIT, 16) {
                record(M, TAG, &format!("DispatchRays waited {waited_ms:.0} ms for background DXR state-object build #{}", self.id));
            }
        }
        Arc::clone(self.done.get().unwrap())
    }
}

fn finalize_results(
    prog_exports: &[Vec<String>],
    results: Vec<ProgramResult>,
    meta: Option<(u64, usize, u32, usize, usize, bool, std::time::Instant)>,
) -> Vec<CompiledProgram> {
    let mut compiled = Vec::with_capacity(prog_exports.len());
    let mut compiled_ok = 0usize;
    let mut total = BuildTimes::default();
    let (mut from_pack, mut reused) = (0usize, 0usize);
    for (pi, (exports, res)) in prog_exports.iter().zip(results).enumerate() {
        total.translate_ms += res.times.translate_ms;
        total.dxil_ms += res.times.dxil_ms;
        total.pso_ms += res.times.pso_ms;
        from_pack += res.times.from_pack as usize;
        reused += res.times.reused as usize;
        if !res.translated {
            if let Err(e) = &res.built {
                record(M, TAG, &format!("DeserializeStateObjectX prog#{pi} {e}"));
            }
            continue;
        }
        let mut cp = CompiledProgram {
            root_sig: 0,
            pso: 0,
            b0_param_idx: None,
            b1_param_idx: None,
            exports: exports.clone(),
        };
        match res.built {
            Ok(b) => {
                cp.root_sig = b.root_sig;
                cp.pso = b.pso;
                cp.b0_param_idx = b.b0_idx;
                cp.b1_param_idx = b.b1_idx;
                compiled_ok += 1;
            }
            Err(e) => record(M, TAG, &format!("DeserializeStateObjectX prog#{pi} {e}")),
        }
        compiled.push(cp);
    }
    if let Some((id, size, version, export_count, threads, async_mode, started)) = meta {
        let mode = if async_mode { "async" } else { "sync" };
        record(
            M,
            TAG,
            &format!(
                "DeserializeStateObjectX#{id}({size}B v{version} {mode}): {} programs ({compiled_ok} compiled to DXIL PSO), {export_count} exports in {:.0} ms on {threads} threads (translate {:.0} ms, DXIL {:.0} ms with {from_pack} from the pack, PSO {:.0} ms, {reused} PSOs reused)",
                prog_exports.len(),
                elapsed_ms(started),
                total.translate_ms,
                total.dxil_ms,
                total.pso_ms
            ),
        );
    }
    compiled
}

fn submit_async_batch<F>(
    pool: &Arc<RtPool>,
    prog_exports: Vec<Vec<String>>,
    meta: Option<(u64, usize, u32, usize, usize, bool, std::time::Instant)>,
    build_one: F,
) -> Arc<ProgramBuild>
where
    F: Fn(usize) -> ProgramResult + Send + Sync + 'static,
{
    let count = prog_exports.len();
    if count == 0 {
        let compiled = finalize_results(&prog_exports, Vec::new(), meta);
        return ProgramBuild::ready(compiled);
    }
    let id = meta.map_or_else(|| NEXT_BUILD_ID.fetch_add(1, Ordering::Relaxed), |m| m.0);
    let build = ProgramBuild::pending(id, Arc::clone(pool));
    struct Batch<F> {
        build: Arc<ProgramBuild>,
        prog_exports: Vec<Vec<String>>,
        slots: Mutex<Vec<Option<ProgramResult>>>,
        remaining: AtomicUsize,
        meta: Option<(u64, usize, u32, usize, usize, bool, std::time::Instant)>,
        build_one: F,
    }
    let batch = Arc::new(Batch {
        build: Arc::clone(&build),
        prog_exports,
        slots: Mutex::new((0..count).map(|_| None).collect()),
        remaining: AtomicUsize::new(count),
        meta,
        build_one,
    });
    let jobs: Vec<Box<dyn FnOnce() + Send>> = (0..count)
        .map(|pi| {
            let b = Arc::clone(&batch);
            Box::new(move || {
                let r = guarded(|| (b.build_one)(pi));
                {
                    let mut g = b.slots.lock().unwrap_or_else(|e| e.into_inner());
                    g[pi] = Some(r);
                }
                if b.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                    let results: Vec<ProgramResult> = {
                        let mut g = b.slots.lock().unwrap_or_else(|e| e.into_inner());
                        g.iter_mut()
                            .map(|s| s.take().unwrap_or_else(|| ProgramResult {
                                exports: Vec::new(),
                                built: Err("missing program result".into()),
                                translated: false,
                                times: BuildTimes::default(),
                            }))
                            .collect()
                    };
                    let compiled = finalize_results(&b.prog_exports, results, b.meta);
                    b.build.finish(compiled);
                }
            }) as Box<dyn FnOnce() + Send>
        })
        .collect();
    pool.enqueue(id, jobs);
    build
}


pub unsafe extern "system" fn create_state_object_x(
    this: *mut c_void,
    _desc: *const u8,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> i32 {
    if ppv.is_null() {
        return E_POINTER;
    }
    unsafe { *ppv = core::ptr::null_mut() };
    let real = unsafe { real_device(this) };
    let so = alloc_state_object(real, HashMap::new(), ProgramBuild::ready(Vec::new()), Vec::new());
    let hr = if riid.is_null() {
        unsafe { *ppv = so };
        S_OK
    } else {
        let h = unsafe { so_qi(so as *mut RtStateObject, riid, ppv) };
        unsafe { so_release(so as *mut RtStateObject) };
        h
    };
    record(M, TAG, &format!("CreateStateObject -> hr={hr:#010x} so={:p}", unsafe { *ppv }));
    hr
}

pub unsafe extern "system" fn get_prebuild_info_x(
    _this: *mut c_void,
    inputs: *const u8,
    out_info: *mut PrebuildInfo,
) {
    if out_info.is_null() {
        return;
    }
    let info = unsafe { rt_bvh::prebuild_info_from_inputs(inputs) };
    unsafe { core::ptr::write_unaligned(out_info, info) };
}

pub unsafe extern "system" fn deserialize_state_object_x(
    this: *mut c_void,
    blob: *const u8,
    size: usize,
    _root_sig: *mut c_void,
    flags: u32,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> i32 {
    if ppv.is_null() {
        return E_POINTER;
    }
    unsafe { *ppv = core::ptr::null_mut() };
    if blob.is_null() || size < 32 {
        return E_INVALIDARG;
    }
    let slice = unsafe { core::slice::from_raw_parts(blob, size) };
    let parsed = match state_obj::parse_state_object(slice) {
        Ok(p) => p,
        Err(e) => {
            record(M, TAG, &format!("DeserializeStateObjectX({size}B flags={flags:#x}) parse FAILED: {e}"));
            return E_INVALIDARG;
        }
    };

    let real = unsafe { real_device(this) };
    let mut exports_map: HashMap<String, Box<ShaderId>> = HashMap::new();
    let mut sym_records = Vec::new();
    let mut prog_exports = Vec::with_capacity(parsed.programs.len());
    for (pi, prog) in parsed.programs.iter().enumerate() {
        for sym in &prog.symbols {
            sym_records.push([0, 0, sym.code_offset, sym.flags, sym.scratch_bytes, sym.rsrc_info]);
        }
        for (name, id) in rt_cps::program_exports(prog, pi as u32) {
            exports_map.insert(name, Box::new(id));
        }
        prog_exports.push(prog.exports.clone());
    }

    let started = std::time::Instant::now();
    let id = NEXT_BUILD_ID.fetch_add(1, Ordering::Relaxed);
    let async_mode = configured_rt_async();
    let pool = global_rt_pool();
    let threads = pool.threads.min(parsed.programs.len().max(1));
    let version = parsed.version;
    let export_count = exports_map.len();
    let dev_ref = Arc::new(unsafe { DeviceRef::acquire(real) });
    let progs = Arc::new(parsed.programs);
    let build = submit_async_batch(
        pool,
        prog_exports,
        Some((id, size, version, export_count, threads, async_mode, started)),
        move |pi| build_program_guarded(dev_ref.0, &progs[pi], pi as u32),
    );
    if !async_mode {
        let _ = build.programs(false);
    }

    let so = alloc_state_object(real, exports_map, build, sym_records);
    if riid.is_null() {
        unsafe { *ppv = so };
        S_OK
    } else {
        let hr = unsafe { so_qi(so as *mut RtStateObject, riid, ppv) };
        unsafe { so_release(so as *mut RtStateObject) };
        hr
    }
}


fn read_vertex(ptr: *const u8, fmt: u32) -> [f32; 3] {
    unsafe {
        match fmt {
            DXGI_FORMAT_R32G32B32_FLOAT => [
                f32::from_bits(core::ptr::read_unaligned(ptr as *const u32)),
                f32::from_bits(core::ptr::read_unaligned(ptr.add(4) as *const u32)),
                f32::from_bits(core::ptr::read_unaligned(ptr.add(8) as *const u32)),
            ],
            DXGI_FORMAT_R32G32_FLOAT => [
                f32::from_bits(core::ptr::read_unaligned(ptr as *const u32)),
                f32::from_bits(core::ptr::read_unaligned(ptr.add(4) as *const u32)),
                0.0,
            ],
            DXGI_FORMAT_R16G16B16A16_FLOAT => {
                let f16 = |o: usize| -> f32 {
                    let h = core::ptr::read_unaligned(ptr.add(o) as *const u16);
                    let s = ((h >> 15) as u32) << 31;
                    let e = ((h >> 10) & 0x1f) as i32;
                    let m = (h & 0x3ff) as u32;
                    if e == 0 {
                        f32::from_bits(s)
                    } else if e == 31 {
                        f32::from_bits(s | 0x7f80_0000 | (m << 13))
                    } else {
                        f32::from_bits(s | (((e + 112) as u32) << 23) | (m << 13))
                    }
                };
                [f16(0), f16(2), f16(4)]
            }
            _ => [0.0; 3],
        }
    }
}

unsafe fn cpu_ptr_for_va(va: u64, min_bytes: u64) -> Option<*const u8> {
    if va == 0 {
        return None;
    }
    if let Some((e, off)) = crate::gpu_va::lookup(va) {
        if off.saturating_add(min_bytes) <= e.len {
            if let Some(p) = crate::placed::lookup(e.res as *mut c_void) {
                return Some((p.va + off) as *const u8);
            }
        }
    }
    None
}

unsafe fn upload_bvh_to_gpu(list: *mut c_void, dst_va: u64, bytes: &[u8]) -> Option<usize> {
    let (entry, off) = crate::gpu_va::lookup_gpu(dst_va)?;
    if entry.res == 0 || bytes.is_empty() || off + bytes.len() as u64 > entry.len {
        return None;
    }
    let get_dev: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(vfn(list, 7)) };
    let mut dev: *mut c_void = core::ptr::null_mut();
    if unsafe { get_dev(list, &IID_DEVICE, &mut dev) } < 0 || dev.is_null() {
        return None;
    }
    let heap: [u32; 5] = [2, 0, 0, 1, 1]; // UPLOAD
    let mut desc = [0u8; 56];
    desc[0..4].copy_from_slice(&1u32.to_le_bytes());
    desc[0x10..0x18].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
    desc[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
    desc[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
    desc[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
    desc[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
    desc[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
    let mk: unsafe extern "system" fn(*mut c_void, *const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(vfn(dev, PC_CREATE_COMMITTED)) };
    let mut up: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk(dev, heap.as_ptr(), 0, desc.as_ptr(), 0xac3, core::ptr::null(), &IID_RESOURCE, &mut up) };
    unsafe { com_release(dev) };
    if hr < 0 || up.is_null() {
        return None;
    }
    let map: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut u8) -> i32 =
        unsafe { core::mem::transmute(vfn(up, 8)) };
    let mut ptr: *mut u8 = core::ptr::null_mut();
    if unsafe { map(up, 0, core::ptr::null(), &mut ptr) } >= 0 && !ptr.is_null() {
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
        let copy: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *mut c_void, u64, u64) =
            unsafe { core::mem::transmute(vfn(list, PC_CL_COPY_BUFFER_REGION)) };
        unsafe { copy(list, entry.res as *mut c_void, off, up, 0, bytes.len() as u64) };
        unsafe { transition_copy_dest_to_common(list, entry.res as *mut c_void) };
    }
    retain_staging(crate::cmdlist_wrap::unwrap(list) as usize, up as usize);
    Some(entry.res)
}

struct QueueStaging {
    fence: usize,
    next: u64,
    inflight: VecDeque<(u64, Vec<usize>)>,
}

#[derive(Default)]
struct StagingTracker {
    pending: HashMap<usize, Vec<usize>>,
    queues: HashMap<usize, QueueStaging>,
    retained: Vec<usize>,
}

impl StagingTracker {
    fn record(&mut self, list: usize, res: usize) {
        self.pending.entry(list).or_default().push(res);
    }

    fn take_lists(&mut self, lists: &[usize]) -> Vec<usize> {
        let mut out = Vec::new();
        for l in lists {
            if let Some(mut v) = self.pending.remove(l) {
                out.append(&mut v);
            }
        }
        out
    }

    fn reclaim(&mut self, queue: usize, completed: u64) -> Vec<usize> {
        let mut freed = Vec::new();
        if let Some(q) = self.queues.get_mut(&queue) {
            while let Some((val, _)) = q.inflight.front() {
                if *val > completed {
                    break;
                }
                if let Some((_, mut batch)) = q.inflight.pop_front() {
                    freed.append(&mut batch);
                }
            }
        }
        freed
    }
}

static STAGING_TRACKER: Mutex<Option<StagingTracker>> = Mutex::new(None);
static STAGING_ACTIVE: AtomicUsize = AtomicUsize::new(0);

fn retain_staging(list: usize, res: usize) {
    if res == 0 {
        return;
    }
    STAGING_TRACKER.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(StagingTracker::default).record(list, res);
    STAGING_ACTIVE.fetch_add(1, Ordering::Release);
}

unsafe fn create_queue_fence(queue: *mut c_void) -> Option<usize> {
    let get_dev: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vfn(queue, 7)) };
    let mut dev: *mut c_void = core::ptr::null_mut();
    if unsafe { get_dev(queue, &IID_DEVICE, &mut dev) } < 0 || dev.is_null() {
        return None;
    }
    let mk: unsafe extern "system" fn(*mut c_void, u64, u32, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(vfn(dev, PC_CREATE_FENCE)) };
    let mut fence: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk(dev, 0, 0, &IID_FENCE, &mut fence) };
    unsafe { com_release(dev) };
    if hr < 0 || fence.is_null() { None } else { Some(fence as usize) }
}

unsafe fn fence_completed(fence: usize) -> u64 {
    let f: unsafe extern "system" fn(*mut c_void) -> u64 = unsafe { core::mem::transmute(vfn(fence as *mut c_void, 8)) };
    unsafe { f(fence as *mut c_void) }
}

pub unsafe fn after_execute(queue: *mut c_void, real_lists: &[*mut c_void], signal_fn: usize) {
    if queue.is_null() || STAGING_ACTIVE.load(Ordering::Acquire) == 0 {
        return;
    }
    let qkey = queue as usize;
    let list_keys: Vec<usize> = real_lists.iter().map(|&p| p as usize).collect();
    let (existing_fence, batch) = {
        let mut g = STAGING_TRACKER.lock().unwrap_or_else(|e| e.into_inner());
        let Some(tr) = g.as_mut() else { return };
        let f = tr.queues.get(&qkey).map(|q| q.fence);
        let b = tr.take_lists(&list_keys);
        (f, b)
    };
    let mut to_release = Vec::new();
    if let Some(fence) = existing_fence {
        let done = unsafe { fence_completed(fence) };
        let mut g = STAGING_TRACKER.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tr) = g.as_mut() {
            to_release = tr.reclaim(qkey, done);
        }
    }
    if !batch.is_empty() {
        let fence = match existing_fence {
            Some(f) => Some(f),
            None => unsafe { create_queue_fence(queue) },
        };
        let mut signaled = None;
        if let Some(f) = fence {
            let next = {
                let mut g = STAGING_TRACKER.lock().unwrap_or_else(|e| e.into_inner());
                let tr = g.get_or_insert_with(StagingTracker::default);
                let q = tr.queues.entry(qkey).or_insert_with(|| QueueStaging { fence: f, next: 0, inflight: VecDeque::new() });
                q.next += 1;
                (q.fence, q.next)
            };
            if next.0 != f && existing_fence.is_none() {
                unsafe { com_release(f as *mut c_void) };
            }
            let sig: unsafe extern "system" fn(*mut c_void, *mut c_void, u64) -> i32 =
                unsafe { core::mem::transmute(if signal_fn != 0 { signal_fn } else { vfn(queue, 14) }) };
            if unsafe { sig(queue, next.0 as *mut c_void, next.1) } >= 0 {
                signaled = Some(next.1);
            }
        }
        let mut g = STAGING_TRACKER.lock().unwrap_or_else(|e| e.into_inner());
        let tr = g.get_or_insert_with(StagingTracker::default);
        match signaled {
            Some(val) => {
                if let Some(q) = tr.queues.get_mut(&qkey) {
                    q.inflight.push_back((val, batch));
                } else {
                    tr.retained.extend(batch);
                }
            }
            None => {
                tr.retained.extend(batch);
            }
        }
    }
    if !to_release.is_empty() {
        STAGING_ACTIVE.fetch_sub(to_release.len(), Ordering::AcqRel);
        for res in to_release {
            unsafe { com_release(res as *mut c_void) };
        }
    }
}

pub unsafe extern "system" fn build_as_x(
    this: *mut c_void,
    pdesc: *const u8,
    num_postbuild: u32,
    postbuild: *const u8,
) {
    if pdesc.is_null() {
        return;
    }
    let q = |o: usize| unsafe { core::ptr::read_unaligned(pdesc.add(o) as *const u64) };
    let u = |o: usize| unsafe { core::ptr::read_unaligned(pdesc.add(o) as *const u32) };

    let dst_va = q(0x00);
    let as_type = u(0x08);
    let build_flags = u(0x0c);
    let num_descs = u(0x10);
    let layout = u(0x14);
    let inputs_ptr = q(0x18);

    let (built, blas_vas) = if as_type == AS_TYPE_BOTTOM_LEVEL {
        let mut tris = Vec::new();
        if inputs_ptr != 0 && num_descs > 0 {
            for gi in 0..num_descs as usize {
                let gptr = if layout == 1 {
                    unsafe { core::ptr::read_unaligned((inputs_ptr as *const u64).add(gi)) as *const u8 }
                } else {
                    unsafe { (inputs_ptr as *const u8).add(gi * 56) }
                };
                if gptr.is_null() {
                    continue;
                }
                let gtype = unsafe { core::ptr::read_unaligned(gptr as *const u32) };
                let gflags = unsafe { core::ptr::read_unaligned(gptr.add(4) as *const u32) };
                if gtype != GEOM_TRIANGLES {
                    continue;
                }
                let idx_fmt = unsafe { core::ptr::read_unaligned(gptr.add(0x10) as *const u32) };
                let vtx_fmt = unsafe { core::ptr::read_unaligned(gptr.add(0x14) as *const u32) };
                let idx_cnt = unsafe { core::ptr::read_unaligned(gptr.add(0x18) as *const u32) } as usize;
                let vtx_cnt = unsafe { core::ptr::read_unaligned(gptr.add(0x1c) as *const u32) } as usize;
                let idx_va = unsafe { core::ptr::read_unaligned(gptr.add(0x20) as *const u64) };
                let vtx_va = unsafe { core::ptr::read_unaligned(gptr.add(0x28) as *const u64) };
                let vtx_stride = unsafe { core::ptr::read_unaligned(gptr.add(0x30) as *const u64) }.max(12) as usize;

                let Some(vbase) = (unsafe { cpu_ptr_for_va(vtx_va, (vtx_cnt * vtx_stride) as u64) }) else {
                    continue;
                };
                if idx_cnt >= 3 && idx_va != 0 {
                    let isz = if idx_fmt == DXGI_FORMAT_R16_UINT { 2 } else { 4 };
                    if let Some(ibase) = unsafe { cpu_ptr_for_va(idx_va, (idx_cnt * isz) as u64) } {
                        for ti in 0..(idx_cnt / 3) {
                            let mut idx = [0usize; 3];
                            for k in 0..3 {
                                let o = (ti * 3 + k) * isz;
                                idx[k] = if idx_fmt == DXGI_FORMAT_R16_UINT {
                                    unsafe { core::ptr::read_unaligned(ibase.add(o) as *const u16) as usize }
                                } else if idx_fmt == DXGI_FORMAT_R32_UINT {
                                    unsafe { core::ptr::read_unaligned(ibase.add(o) as *const u32) as usize }
                                } else {
                                    ti * 3 + k
                                };
                            }
                            if idx.iter().all(|&i| i < vtx_cnt) {
                                tris.push(TriangleInput {
                                    v: [
                                        read_vertex(unsafe { vbase.add(idx[0] * vtx_stride) }, vtx_fmt),
                                        read_vertex(unsafe { vbase.add(idx[1] * vtx_stride) }, vtx_fmt),
                                        read_vertex(unsafe { vbase.add(idx[2] * vtx_stride) }, vtx_fmt),
                                    ],
                                    geometry_index: gi as u32,
                                    primitive_index: ti as u32,
                                    flags: gflags,
                                });
                            }
                        }
                    }
                } else {
                    for ti in 0..(vtx_cnt / 3) {
                        tris.push(TriangleInput {
                            v: [
                                read_vertex(unsafe { vbase.add((ti * 3) * vtx_stride) }, vtx_fmt),
                                read_vertex(unsafe { vbase.add((ti * 3 + 1) * vtx_stride) }, vtx_fmt),
                                read_vertex(unsafe { vbase.add((ti * 3 + 2) * vtx_stride) }, vtx_fmt),
                            ],
                            geometry_index: gi as u32,
                            primitive_index: ti as u32,
                            flags: gflags,
                        });
                    }
                }
            }
        }
        (rt_bvh::build_blas(&tris), Vec::new())
    } else {
        let mut instances = Vec::new();
        let mut blas_vas = Vec::new();
        if inputs_ptr != 0 && num_descs > 0 {
            if let Some(ibase) = unsafe { cpu_ptr_for_va(inputs_ptr, num_descs as u64 * 64) } {
                let reg = AS_REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
                for i in 0..num_descs as usize {
                    let p = unsafe { ibase.add(i * 64) };
                    let mut m = [0.0f32; 12];
                    for k in 0..12 {
                        m[k] = f32::from_bits(unsafe { core::ptr::read_unaligned(p.add(k * 4) as *const u32) });
                    }
                    let id_mask = unsafe { core::ptr::read_unaligned(p.add(0x30) as *const u32) };
                    let sbt_flags = unsafe { core::ptr::read_unaligned(p.add(0x34) as *const u32) };
                    let blas_va = unsafe { core::ptr::read_unaligned(p.add(0x38) as *const u64) };
                    let bounds = reg
                        .as_ref()
                        .and_then(|r| r.get(&blas_va))
                        .map(|r| r.bounds)
                        .unwrap_or(Aabb { min: [-1e3; 3], max: [1e3; 3] });
                    if blas_va != 0 && !blas_vas.contains(&blas_va) {
                        blas_vas.push(blas_va);
                    }
                    instances.push(InstanceInput {
                        transform: m,
                        instance_id: id_mask & 0x00ff_ffff,
                        mask: id_mask >> 24,
                        sbt_offset: sbt_flags & 0x00ff_ffff,
                        flags: sbt_flags >> 24,
                        blas_va,
                        blas_bounds: bounds,
                    });
                }
            }
        }
        (rt_bvh::build_tlas(&instances), blas_vas)
    };

    let res = unsafe { upload_bvh_to_gpu(this, dst_va, &built.bytes) }.unwrap_or(0);
    if dst_va != 0 {
        AS_REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(
            dst_va,
            AsRecord {
                va: dst_va,
                size: built.bytes.len() as u64,
                res,
                as_type,
                bounds: built.bounds,
                blas_vas,
                pending_copy: (build_flags & BUILD_FLAG_ALLOW_COMPACTION != 0).then(|| std::sync::Arc::new(built.bytes.clone())),
            },
        );
    }

    if num_postbuild > 0 && !postbuild.is_null() {
        for i in 0..num_postbuild as usize {
            let pb = unsafe { postbuild.add(i * 16) };
            let dest = unsafe { core::ptr::read_unaligned(pb as *const u64) };
            let _ = unsafe { upload_bvh_to_gpu(this, dest, &(built.bytes.len() as u64).to_le_bytes()) };
        }
    }

    if once(&LOG_BUILD, 12) {
        record(
            M,
            TAG,
            &format!(
                "BuildRaytracingAccelerationStructure type={} descs={} leaves={} internal={} bytes={} dst={dst_va:#x}",
                if as_type == AS_TYPE_TOP_LEVEL { "TLAS" } else { "BLAS" },
                num_descs,
                built.leaf_count,
                built.internal_count,
                built.bytes.len()
            ),
        );
    }
}

fn postbuild_record(info_type: u32, size: u64) -> Option<Vec<u8>> {
    match info_type {
        POSTBUILD_COMPACTED_SIZE | POSTBUILD_CURRENT_SIZE => Some(size.to_le_bytes().to_vec()),
        _ => None,
    }
}

pub unsafe extern "system" fn emit_postbuild_info_x(this: *mut c_void, pdesc: *const u8, num_src: u32, src_vas: *const u64) {
    static LOG: AtomicUsize = AtomicUsize::new(0);
    if pdesc.is_null() || src_vas.is_null() || num_src == 0 {
        return;
    }
    let dest = unsafe { core::ptr::read_unaligned(pdesc as *const u64) };
    let info_type = unsafe { core::ptr::read_unaligned(pdesc.add(8) as *const u32) };
    let mut out = Vec::new();
    {
        let reg = AS_REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        for i in 0..num_src as usize {
            let va = unsafe { core::ptr::read_unaligned(src_vas.add(i)) };
            let size = reg.as_ref().and_then(|r| r.get(&va)).map_or(0, |r| r.size);
            match postbuild_record(info_type, size) {
                Some(rec) => out.extend_from_slice(&rec),
                None => {
                    if once(&LOG, 8) {
                        record(M, TAG, &format!("EmitRaytracingAccelerationStructurePostbuildInfo info type {info_type} not supported; destination {dest:#x} left unwritten"));
                    }
                    return;
                }
            }
        }
    }
    if unsafe { upload_bvh_to_gpu(this, dest, &out) }.is_none() && once(&LOG, 8) {
        record(M, TAG, &format!("EmitRaytracingAccelerationStructurePostbuildInfo: destination {dest:#x} is not inside a known buffer"));
    }
}

fn parse_serialized_as_header(hdr: &[u8]) -> Option<(u64, u64, u64)> {
    if hdr.len() < SERIALIZED_AS_HEADER_SIZE as usize {
        return None;
    }
    let serialized_size = u64::from_le_bytes(hdr[0x20..0x28].try_into().ok()?);
    let deserialized_size = u64::from_le_bytes(hdr[0x28..0x30].try_into().ok()?);
    let num_blas = u64::from_le_bytes(hdr[0x30..0x38].try_into().ok()?);
    let ptr_bytes = num_blas.checked_mul(8)?;
    let raw_off = SERIALIZED_AS_HEADER_SIZE.checked_add(ptr_bytes)?;
    let payload_off = raw_off.checked_add(15)? & !15;
    if deserialized_size == 0 {
        return None;
    }
    if serialized_size != 0 && payload_off.checked_add(deserialized_size)? > serialized_size {
        return None;
    }
    Some((payload_off, deserialized_size, num_blas))
}

fn fixup_deserialized_tlas_ptrs(dst: &mut [u8], blas_ptrs: &[u64]) -> bool {
    if dst.len() < 0x38 {
        return false;
    }
    let leaf_off = u32::from_le_bytes(dst[0x34..0x38].try_into().unwrap()) as usize;
    for (i, &va) in blas_ptrs.iter().enumerate() {
        let Some(pos) = i.checked_mul(128).and_then(|o| leaf_off.checked_add(60)?.checked_add(o)) else {
            return false;
        };
        if pos + 4 > dst.len() {
            return false;
        }
        let shifted = (va >> 8) as u32;
        dst[pos..pos + 4].copy_from_slice(&shifted.to_le_bytes());
    }
    true
}

unsafe fn cpu_view_for_va(va: u64, min_bytes: u64) -> Option<(*const u8, u64, Option<*mut c_void>)> {
    if va == 0 {
        return None;
    }
    let (e, off) = crate::gpu_va::lookup(va)?;
    let avail = e.len.checked_sub(off)?;
    if avail < min_bytes {
        return None;
    }
    if let Some(p) = crate::placed::lookup(e.res as *mut c_void) {
        return Some(((p.va + off) as *const u8, avail, None));
    }
    if e.res == 0 {
        return None;
    }
    let res = e.res as *mut c_void;
    let get_hp: unsafe extern "system" fn(*mut c_void, *mut [u32; 5], *mut u32) -> i32 = unsafe { core::mem::transmute(vfn(res, 14)) };
    let mut hp = [0u32; 5];
    let mut flags = 0u32;
    if unsafe { get_hp(res, &mut hp, &mut flags) } < 0 {
        return None;
    }
    let cpu_readable = hp[0] == 2 || hp[0] == 3 || (hp[0] == 4 && hp[1] >= 2);
    if !cpu_readable {
        return None;
    }
    let map: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut u8) -> i32 = unsafe { core::mem::transmute(vfn(res, 8)) };
    let mut ptr: *mut u8 = core::ptr::null_mut();
    if unsafe { map(res, 0, core::ptr::null(), &mut ptr) } < 0 || ptr.is_null() {
        return None;
    }
    Some((unsafe { ptr.add(off as usize) }, avail, Some(res)))
}

unsafe fn unmap_cpu_view(mapped: Option<*mut c_void>) {
    if let Some(res) = mapped {
        let unmap: unsafe extern "system" fn(*mut c_void, u32, *const c_void) = unsafe { core::mem::transmute(vfn(res, 9)) };
        unsafe { unmap(res, 0, core::ptr::null()) };
    }
}

unsafe fn deserialize_as_x(list: *mut c_void, dst_va: u64, src_va: u64, refuse: &dyn Fn(String), log_ok: &AtomicUsize) {
    let Some((src_ptr, avail, mapped)) = (unsafe { cpu_view_for_va(src_va, SERIALIZED_AS_HEADER_SIZE) }) else {
        let detail = if let Some((e, _)) = crate::gpu_va::lookup(src_va) {
            let mut hp = [0u32; 5];
            if e.res != 0 {
                let get_hp: unsafe extern "system" fn(*mut c_void, *mut [u32; 5], *mut u32) -> i32 = unsafe { core::mem::transmute(vfn(e.res as *mut c_void, 14)) };
                let mut f = 0u32;
                let _ = unsafe { get_hp(e.res as *mut c_void, &mut hp, &mut f) };
            }
            format!("source buffer res={:#x} len={:#x} heap_type={} cpu_page={} is not CPU-readable", e.res, e.len, hp[0], hp[1])
        } else {
            "source VA is not in any tracked buffer".into()
        };
        return refuse(detail);
    };
    let hdr = unsafe { core::slice::from_raw_parts(src_ptr, SERIALIZED_AS_HEADER_SIZE as usize) };
    let Some((payload_off, deser_size, num_blas)) = parse_serialized_as_header(hdr) else {
        unsafe { unmap_cpu_view(mapped) };
        return refuse("invalid serialized acceleration-structure header".into());
    };
    if payload_off.saturating_add(deser_size) > avail {
        unsafe { unmap_cpu_view(mapped) };
        return refuse(format!("serialized payload {payload_off:#x}+{deser_size:#x} exceeds source buffer ({avail:#x})"));
    }
    let mut blas_vas = Vec::new();
    let res = if num_blas == 0 {
        let gpu_copied = unsafe { gpu_copy_range(list, dst_va, src_va + payload_off, deser_size) };
        match gpu_copied {
            Some(r) => Some(r),
            None => {
                let payload = unsafe { core::slice::from_raw_parts(src_ptr.add(payload_off as usize), deser_size as usize) };
                unsafe { upload_bvh_to_gpu(list, dst_va, payload) }
            }
        }
    } else {
        let mut buf = unsafe { core::slice::from_raw_parts(src_ptr.add(payload_off as usize), deser_size as usize) }.to_vec();
        let ptr_base = unsafe { src_ptr.add(SERIALIZED_AS_HEADER_SIZE as usize) };
        for i in 0..num_blas as usize {
            let va = unsafe { core::ptr::read_unaligned(ptr_base.add(i * 8) as *const u64) };
            blas_vas.push(va);
        }
        if !fixup_deserialized_tlas_ptrs(&mut buf, &blas_vas) {
            unsafe { unmap_cpu_view(mapped) };
            return refuse("deserialized TLAS pointer fixup out of bounds".into());
        }
        unsafe { upload_bvh_to_gpu(list, dst_va, &buf) }
    };
    unsafe { unmap_cpu_view(mapped) };
    let Some(res) = res else {
        return refuse(format!("deserialized {deser_size} bytes could not be placed at destination"));
    };
    let as_type = if num_blas > 0 { AS_TYPE_TOP_LEVEL } else { AS_TYPE_BOTTOM_LEVEL };
    AS_REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(
        dst_va,
        AsRecord {
            va: dst_va,
            size: deser_size,
            res,
            as_type,
            bounds: Aabb { min: [-1e3; 3], max: [1e3; 3] },
            blas_vas,
            pending_copy: None,
        },
    );
    if once(log_ok, 16) {
        record(M, TAG, &format!("CopyRaytracingAccelerationStructure DESERIALIZE {src_va:#x} -> {dst_va:#x} ({deser_size} bytes, off={payload_off:#x}, blas_ptrs={num_blas})"));
    }
}

unsafe fn transition_copy_dest_to_common(list: *mut c_void, res: *mut c_void) {
    if list.is_null() || res.is_null() {
        return;
    }
    let mut b = [0u8; 32];
    b[8..16].copy_from_slice(&(res as u64).to_le_bytes());
    b[16..20].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    b[20..24].copy_from_slice(&0x400u32.to_le_bytes());
    let f: unsafe extern "system" fn(*mut c_void, u32, *const u8) =
        unsafe { core::mem::transmute(crate::cmdlist_wrap::pc_method(list, PC_CL_RESOURCE_BARRIER)) };
    unsafe { f(list, 1, b.as_ptr()) };
}

unsafe fn gpu_copy_range(list: *mut c_void, dst_va: u64, src_va: u64, size: u64) -> Option<usize> {
    if list.is_null() || size == 0 {
        return None;
    }
    let (s, soff) = crate::gpu_va::lookup_gpu(src_va)?;
    let (d, doff) = crate::gpu_va::lookup_gpu(dst_va)?;
    if s.res == 0 || d.res == 0 || s.res == d.res || soff.checked_add(size)? > s.len || doff.checked_add(size)? > d.len {
        return None;
    }
    let copy: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *mut c_void, u64, u64) =
        unsafe { core::mem::transmute(vfn(list, PC_CL_COPY_BUFFER_REGION)) };
    unsafe { copy(list, d.res as *mut c_void, doff, s.res as *mut c_void, soff, size) };
    unsafe { transition_copy_dest_to_common(list, d.res as *mut c_void) };
    Some(d.res)
}

pub unsafe extern "system" fn copy_as_x(this: *mut c_void, dst_va: u64, src_va: u64, mode: u32) {
    static LOG: AtomicUsize = AtomicUsize::new(0);
    let refuse = |why: String| {
        if once(&LOG, 16) {
            record(M, TAG, &format!("CopyRaytracingAccelerationStructure dst={dst_va:#x} src={src_va:#x} mode={mode}: {why}"));
        }
    };
    if mode == COPY_MODE_DESERIALIZE {
        return unsafe { deserialize_as_x(this, dst_va, src_va, &refuse, &LOG) };
    }
    if mode != COPY_MODE_CLONE && mode != COPY_MODE_COMPACT {
        return refuse("mode not supported (only CLONE, COMPACT, and DESERIALIZE)".into());
    }
    let src = AS_REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|r| r.get(&src_va)).cloned();
    let Some(src) = src else {
        return refuse("source was not built by BuildRaytracingAccelerationStructure".into());
    };
    let res = match &src.pending_copy {
        Some(bytes) => unsafe { upload_bvh_to_gpu(this, dst_va, bytes) },
        None => unsafe { gpu_copy_as(this, dst_va, &src) },
    };
    let Some(res) = res else {
        return refuse(format!("{} bytes could not be placed at the destination", src.size));
    };
    let size = src.size;
    let mut rec = src;
    rec.va = dst_va;
    rec.res = res;
    rec.pending_copy = None;
    {
        let mut reg = AS_REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let reg = reg.get_or_insert_with(HashMap::new);
        if mode == COPY_MODE_COMPACT {
            if let Some(s) = reg.get_mut(&src_va) {
                s.pending_copy = None;
            }
        }
        reg.insert(dst_va, rec);
    }
    if once(&LOG, 16) {
        record(M, TAG, &format!("CopyRaytracingAccelerationStructure {} {src_va:#x} -> {dst_va:#x} ({size} bytes)", if mode == COPY_MODE_COMPACT { "COMPACT" } else { "CLONE" }));
    }
}

unsafe fn gpu_copy_as(list: *mut c_void, dst_va: u64, src: &AsRecord) -> Option<usize> {
    let (s, soff) = crate::gpu_va::lookup_gpu(src.va)?;
    let (d, doff) = crate::gpu_va::lookup_gpu(dst_va)?;
    if s.res == 0 || d.res == 0 || s.res == d.res || soff + src.size > s.len || doff + src.size > d.len {
        return None;
    }
    let copy: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *mut c_void, u64, u64) =
        unsafe { core::mem::transmute(vfn(list, PC_CL_COPY_BUFFER_REGION)) };
    unsafe { copy(list, d.res as *mut c_void, doff, s.res as *mut c_void, soff, src.size) };
    unsafe { transition_copy_dest_to_common(list, d.res as *mut c_void) };
    Some(d.res)
}

pub unsafe extern "system" fn set_pipeline_state1_x(this: *mut c_void, state_obj: *mut c_void) {
    ACTIVE_SO.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(this as usize, state_obj as usize);
}

pub unsafe extern "system" fn dispatch_rays_x(this: *mut c_void, pdesc: *const u8) {
    if pdesc.is_null() {
        return;
    }
    let slice = unsafe { core::slice::from_raw_parts(pdesc, DispatchRaysDesc::BYTE_SIZE) };
    let Some(desc) = DispatchRaysDesc::from_bytes(slice) else {
        return;
    };
    if desc.width == 0 || desc.height == 0 {
        return;
    }
    let so_ptr = ACTIVE_SO
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&(this as usize)).copied())
        .unwrap_or(0);
    if so_ptr == 0 {
        if once(&LOG_DISPATCH, 8) {
            record(M, TAG, "DispatchRays called without an active state object (SetPipelineState1)");
        }
        return;
    }

    let so = unsafe { &*(so_ptr as *const RtStateObject) };
    let programs = so.data.programs.programs(true);
    let Some(prog) = programs.iter().rev().find(|p| p.pso != 0 && p.root_sig != 0) else {
        if once(&LOG_DISPATCH, 8) {
            record(M, TAG, &format!("DispatchRays({}x{}x{}): state object has no compiled DXIL compute PSO", desc.width, desc.height, desc.depth));
        }
        return;
    };

    let mut priority = Vec::new();
    for va in [desc.raygen_va, desc.miss_va, desc.hitgroup_va, desc.callable_va] {
        if va != 0 {
            if let Some((e, _)) = crate::gpu_va::lookup_gpu(va) {
                priority.push(GlobalWindow {
                    va: e.start,
                    size: e.len.min(u32::MAX as u64) as u32,
                    res: e.res,
                    writable: crate::gpu_va::is_buffer_uav(e.res),
                });
            }
        }
    }
    if let Some(reg) = AS_REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        for rec in reg.values() {
            if rec.res != 0 {
                priority.push(GlobalWindow {
                    va: rec.va,
                    size: rec.size.min(u32::MAX as u64) as u32,
                    res: rec.res,
                    writable: crate::gpu_va::is_buffer_uav(rec.res),
                });
            }
        }
    }

    let set_rs: unsafe extern "system" fn(*mut c_void, *mut c_void) = unsafe { core::mem::transmute(crate::cmdlist_wrap::real_method(PC_CL_SET_COMPUTE_RS)) };
    let set_pso: unsafe extern "system" fn(*mut c_void, *mut c_void) = unsafe { core::mem::transmute(crate::cmdlist_wrap::real_method(PC_CL_SET_PSO)) };
    let set_consts: unsafe extern "system" fn(*mut c_void, u32, u32, *const u32, u32) =
        unsafe { core::mem::transmute(crate::cmdlist_wrap::real_method(PC_CL_SET_COMPUTE_CONSTS)) };
    let dispatch: unsafe extern "system" fn(*mut c_void, u32, u32, u32) = unsafe { core::mem::transmute(crate::cmdlist_wrap::real_method(PC_CL_DISPATCH)) };

    unsafe {
        set_rs(this, prog.root_sig as *mut c_void);
        crate::cl2_state::reapply_compute_args(this);
        set_pso(this, prog.pso as *mut c_void);
        crate::global_bind::bind_now(this, true, &priority);
        if let Some(i0) = prog.b0_param_idx {
            let c0 = desc.dispatch_constants();
            set_consts(this, i0, c0.len() as u32, c0.as_ptr(), 0);
        }
        if let Some(i1) = prog.b1_param_idx {
            let c1 = [0u32, 0];
            set_consts(this, i1, c1.len() as u32, c1.as_ptr(), 0);
        }
        let (gx, gy, gz) = desc.dispatch_groups();
        dispatch(this, gx, gy, gz);
        crate::cl2_state::reapply_bindings(this);
        crate::cl2_state::rebind_pso(this);
    }

    if once(&LOG_DISPATCH, 12) {
        let (gx, gy, gz) = desc.dispatch_groups();
        record(
            M,
            TAG,
            &format!(
                "DispatchRays({}x{}x{} -> groups {gx}x{gy}x{gz}) raygen={:#x} exports={:?}",
                desc.width, desc.height, desc.depth, desc.raygen_va, prog.exports
            ),
        );
    }
}

pub fn on_close(list: *mut c_void) {
    if let Some(m) = ACTIVE_SO.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(&(list as usize));
    }
}

pub fn install_device(v: &mut [usize]) {
    if v.len() > DEV_SLOT_DESERIALIZE_STATE_OBJECT_X {
        v[DEV_SLOT_CREATE_STATE_OBJECT] = create_state_object_x as usize;
        v[DEV_SLOT_GET_PREBUILD_INFO] = get_prebuild_info_x as usize;
        v[DEV_SLOT_DESERIALIZE_STATE_OBJECT_X] = deserialize_state_object_x as usize;
    }
}

pub unsafe extern "system" fn build_as_batched_x(this: *mut c_void, count: u32, pdescs: *const u8) {
    if pdescs.is_null() || count == 0 {
        return;
    }
    for i in 0..count as usize {
        let entry = unsafe { pdescs.add(i * 0x50) };
        let num_post = unsafe { core::ptr::read_unaligned(entry.add(0x30) as *const u32) };
        let ppost = unsafe { core::ptr::read_unaligned(entry.add(0x38) as *const *const u8) };
        unsafe { build_as_x(this, entry, num_post, ppost) };
    }
}

pub fn install_cmdlist(vt: &[AtomicUsize]) {
    if vt.len() > CL_SLOT_DISPATCH_RAYS {
        vt[CL_SLOT_BUILD_AS].store(build_as_x as usize, Ordering::Relaxed);
        vt[CL_SLOT_EMIT_POSTBUILD_INFO].store(emit_postbuild_info_x as usize, Ordering::Relaxed);
        vt[CL_SLOT_COPY_AS].store(copy_as_x as usize, Ordering::Relaxed);
        vt[CL_SLOT_SET_PIPELINE_STATE1].store(set_pipeline_state1_x as usize, Ordering::Relaxed);
        vt[CL_SLOT_DISPATCH_RAYS].store(dispatch_rays_x as usize, Ordering::Relaxed);
    }
    if vt.len() > CL_SLOT_BUILD_AS_BATCHED_X {
        vt[CL_SLOT_BUILD_AS_BATCHED_X].store(build_as_batched_x as usize, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_tracker_reclaims_batches_only_after_fence_completion() {
        let mut tr = StagingTracker::default();
        tr.record(0x100, 1);
        tr.record(0x100, 2);
        tr.record(0x200, 3);
        let b1 = tr.take_lists(&[0x100]);
        assert_eq!(b1, vec![1, 2]);
        assert!(tr.take_lists(&[0x100]).is_empty());
        tr.queues.insert(0x900, QueueStaging { fence: 0xabc, next: 1, inflight: VecDeque::from([(1, b1)]) });
        let b2 = tr.take_lists(&[0x200]);
        assert_eq!(b2, vec![3]);
        tr.queues.get_mut(&0x900).unwrap().inflight.push_back((2, b2));
        assert!(tr.reclaim(0x900, 0).is_empty());
        assert_eq!(tr.reclaim(0x900, 1), vec![1, 2]);
        assert!(tr.reclaim(0x900, 1).is_empty());
        assert_eq!(tr.reclaim(0x900, 2), vec![3]);
    }

    #[test]
    fn rt_async_setting_is_validated() {
        assert_eq!(parse_rt_async(None), Ok(true));
        assert_eq!(parse_rt_async(Some("")), Ok(true));
        assert_eq!(parse_rt_async(Some("1")), Ok(true));
        assert_eq!(parse_rt_async(Some(" ON ")), Ok(true));
        assert_eq!(parse_rt_async(Some("true")), Ok(true));
        assert_eq!(parse_rt_async(Some("0")), Ok(false));
        assert_eq!(parse_rt_async(Some("off")), Ok(false));
        assert_eq!(parse_rt_async(Some("false")), Ok(false));
        assert!(parse_rt_async(Some("maybe")).is_err());
    }

    #[test]
    fn async_state_object_build_returns_immediately_and_waits_on_first_use() {
        let pool = RtPool::new(2);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let g2 = Arc::clone(&gate);
        let rs = alloc_blob(vec![1]) as usize;
        let pso = alloc_blob(vec![2]) as usize;
        let build = submit_async_batch(&pool, vec![vec!["RayGen".into()]], None, move |_| {
            let (m, cv) = &*g2;
            let mut open = m.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
            ProgramResult {
                exports: vec![("RayGen".into(), ShaderId::new(0, 1, 1))],
                built: Ok(BuiltProgram { root_sig: rs, pso, b0_idx: Some(0), b1_idx: None }),
                translated: true,
                times: BuildTimes::default(),
            }
        });
        assert!(!build.is_ready());
        let b2 = Arc::clone(&build);
        let waiter = std::thread::spawn(move || {
            let p = b2.programs(false);
            assert_eq!(p.len(), 1);
            assert_eq!(p[0].pso, pso);
            assert_eq!(p[0].root_sig, rs);
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(!build.is_ready());
        {
            let (m, cv) = &*gate;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        waiter.join().unwrap();
        assert!(build.is_ready());
        drop(build);
        pool.close();
    }

    #[test]
    fn waiting_on_a_queued_state_object_prioritizes_its_remaining_jobs() {
        let pool = RtPool::new(1);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let order = Arc::new(Mutex::new(Vec::<u32>::new()));
        let g1 = Arc::clone(&gate);
        let o1 = Arc::clone(&order);
        let b1 = submit_async_batch(&pool, vec![vec!["a".into()]], None, move |_| {
            let (m, cv) = &*g1;
            let mut open = m.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
            o1.lock().unwrap().push(1);
            ProgramResult { exports: Vec::new(), built: Err("noop".into()), translated: false, times: BuildTimes::default() }
        });
        let o2 = Arc::clone(&order);
        let b2 = submit_async_batch(&pool, vec![vec!["b".into()]], None, move |_| {
            o2.lock().unwrap().push(2);
            ProgramResult { exports: Vec::new(), built: Err("noop".into()), translated: false, times: BuildTimes::default() }
        });
        let o3 = Arc::clone(&order);
        let b3 = submit_async_batch(&pool, vec![vec!["c".into()]], None, move |_| {
            o3.lock().unwrap().push(3);
            ProgramResult { exports: Vec::new(), built: Err("noop".into()), translated: false, times: BuildTimes::default() }
        });
        let b3w = Arc::clone(&b3);
        let waiter = std::thread::spawn(move || {
            let _ = b3w.programs(false);
        });
        std::thread::sleep(std::time::Duration::from_millis(30));
        {
            let (m, cv) = &*gate;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        waiter.join().unwrap();
        let _ = b1.programs(false);
        let _ = b2.programs(false);
        assert_eq!(*order.lock().unwrap(), vec![1, 3, 2]);
        pool.close();
    }

    #[test]
    fn early_release_of_pending_state_object_still_releases_built_com_objects() {
        let pool = RtPool::new(1);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let g2 = Arc::clone(&gate);
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let d2 = Arc::clone(&done);
        let rs = alloc_blob(vec![10]);
        let pso = alloc_blob(vec![20]);
        unsafe {
            blob_addref(rs);
            blob_addref(pso);
        }
        let (rs_addr, pso_addr) = (rs as usize, pso as usize);
        let build = submit_async_batch(&pool, vec![vec!["e".into()]], None, move |_| {
            let (m, cv) = &*g2;
            let mut open = m.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
            ProgramResult {
                exports: vec![("e".into(), ShaderId::new(0, 1, 2))],
                built: Ok(BuiltProgram { root_sig: rs_addr, pso: pso_addr, b0_idx: None, b1_idx: None }),
                translated: true,
                times: BuildTimes::default(),
            }
        });
        let so = alloc_state_object(core::ptr::null_mut(), HashMap::new(), build, Vec::new());
        assert_eq!(unsafe { so_release(so as *mut RtStateObject) }, 0);
        assert_eq!((refs(rs), refs(pso)), (2, 2));
        {
            let (m, cv) = &*gate;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        let _ = submit_async_batch(&pool, vec![vec!["fence".into()]], None, move |_| {
            let (m, cv) = &*d2;
            *m.lock().unwrap() = true;
            cv.notify_all();
            ProgramResult { exports: Vec::new(), built: Err("noop".into()), translated: false, times: BuildTimes::default() }
        });
        {
            let (m, cv) = &*done;
            let mut fin = m.lock().unwrap();
            while !*fin {
                fin = cv.wait(fin).unwrap();
            }
        }
        assert_eq!((refs(rs), refs(pso)), (1, 1));
        unsafe {
            blob_release(rs);
            blob_release(pso);
        }
        pool.close();
    }

    #[test]
    fn inflight_pso_dedup_builds_once_for_concurrent_waiters() {
        let key = PsoKey { device: 0x5eed_0002, hlsl: 99, rts0: vec![7, 8, 9] };
        let builds = Arc::new(AtomicUsize::new(0));
        let rs = alloc_blob(vec![5]) as usize;
        let pso = alloc_blob(vec![6]) as usize;
        let mut handles = Vec::new();
        for _ in 0..4 {
            let k = key.clone();
            let c = Arc::clone(&builds);
            handles.push(std::thread::spawn(move || {
                let mut times = BuildTimes::default();
                let b = get_or_build_pso(k, &mut times, |_| {
                    c.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(40));
                    Ok(BuiltProgram { root_sig: rs, pso, b0_idx: Some(1), b1_idx: None })
                })
                .unwrap();
                (b, times.reused)
            }));
        }
        let res: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert_eq!(res.iter().filter(|(_, reused)| !*reused).count(), 1);
        assert_eq!(res.iter().filter(|(_, reused)| *reused).count(), 3);
        for (b, _) in res {
            assert_eq!((b.root_sig, b.pso), (rs, pso));
            unsafe {
                com_release(b.pso as *mut c_void);
                com_release(b.root_sig as *mut c_void);
            }
        }
    }

    #[test]
    fn compile_threads_setting_is_validated() {
        assert_eq!(parse_compile_threads(None, 12), Ok(12));
        assert_eq!(parse_compile_threads(Some("auto"), 12), Ok(12));
        assert_eq!(parse_compile_threads(Some(" 0 "), 12), Ok(12));
        assert_eq!(parse_compile_threads(Some("1"), 12), Ok(1));
        assert_eq!(parse_compile_threads(Some("4"), 12), Ok(4));
        assert_eq!(parse_compile_threads(None, 0), Ok(1));
        assert_eq!(parse_compile_threads(None, 500), Ok(MAX_COMPILE_THREADS));
        assert!(parse_compile_threads(Some("65"), 12).is_err());
        assert!(parse_compile_threads(Some("-1"), 12).is_err());
        assert!(parse_compile_threads(Some("fast"), 12).is_err());
    }

    fn result_for(i: usize) -> ProgramResult {
        ProgramResult {
            exports: vec![(format!("e{i}"), ShaderId { bytes: [i as u8; 32] })],
            built: Ok(BuiltProgram { root_sig: i, pso: i + 100, b0_idx: None, b1_idx: Some(i as u32) }),
            translated: true,
            times: BuildTimes::default(),
        }
    }

    #[test]
    fn parallel_program_builds_keep_program_order_and_use_several_threads() {
        let ids = Mutex::new(std::collections::HashSet::new());
        let out = build_programs(16, 4, |i| {
            ids.lock().unwrap().insert(std::thread::current().id());
            std::thread::sleep(std::time::Duration::from_millis(20));
            result_for(i)
        });
        assert_eq!(out.len(), 16);
        for (i, r) in out.iter().enumerate() {
            assert_eq!(r.exports[0].0, format!("e{i}"));
            assert_eq!(r.built.as_ref().unwrap().pso, i + 100);
        }
        assert!(ids.lock().unwrap().len() > 1);
    }

    #[test]
    fn serial_program_builds_stay_on_the_calling_thread() {
        let me = std::thread::current().id();
        let out = build_programs(3, 1, |i| {
            assert_eq!(std::thread::current().id(), me);
            result_for(i)
        });
        assert_eq!(out.iter().map(|r| r.built.as_ref().unwrap().root_sig).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(build_programs(0, 8, result_for).is_empty());
    }

    #[test]
    fn a_panicking_program_build_is_reported_not_propagated() {
        let r = guarded(|| panic!("translator bug {}", 7));
        assert!(!r.translated);
        assert_eq!(r.built.unwrap_err(), "build panicked: translator bug 7");
        let out = build_programs(3, 3, |i| if i == 1 { guarded(|| panic!("boom")) } else { result_for(i) });
        assert!(out[0].built.is_ok() && out[2].built.is_ok());
        assert_eq!(out[1].built.as_ref().unwrap_err(), "build panicked: boom");
    }

    fn refs(b: *mut RtBlob) -> u32 {
        unsafe { (*b).refs.load(Ordering::SeqCst) }
    }

    #[test]
    fn pso_cache_shares_one_root_signature_and_pso_per_program() {
        let (rs, pso) = (alloc_blob(vec![1]), alloc_blob(vec![2]));
        let key = PsoKey { device: 0x5eed_0001, hlsl: 7, rts0: vec![1, 2, 3] };
        assert!(cached_pso(&key).is_none());
        let built = BuiltProgram { root_sig: rs as usize, pso: pso as usize, b0_idx: Some(0), b1_idx: None };
        assert_eq!(remember_pso(key.clone(), built), built);
        assert_eq!((refs(rs), refs(pso)), (2, 2));
        assert_eq!(cached_pso(&key), Some(built));
        assert_eq!((refs(rs), refs(pso)), (3, 3));

        let (rs2, pso2) = (alloc_blob(vec![3]), alloc_blob(vec![4]));
        unsafe {
            blob_addref(rs2);
            blob_addref(pso2);
        }
        let raced = BuiltProgram { root_sig: rs2 as usize, pso: pso2 as usize, b0_idx: Some(0), b1_idx: None };
        assert_eq!(remember_pso(key.clone(), raced), built);
        assert_eq!((refs(rs2), refs(pso2)), (1, 1));
        assert_eq!((refs(rs), refs(pso)), (4, 4));

        let other = PsoKey { rts0: vec![9], ..key.clone() };
        assert!(cached_pso(&other).is_none());
        unsafe {
            blob_release(rs2);
            blob_release(pso2);
        }
    }

    #[test]
    fn raytracing_tier_config_validates_settings() {
        assert_eq!(parse_raytracing_tier(None, true), RAYTRACING_TIER_1_1);
        assert_eq!(parse_raytracing_tier(None, false), RAYTRACING_TIER_NOT_SUPPORTED);
        assert_eq!(parse_raytracing_tier(Some("0"), true), RAYTRACING_TIER_NOT_SUPPORTED);
        assert_eq!(parse_raytracing_tier(Some("off"), true), RAYTRACING_TIER_NOT_SUPPORTED);
        assert_eq!(parse_raytracing_tier(Some("10"), true), RAYTRACING_TIER_1_0);
        assert_eq!(parse_raytracing_tier(Some("1.0"), true), RAYTRACING_TIER_1_0);
        assert_eq!(parse_raytracing_tier(Some("20"), true), RAYTRACING_TIER_1_1);
        assert_eq!(parse_raytracing_tier(Some("1.1"), true), RAYTRACING_TIER_1_1);
    }

    #[test]
    fn dispatch_rays_desc_decodes_104_byte_xbox_abi() {
        let mut raw = [0u8; DispatchRaysDesc::BYTE_SIZE];
        raw[0x00..0x08].copy_from_slice(&0x10_0000_0000u64.to_le_bytes());
        raw[0x08..0x10].copy_from_slice(&0x40u64.to_le_bytes());
        raw[0x10..0x18].copy_from_slice(&0x10_0000_1000u64.to_le_bytes());
        raw[0x18..0x20].copy_from_slice(&0x80u64.to_le_bytes());
        raw[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        raw[0x58..0x5c].copy_from_slice(&1920u32.to_le_bytes());
        raw[0x5c..0x60].copy_from_slice(&1080u32.to_le_bytes());
        raw[0x60..0x64].copy_from_slice(&1u32.to_le_bytes());
        let d = DispatchRaysDesc::from_bytes(&raw).expect("decode DispatchRaysDesc");
        assert_eq!(d.raygen_va, 0x10_0000_0000);
        assert_eq!(d.raygen_size, 0x40);
        assert_eq!(d.miss_va, 0x10_0000_1000);
        assert_eq!(d.dispatch_groups(), (240, 135, 1));
        let c = d.dispatch_constants();
        assert_eq!(c[10], 1920);
        assert_eq!(c[11], 1080);
        assert_eq!(c[12], 1);
    }

    #[test]
    fn state_object_com_properties_and_shader_identifiers_work() {
        let mut exports = HashMap::new();
        exports.insert("OcclusionMainRG".into(), Box::new(ShaderId::new(3, 5, 0x1234)));
        let syms = vec![[0, 0, 0, 0, 0x200, 0x18e1b]];
        let so = alloc_state_object(core::ptr::null_mut(), exports, ProgramBuild::ready(Vec::new()), syms);

        let mut props: *mut c_void = core::ptr::null_mut();
        let hr = unsafe { so_qi(so as *mut RtStateObject, &IID_ID3D12_STATE_OBJECT_PROPERTIES, &mut props) };
        assert_eq!(hr, S_OK);
        assert!(!props.is_null());

        let mut out0 = [0u8; 16];
        assert_eq!(unsafe { props_get_stats_x(props, 0, out0.as_mut_ptr()) }, S_OK);
        let mut out1 = [0u8; 16];
        assert_eq!(unsafe { props_get_stats_x(props, 1, out1.as_mut_ptr()) }, S_OK);
        let sblob = u64::from_le_bytes(out1[0..8].try_into().unwrap()) as *mut RtBlob;
        assert!(!sblob.is_null());
        let sptr = unsafe { blob_ptr(sblob) as *const u8 };
        assert_eq!(unsafe { core::ptr::read_unaligned(sptr.add(8) as *const u32) }, 1);

        let mut meta: *mut c_void = core::ptr::null_mut();
        assert_eq!(unsafe { so_get_meta_x(so as *mut RtStateObject, &mut meta) }, S_OK);
        let mut mblob: *mut c_void = core::ptr::null_mut();
        assert_eq!(unsafe { meta_get_blob_x(meta, 1, 0, &mut mblob) }, S_OK);
        assert!(!mblob.is_null());
        let mptr = unsafe { blob_ptr(mblob as *mut RtBlob) as *const u8 };
        assert_eq!(unsafe { core::ptr::read_unaligned(mptr.add(16) as *const u32) }, 1);
        assert_eq!(unsafe { blob_release(mblob as *mut RtBlob) }, 0);

        let name: Vec<u16> = "OcclusionMainRG".encode_utf16().chain(Some(0)).collect();
        let id_ptr = unsafe { props_get_shader_id(props, name.as_ptr()) };
        assert!(!id_ptr.is_null());
        let id = unsafe { *(id_ptr as *const ShaderId) };
        assert_eq!(id.program_index(), 3);
        assert_eq!(id.continuation_id(), 5);

        unsafe { props_set_pipeline_stack_size(props, 8192) };
        assert_eq!(unsafe { props_get_pipeline_stack_size(props) }, 8192);

        assert_eq!(unsafe { props_release(props) }, 1);
        assert_eq!(unsafe { so_release(so as *mut RtStateObject) }, 0);
    }

    #[test]
    fn title_cbv_bindings_skip_root_constants_and_the_internal_space() {
        use tombox_xshader::meta::{RootParam, RootParamKind, RootRange, RootSig};
        let range = |range_type, base_register, space| RootRange { range_type, num: 1, base_register, space, offset_in_table: 0 };
        let rs = RootSig {
            version: 2,
            flags: 0,
            params: vec![
                RootParam { visibility: 0, kind: RootParamKind::Constants { register: 0, space: XDXR_INTERNAL_SPACE, num32: 13 } },
                RootParam { visibility: 0, kind: RootParamKind::Constants { register: 0, space: 1, num32: 4 } },
                RootParam { visibility: 0, kind: RootParamKind::Descriptor { kind: 2, register: 3, space: 1 } },
                RootParam { visibility: 0, kind: RootParamKind::Descriptor { kind: 3, register: 0, space: 1 } },
                RootParam { visibility: 0, kind: RootParamKind::Table(vec![range(0, 0, 0), range(2, 5, 2), range(2, 0, XDXR_INTERNAL_SPACE)]) },
            ],
            num_static_samplers: 0,
            static_samplers: Vec::new(),
        };
        assert_eq!(title_cbv_bindings(&rs), vec![(3, 1), (5, 2)]);
    }

    #[test]
    fn only_used_title_cbvs_count_as_reads() {
        let decl = "cbuffer TbRtTb_1_0 : register(b0, space1) { uint4 tb_rttb_1_0[64]; };\n";
        assert!(!reads_title_cbv(decl));
        assert!(reads_title_cbv(&format!("{decl}uint x = tb_rttb_1_0[2].y;\n")));
        assert!(reads_title_cbv("  uint4 c = tb_rtcb_1_3[0];\n"));
        assert!(!reads_title_cbv("uint4 c = tb_rtc_1_0[0];\n")); // root constants are not CBV descriptors
    }

    #[test]
    fn postbuild_records_cover_compacted_and_current_size_only() {
        assert_eq!(postbuild_record(POSTBUILD_COMPACTED_SIZE, 0x1234), Some(0x1234u64.to_le_bytes().to_vec()));
        assert_eq!(postbuild_record(POSTBUILD_CURRENT_SIZE, 64), Some(64u64.to_le_bytes().to_vec()));
        assert_eq!(postbuild_record(1, 64), None); // TOOLS_VISUALIZATION
        assert_eq!(postbuild_record(2, 64), None); // SERIALIZATION
    }

    #[test]
    fn copy_as_refuses_without_registering_and_keeps_pending_bytes() {
        let src_va = 0x7e57_0000_0000u64;
        let dst_va = 0x7e57_0001_0000u64;
        unsafe { copy_as_x(core::ptr::null_mut(), dst_va, src_va, 3) };
        unsafe { copy_as_x(core::ptr::null_mut(), dst_va, src_va, COPY_MODE_COMPACT) };
        let has = |va: u64| AS_REGISTRY.lock().unwrap().as_ref().map_or(false, |r| r.contains_key(&va));
        assert!(!has(dst_va));

        AS_REGISTRY.lock().unwrap().get_or_insert_with(HashMap::new).insert(
            src_va,
            AsRecord {
                va: src_va,
                size: 128,
                res: 0,
                as_type: AS_TYPE_BOTTOM_LEVEL,
                bounds: Aabb { min: [0.0; 3], max: [1.0; 3] },
                blas_vas: Vec::new(),
                pending_copy: Some(std::sync::Arc::new(vec![0u8; 128])),
            },
        );
        unsafe { copy_as_x(core::ptr::null_mut(), dst_va, src_va, COPY_MODE_COMPACT) };
        assert!(!has(dst_va));
        let reg = AS_REGISTRY.lock().unwrap();
        assert!(reg.as_ref().unwrap()[&src_va].pending_copy.is_some());
        drop(reg);
        AS_REGISTRY.lock().unwrap().as_mut().unwrap().remove(&src_va);
    }

    #[test]
    fn prebuild_info_handles_tlas_and_blas_raw_inputs() {
        let mut tlas_in = [0u8; 24];
        tlas_in[0..4].copy_from_slice(&AS_TYPE_TOP_LEVEL.to_le_bytes());
        tlas_in[8..12].copy_from_slice(&16u32.to_le_bytes());
        let mut out = PrebuildInfo::default();
        unsafe { get_prebuild_info_x(core::ptr::null_mut(), tlas_in.as_ptr(), &mut out) };
        assert!(out.result_data_max_size >= 16 * 128);
        assert_eq!(out.result_data_max_size % 256, 0);
        assert!(out.scratch_data_size >= 4096);
    }

    #[test]
    fn serialized_as_header_and_tlas_pointer_fixup_match_console_abi() {
        let mut hdr = [0u8; 56];
        hdr[0x20..0x28].copy_from_slice(&0x1040u64.to_le_bytes());
        hdr[0x28..0x30].copy_from_slice(&0x1000u64.to_le_bytes());
        hdr[0x30..0x38].copy_from_slice(&0u64.to_le_bytes());
        assert_eq!(parse_serialized_as_header(&hdr), Some((0x40, 0x1000, 0)));

        hdr[0x20..0x28].copy_from_slice(&0x1050u64.to_le_bytes());
        hdr[0x30..0x38].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(parse_serialized_as_header(&hdr), Some((0x40, 0x1000, 1)));

        hdr[0x30..0x38].copy_from_slice(&2u64.to_le_bytes());
        assert_eq!(parse_serialized_as_header(&hdr), Some((0x50, 0x1000, 2)));

        hdr[0x28..0x30].copy_from_slice(&0u64.to_le_bytes());
        assert_eq!(parse_serialized_as_header(&hdr), None);

        let mut tlas = vec![0u8; 0x80 + 256];
        tlas[0x34..0x38].copy_from_slice(&0x80u32.to_le_bytes());
        let ptrs = [0x0000_0003_0c00_1200u64, 0x0000_0003_0c00_3400u64];
        assert!(fixup_deserialized_tlas_ptrs(&mut tlas, &ptrs));
        assert_eq!(u32::from_le_bytes(tlas[0x80 + 60..0x80 + 64].try_into().unwrap()), (ptrs[0] >> 8) as u32);
        assert_eq!(u32::from_le_bytes(tlas[0x80 + 128 + 60..0x80 + 128 + 64].try_into().unwrap()), (ptrs[1] >> 8) as u32);
    }

    #[test]
    fn optional_real_state_object_programs_create_pc_psos() {
        let Ok(path) = std::env::var("TOMBOX_TEST_STATEOBJ") else { return };
        if !tombox_xshader::dxc::available() {
            eprintln!("skipped: DXC unavailable");
            return;
        }
        let blob = std::fs::read(&path).expect("read stateobj");
        let so = state_obj::parse_state_object(&blob).expect("parse stateobj");
        let dev = unsafe {
            if std::env::var("TOMBOX_D3D12_DEBUG").is_ok() {
                let p = crate::host_proc(b"D3D12GetDebugInterface\0");
                assert!(p != 0);
                let get: unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(p);
                let mut dbg: *mut c_void = core::ptr::null_mut();
                assert!(get(&crate::IID_ID3D12DEBUG, &mut dbg) >= 0 && !dbg.is_null());
                let enable: unsafe extern "system" fn(*mut c_void) = core::mem::transmute(vfn(dbg, 3));
                enable(dbg);
                com_release(dbg);
            }
            let p = crate::host_proc(b"D3D12CreateDevice\0");
            assert!(p != 0);
            let mk: unsafe extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(p);
            let mut d: *mut c_void = core::ptr::null_mut();
            assert!(mk(core::ptr::null_mut(), 0xc100, &IID_DEVICE, &mut d) >= 0 && !d.is_null());
            d
        };
        let dev_addr = dev as usize;
        let build_all = || build_programs(so.programs.len(), 4, |i| build_program_guarded(dev_addr, &so.programs[i], i as u32));
        let first = build_all();
        let failures: Vec<String> = first.iter().enumerate().filter_map(|(i, r)| r.built.as_ref().err().map(|e| format!("prog#{i}: {e}"))).collect();
        assert!(failures.is_empty(), "{failures:#?}");
        assert!(first.iter().all(|r| r.translated && !r.times.reused));
        let second = build_all();
        for (a, b) in first.iter().zip(&second) {
            assert!(b.times.reused);
            assert_eq!(a.built.as_ref().unwrap(), b.built.as_ref().unwrap());
        }
        for r in first.iter().chain(&second) {
            let b = r.built.as_ref().unwrap();
            unsafe {
                com_release(b.pso as *mut c_void);
                com_release(b.root_sig as *mut c_void);
            }
        }
        unsafe { com_release(dev) };
    }
}
