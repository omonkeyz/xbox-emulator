
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::collections::HashMap;
use std::sync::Mutex;
use compat_common::record;
use tombox_xshader::global_abi::{
    window_cb_flags, Window, TB_GLOBAL_CBV, TB_GLOBAL_CB_BYTES, TB_GLOBAL_FAULT_OFFSET,
    TB_GLOBAL_ROOT_DWORDS, TB_GLOBAL_SPACE, TB_GLOBAL_SRV_OFFSET, TB_GLOBAL_TABLE_DESCRIPTORS,
    TB_GLOBAL_UAV_OFFSET, TB_GLOBAL_WINDOWS,
};

use crate::device_wrap::Guid;
use crate::gds::{PARAM_TABLE, ROOT_DWORD_LIMIT, ROOT_FLAG_LOCAL};

const M: &str = "d3d12_xs";
const TAG: &str = "global_bind";

const PARAM_SIZE: usize = 32;
const RANGE1_SIZE: usize = 24;
const VERSION_1_1: u32 = 2;
const RANGE_FLAG_VOLATILE: u32 = 0x1 | 0x2; // DESCRIPTORS_VOLATILE | DATA_VOLATILE
const MAX_TIER1_HEAP: u32 = 1_000_000;

pub const TAIL_RING: u32 = 8;
pub const TAIL_SLOTS: u32 = TAIL_RING * TB_GLOBAL_TABLE_DESCRIPTORS as u32;

const XBOX_SET_DESCRIPTOR_HEAPS: usize = 32;
const XBOX_SET_COMPUTE_ROOT_SIGNATURE: usize = 33;
const XBOX_SET_GRAPHICS_ROOT_SIGNATURE: usize = 34;

const PC_SET_DESCRIPTOR_HEAPS: usize = 28;
const PC_SET_COMPUTE_ROOT_TABLE: usize = 31;
const PC_SET_GRAPHICS_ROOT_TABLE: usize = 32;

const DEV_CREATE_DESCRIPTOR_HEAP: usize = 14;
const DEV_GET_DESC_INC: usize = 15;
const DEV_CREATE_CBV: usize = 17;
const DEV_CREATE_SRV: usize = 18;
const DEV_CREATE_UAV: usize = 19;
const DEV_CREATE_COMMITTED: usize = 27;
const RES_MAP: usize = 8;
const RES_GET_GPU_VA: usize = 11;

const STATE_COMMON: u32 = 0;
const STATE_GENERIC_READ: u32 = 0xac3;

const IID_DESERIALIZER: Guid = Guid { d1: 0x7f91_ce67, d2: 0x090c, d3: 0x4bb7, d4: [0xb7, 0x8e, 0xed, 0x8f, 0xf2, 0xe3, 0x1d, 0xa0] };
const IID_RESOURCE: Guid = Guid { d1: 0x6964_42be, d2: 0xa72e, d3: 0x4059, d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad] };
const IID_DEVICE: Guid = Guid { d1: 0x1898_19f1, d2: 0x1db6, d3: 0x4b57, d4: [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7] };
const IID_DESC_HEAP: Guid = Guid { d1: 0x8efb_471d, d2: 0x616c, d3: 0x4f49, d4: [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51] };

#[derive(Clone, Copy, Debug)]
struct TailHeap {
    device: usize,
    base_slot: u32,
    cpu_start: u64,
    gpu_start: u64,
    inc: u32,
    cursor: u32,
}

#[derive(Clone, Copy, Default)]
struct ListState {
    cbv_heap: usize,
    sampler_heap: usize,
    croot: usize,
    groot: usize,
}

struct Backing {
    device: usize,
    cb_bufs: Vec<(usize, u64, *mut u8)>, // (res, gpu_va, mapped_cpu)
    fault_res: usize,
    fallback_heap: usize,
    fallback_tail: TailHeap,
    ring_cursor: usize,
}

unsafe impl Send for Backing {}

static ROOTS: Mutex<Option<HashMap<usize, u32>>> = Mutex::new(None);
static HEAPS: Mutex<Option<HashMap<usize, TailHeap>>> = Mutex::new(None);
static LISTS: Mutex<Option<HashMap<usize, ListState>>> = Mutex::new(None);
static BACKING: Mutex<Option<Backing>> = Mutex::new(None);

static PREV_HEAPS: AtomicUsize = AtomicUsize::new(0);
static PREV_CROOT: AtomicUsize = AtomicUsize::new(0);
static PREV_GROOT: AtomicUsize = AtomicUsize::new(0);
static LOG: AtomicUsize = AtomicUsize::new(0);
static SKIP_LOG: AtomicUsize = AtomicUsize::new(0);

fn once(n: usize) -> bool {
    LOG.fetch_add(1, Ordering::Relaxed) < n
}

fn parse_enabled(v: Option<&str>) -> bool {
    matches!(v.map(str::trim), Some(s) if !s.is_empty() && s != "0")
}

pub fn enabled() -> bool {
    if cfg!(test) {
        return true;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| parse_enabled(std::env::var("TOMBOX_GLOBAL_BIND").ok().as_deref()))
}

pub(crate) fn refusal(needs_global: bool) -> Option<String> {
    if !needs_global || enabled() {
        return None;
    }
    Some(format!(
        "shader reads or writes global memory through a 64-bit GPU VA, and TbGlobal binding (space {TB_GLOBAL_SPACE}) is off (set TOMBOX_GLOBAL_BIND=1 to enable it)"
    ))
}

unsafe fn vslot(obj: *mut c_void, i: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(i) }
}

fn u32_at(p: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(p[o..o + 4].try_into().unwrap())
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn range1(range_type: u32, num: u32, base_reg: u32, space: u32, flags: u32, offset: u32) -> [u8; RANGE1_SIZE] {
    let mut r = [0u8; RANGE1_SIZE];
    put32(&mut r, 0, range_type);
    put32(&mut r, 4, num);
    put32(&mut r, 8, base_reg);
    put32(&mut r, 12, space);
    put32(&mut r, 16, flags);
    put32(&mut r, 20, offset);
    r
}

pub fn descriptor_ranges() -> [[u8; RANGE1_SIZE]; 3] {
    [
        range1(2, 1, TB_GLOBAL_CBV, TB_GLOBAL_SPACE, RANGE_FLAG_VOLATILE, 0),
        range1(0, TB_GLOBAL_WINDOWS as u32, 0, TB_GLOBAL_SPACE, RANGE_FLAG_VOLATILE, TB_GLOBAL_SRV_OFFSET as u32),
        range1(1, (TB_GLOBAL_WINDOWS + 1) as u32, 0, TB_GLOBAL_SPACE, RANGE_FLAG_VOLATILE, TB_GLOBAL_UAV_OFFSET as u32),
    ]
}

pub fn table_param(ranges_ptr: u64) -> [u8; PARAM_SIZE] {
    let mut p = [0u8; PARAM_SIZE];
    put32(&mut p, 0, PARAM_TABLE);
    put32(&mut p, 8, 3); // NumDescriptorRanges
    put64(&mut p, 16, ranges_ptr);
    put32(&mut p, 24, 0); // D3D12_SHADER_VISIBILITY_ALL
    p
}

pub fn append_plan(params: &[u8], flags: u32, ranges_ptr: u64) -> Option<(Vec<u8>, u32)> {
    if !enabled() || flags & ROOT_FLAG_LOCAL != 0 {
        return None;
    }
    if crate::gds::root_cost(params) + TB_GLOBAL_ROOT_DWORDS > ROOT_DWORD_LIMIT {
        return None;
    }
    let idx = (params.len() / PARAM_SIZE) as u32;
    let mut v = params.to_vec();
    v.extend_from_slice(&table_param(ranges_ptr));
    Some((v, idx))
}

pub unsafe fn patch_root_signature(blob: *const u8, len: usize) -> Option<(Vec<u8>, u32)> {
    if !enabled() || blob.is_null() || len == 0 {
        return None;
    }
    let mk = crate::host_proc(b"D3D12CreateVersionedRootSignatureDeserializer\0");
    let ser = crate::host_proc(b"D3D12SerializeVersionedRootSignature\0");
    if mk == 0 || ser == 0 {
        return None;
    }
    let mk: unsafe extern "system" fn(*const u8, usize, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(mk) };
    let mut d: *mut c_void = core::ptr::null_mut();
    if unsafe { mk(blob, len, &IID_DESERIALIZER, &mut d) } < 0 || d.is_null() {
        return None;
    }
    let get: unsafe extern "system" fn(*mut c_void, u32, *mut *const u8) -> i32 = unsafe { core::mem::transmute(vslot(d, 3)) };
    let mut desc: *const u8 = core::ptr::null();
    let mut out = None;
    if unsafe { get(d, VERSION_1_1, &mut desc) } >= 0 && !desc.is_null() {
        let vd = unsafe { core::slice::from_raw_parts(desc, 48) }.to_vec();
        let n = u32_at(&vd, 8) as usize;
        let pp = u64::from_le_bytes(vd[16..24].try_into().unwrap()) as *const u8;
        let params = if n == 0 || pp.is_null() { Vec::new() } else { unsafe { core::slice::from_raw_parts(pp, n * PARAM_SIZE) }.to_vec() };
        let already_patched = (0..n).any(|i| {
            let p = &params[i * PARAM_SIZE..(i + 1) * PARAM_SIZE];
            if u32_at(p, 0) != PARAM_TABLE {
                return false;
            }
            let nr = u32_at(p, 8) as usize;
            let rp = u64::from_le_bytes(p[16..24].try_into().unwrap()) as *const u8;
            if nr == 0 || rp.is_null() {
                return false;
            }
            (0..nr).any(|j| {
                let r = unsafe { core::slice::from_raw_parts(rp.add(j * RANGE1_SIZE), RANGE1_SIZE) };
                u32_at(r, 12) == TB_GLOBAL_SPACE
            })
        });
        let ranges = descriptor_ranges();
        let plan = if already_patched { None } else { append_plan(&params, u32_at(&vd, 40), ranges.as_ptr() as u64) };
        if plan.is_none() && SKIP_LOG.fetch_add(1, Ordering::Relaxed) < 40 {
            record(M, TAG, &format!("root signature not given a TbGlobal table: already_patched={already_patched} params={n} cost={} flags={:#x}", crate::gds::root_cost(&params), u32_at(&vd, 40)));
        }
        if !already_patched {
        if let Some((np, idx)) = plan {
            let mut nd = vd.clone();
            put32(&mut nd, 8, (n + 1) as u32);
            put64(&mut nd, 16, np.as_ptr() as u64);
            let ser: unsafe extern "system" fn(*const u8, *mut *mut c_void, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(ser) };
            let (mut b, mut e): (*mut c_void, *mut c_void) = (core::ptr::null_mut(), core::ptr::null_mut());
            let hr = unsafe { ser(nd.as_ptr(), &mut b, &mut e) };
            if hr >= 0 && !b.is_null() {
                let ptr: unsafe extern "system" fn(*mut c_void) -> *const u8 = unsafe { core::mem::transmute(vslot(b, 3)) };
                let size: unsafe extern "system" fn(*mut c_void) -> usize = unsafe { core::mem::transmute(vslot(b, 4)) };
                out = Some((unsafe { core::slice::from_raw_parts(ptr(b), size(b)) }.to_vec(), idx));
            } else if once(8) {
                let why = if e.is_null() {
                    String::new()
                } else {
                    let ptr: unsafe extern "system" fn(*mut c_void) -> *const u8 = unsafe { core::mem::transmute(vslot(e, 3)) };
                    let size: unsafe extern "system" fn(*mut c_void) -> usize = unsafe { core::mem::transmute(vslot(e, 4)) };
                    let raw = unsafe { core::slice::from_raw_parts(ptr(e), size(e)) };
                    String::from_utf8_lossy(raw).trim_end_matches(['\0', '\r', '\n']).to_string()
                };
                record(M, TAG, &format!("re-serializing root signature with TbGlobal table FAILED hr={hr:#010x}: {why}"));
            }
            for o in [b, e] {
                if !o.is_null() {
                    let r: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(vslot(o, 2)) };
                    unsafe { r(o) };
                }
            }
        }
        }
    }
    let r: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(vslot(d, 2)) };
    unsafe { r(d) };
    out
}

pub fn note_root_signature(rs: *mut c_void, idx: u32) {
    if !rs.is_null() {
        ROOTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(rs as usize, idx);
    }
}

pub fn root_index(rs: *mut c_void) -> Option<u32> {
    if rs.is_null() {
        return None;
    }
    ROOTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&(rs as usize)).copied())
}

pub fn grow_heap_desc(desc: &[u32; 4]) -> Option<([u32; 4], u32)> {
    if !enabled() {
        return None;
    }
    let (htype, num, flags, node) = (desc[0], desc[1], desc[2], desc[3]);
    if htype != 0 || (flags & 1) == 0 || num == 0 {
        return None;
    }
    let grown = num.checked_add(TAIL_SLOTS)?;
    if grown > MAX_TIER1_HEAP {
        return None;
    }
    Some(([htype, grown, flags, node], num))
}

pub unsafe fn note_heap(real_dev: *mut c_void, heap: *mut c_void, orig_num: u32) {
    if real_dev.is_null() || heap.is_null() {
        return;
    }
    let inc_f: unsafe extern "system" fn(*mut c_void, u32) -> u32 = unsafe { core::mem::transmute(vslot(real_dev, DEV_GET_DESC_INC)) };
    let inc = unsafe { inc_f(real_dev, 0) };
    let hv = unsafe { *(heap as *const *const usize) };
    let get_cpu: unsafe extern "system" fn(*mut c_void) -> u64 = unsafe { core::mem::transmute(*hv.add(9)) };
    let get_gpu: unsafe extern "system" fn(*mut c_void) -> u64 = unsafe { core::mem::transmute(*hv.add(10)) };
    let cpu_start = unsafe { get_cpu(heap) };
    let gpu_start = unsafe { get_gpu(heap) };
    if cpu_start == 0 || gpu_start == 0 || inc == 0 {
        return;
    }
    HEAPS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(
        heap as usize,
        TailHeap { device: real_dev as usize, base_slot: orig_num, cpu_start, gpu_start, inc, cursor: 0 },
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlobalWindow {
    pub va: u64,
    pub size: u32,
    pub res: usize,
    pub writable: bool,
}

pub fn select_windows(priority: &[GlobalWindow], tracked: &[(crate::gpu_va::Entry, bool)]) -> Vec<GlobalWindow> {
    let mut out: Vec<GlobalWindow> = Vec::with_capacity(TB_GLOBAL_WINDOWS);
    let push_if_new = |out: &mut Vec<GlobalWindow>, w: GlobalWindow| {
        if w.size < 4 || w.res == 0 || out.len() >= TB_GLOBAL_WINDOWS {
            return;
        }
        if out.iter().any(|x| x.va == w.va) {
            return;
        }
        out.push(w);
    };
    for &w in priority {
        push_if_new(&mut out, w);
    }
    for &(e, uav) in tracked.iter().filter(|(_, u)| *u) {
        let sz = e.len.min(u32::MAX as u64) as u32;
        push_if_new(&mut out, GlobalWindow { va: e.start, size: sz, res: e.res, writable: uav });
    }
    for &(e, uav) in tracked.iter().filter(|(_, u)| !*u) {
        let sz = e.len.min(u32::MAX as u64) as u32;
        push_if_new(&mut out, GlobalWindow { va: e.start, size: sz, res: e.res, writable: uav });
    }
    out
}

unsafe fn create_buffer(dev: *mut c_void, bytes: u64, heap_type: u32, init_state: u32, allow_uav: bool) -> Option<*mut c_void> {
    let heap: [u32; 5] = [heap_type, 0, 0, 1, 1];
    let mut desc = [0u8; 56];
    put32(&mut desc, 0, 1); // BUFFER
    put64(&mut desc, 0x10, bytes);
    put32(&mut desc, 0x18, 1);
    desc[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
    desc[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
    put32(&mut desc, 0x24, 1);
    put32(&mut desc, 0x2c, 1); // ROW_MAJOR
    put32(&mut desc, 0x30, if allow_uav { 4 } else { 0 });
    let create: unsafe extern "system" fn(*mut c_void, *const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_COMMITTED)) };
    let mut res: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { create(dev, heap.as_ptr(), 0, desc.as_ptr(), init_state, core::ptr::null(), &IID_RESOURCE, &mut res) };
    if hr < 0 || res.is_null() { None } else { Some(res) }
}

unsafe fn ensure_backing(list: *mut c_void) -> bool {
    let mut g = BACKING.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_some() {
        return true;
    }
    let get: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(list, 7)) };
    let mut dev: *mut c_void = core::ptr::null_mut();
    if unsafe { get(list, &IID_DEVICE, &mut dev) } < 0 || dev.is_null() {
        return false;
    }
    let mut cb_bufs = Vec::with_capacity(TAIL_RING as usize);
    for _ in 0..TAIL_RING {
        let Some(res) = (unsafe { create_buffer(dev, TB_GLOBAL_CB_BYTES as u64, 2, STATE_GENERIC_READ, false) }) else {
            return false;
        };
        let get_va: unsafe extern "system" fn(*mut c_void) -> u64 = unsafe { core::mem::transmute(vslot(res, RES_GET_GPU_VA)) };
        let va = unsafe { get_va(res) };
        let map: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut u8) -> i32 = unsafe { core::mem::transmute(vslot(res, RES_MAP)) };
        let mut ptr: *mut u8 = core::ptr::null_mut();
        if unsafe { map(res, 0, core::ptr::null(), &mut ptr) } < 0 || ptr.is_null() {
            return false;
        }
        cb_bufs.push((res as usize, va, ptr));
    }
    let Some(fault_res) = (unsafe { create_buffer(dev, 256, 1, STATE_COMMON, true) }) else {
        return false;
    };
    let hdesc: [u32; 4] = [0, TAIL_SLOTS, 1, 0];
    let mk_heap: unsafe extern "system" fn(*mut c_void, *const u32, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_DESCRIPTOR_HEAP)) };
    let mut fallback_heap: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { mk_heap(dev, hdesc.as_ptr(), &IID_DESC_HEAP, &mut fallback_heap) };
    if hr < 0 || fallback_heap.is_null() {
        return false;
    }
    crate::heap_wrap::ensure_patched(fallback_heap);
    let inc_f: unsafe extern "system" fn(*mut c_void, u32) -> u32 = unsafe { core::mem::transmute(vslot(dev, DEV_GET_DESC_INC)) };
    let inc = unsafe { inc_f(dev, 0) };
    let hv = unsafe { *(fallback_heap as *const *const usize) };
    let get_cpu: unsafe extern "system" fn(*mut c_void) -> u64 = unsafe { core::mem::transmute(*hv.add(9)) };
    let get_gpu: unsafe extern "system" fn(*mut c_void) -> u64 = unsafe { core::mem::transmute(*hv.add(10)) };
    let fallback_tail = TailHeap {
        device: dev as usize,
        base_slot: 0,
        cpu_start: unsafe { get_cpu(fallback_heap) },
        gpu_start: unsafe { get_gpu(fallback_heap) },
        inc,
        cursor: 0,
    };
    record(M, TAG, &format!("TbGlobal backing ready: {TAIL_RING} CBV ring slots ({TB_GLOBAL_CB_BYTES}B), fault UAV {fault_res:p}, fallback heap {fallback_heap:p}"));
    *g = Some(Backing {
        device: dev as usize,
        cb_bufs,
        fault_res: fault_res as usize,
        fallback_heap: fallback_heap as usize,
        fallback_tail,
        ring_cursor: 0,
    });
    true
}

unsafe fn write_raw_srv(dev: *mut c_void, res: *mut c_void, num_dwords: u32, handle: u64) {
    let mut sd = [0u8; 40];
    put32(&mut sd, 0, 39); // DXGI_FORMAT_R32_TYPELESS
    put32(&mut sd, 4, 1);  // D3D12_SRV_DIMENSION_BUFFER
    put32(&mut sd, 8, 0x1688); // DEFAULT_COMPONENT_MAPPING
    put32(&mut sd, 24, num_dwords.max(1));
    put32(&mut sd, 32, 1); // D3D12_BUFFER_SRV_FLAG_RAW
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, u64) = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_SRV)) };
    unsafe { f(dev, res, sd.as_ptr(), handle) };
}

unsafe fn write_raw_uav(dev: *mut c_void, res: *mut c_void, num_dwords: u32, handle: u64) {
    let mut ud = [0u8; 40];
    put32(&mut ud, 0, 39); // DXGI_FORMAT_R32_TYPELESS
    put32(&mut ud, 4, 1);  // D3D12_UAV_DIMENSION_BUFFER
    put32(&mut ud, 16, num_dwords.max(1));
    put32(&mut ud, 32, 1); // D3D12_BUFFER_UAV_FLAG_RAW
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u8, u64) = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_UAV)) };
    unsafe { f(dev, res, core::ptr::null_mut(), ud.as_ptr(), handle) };
}

pub unsafe fn bind_now(list: *mut c_void, compute: bool, priority: &[GlobalWindow]) -> bool {
    if !enabled() || list.is_null() {
        return false;
    }
    let ls = LISTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&(list as usize)).copied()).unwrap_or_default();
    let rs = if compute { ls.croot } else { ls.groot };
    let Some(root_idx) = root_index(rs as *mut c_void) else {
        return false;
    };
    if !unsafe { ensure_backing(list) } {
        return false;
    }
    let tracked = crate::gpu_va::snapshot_buffers();
    let windows = select_windows(priority, &tracked);
    let cb_in: Vec<(Window, bool)> = windows.iter().map(|w| (Window { base: w.va, size: w.size }, w.writable)).collect();
    let cb_words = window_cb_flags(&cb_in, [0, 0]);

    let mut use_fallback = false;
    let (dev, cpu_base, gpu_base, inc) = {
        let mut hmap = HEAPS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(th) = hmap.as_mut().and_then(|m| m.get_mut(&ls.cbv_heap)) {
            let slot_idx = th.cursor % TAIL_RING;
            th.cursor = th.cursor.wrapping_add(1);
            let off = (th.base_slot + slot_idx * TB_GLOBAL_TABLE_DESCRIPTORS as u32) as u64 * th.inc as u64;
            (th.device as *mut c_void, th.cpu_start + off, th.gpu_start + off, th.inc as u64)
        } else {
            use_fallback = true;
            let mut bg = BACKING.lock().unwrap_or_else(|e| e.into_inner());
            let b = bg.as_mut().unwrap();
            let slot_idx = b.fallback_tail.cursor % TAIL_RING;
            b.fallback_tail.cursor = b.fallback_tail.cursor.wrapping_add(1);
            let off = (slot_idx * TB_GLOBAL_TABLE_DESCRIPTORS as u32) as u64 * b.fallback_tail.inc as u64;
            (b.device as *mut c_void, b.fallback_tail.cpu_start + off, b.fallback_tail.gpu_start + off, b.fallback_tail.inc as u64)
        }
    };

    let (cb_va, fault_res, fallback_heap) = {
        let mut bg = BACKING.lock().unwrap_or_else(|e| e.into_inner());
        let b = bg.as_mut().unwrap();
        let idx = b.ring_cursor % b.cb_bufs.len();
        b.ring_cursor = b.ring_cursor.wrapping_add(1);
        let (_, va, ptr) = b.cb_bufs[idx];
        unsafe {
            core::ptr::write_bytes(ptr, 0, TB_GLOBAL_CB_BYTES);
            core::ptr::copy_nonoverlapping(cb_words.as_ptr() as *const u8, ptr, cb_words.len() * 4);
        }
        (va, b.fault_res as *mut c_void, b.fallback_heap as *mut c_void)
    };

    let cv = [cb_va, TB_GLOBAL_CB_BYTES as u64];
    let mk_cbv: unsafe extern "system" fn(*mut c_void, *const u64, u64) = unsafe { core::mem::transmute(vslot(dev, DEV_CREATE_CBV)) };
    unsafe { mk_cbv(dev, cv.as_ptr(), cpu_base) };

    for i in 0..TB_GLOBAL_WINDOWS {
        let srv_h = cpu_base + (TB_GLOBAL_SRV_OFFSET + i) as u64 * inc;
        let uav_h = cpu_base + (TB_GLOBAL_UAV_OFFSET + i) as u64 * inc;
        if let Some(w) = windows.get(i) {
            let ndw = (w.size / 4).max(1);
            if w.writable {
                unsafe {
                    write_raw_srv(dev, core::ptr::null_mut(), 1, srv_h);
                    write_raw_uav(dev, w.res as *mut c_void, ndw, uav_h);
                }
            } else {
                unsafe {
                    write_raw_srv(dev, w.res as *mut c_void, ndw, srv_h);
                    write_raw_uav(dev, core::ptr::null_mut(), 1, uav_h);
                }
            }
        } else {
            unsafe {
                write_raw_srv(dev, core::ptr::null_mut(), 1, srv_h);
                write_raw_uav(dev, core::ptr::null_mut(), 1, uav_h);
            }
        }
    }
    unsafe { write_raw_uav(dev, fault_res, 64, cpu_base + TB_GLOBAL_FAULT_OFFSET as u64 * inc) };

    if use_fallback && ls.cbv_heap == 0 {
        let mut heaps = [fallback_heap, core::ptr::null_mut()];
        let n = if ls.sampler_heap != 0 {
            heaps[1] = ls.sampler_heap as *mut c_void;
            2u32
        } else {
            1u32
        };
        let set_heaps: unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void) =
            unsafe { core::mem::transmute(vslot(list, PC_SET_DESCRIPTOR_HEAPS)) };
        unsafe { set_heaps(list, n, heaps.as_ptr()) };
    } else if use_fallback && once(8) {
        record(M, TAG, "active title CBV_SRV_UAV heap lacks TbGlobal tail; skipping table bind to avoid displacing title heap");
        return false;
    }

    let set_tbl: unsafe extern "system" fn(*mut c_void, u32, u64) =
        unsafe { core::mem::transmute(vslot(list, if compute { PC_SET_COMPUTE_ROOT_TABLE } else { PC_SET_GRAPHICS_ROOT_TABLE })) };
    unsafe { set_tbl(list, root_idx, gpu_base) };
    true
}

unsafe extern "system" fn h_set_heaps(this: *mut c_void, n: u32, p: *const usize) {
    let f: unsafe extern "system" fn(*mut c_void, u32, *const usize) = unsafe { core::mem::transmute(PREV_HEAPS.load(Ordering::Relaxed)) };
    unsafe { f(this, n, p) };
    if !p.is_null() {
        let mut cbv = 0usize;
        let mut smp = 0usize;
        let hmap = HEAPS.lock().unwrap_or_else(|e| e.into_inner());
        for i in 0..n as usize {
            let h = unsafe { *p.add(i) };
            if h == 0 {
                continue;
            }
            if hmap.as_ref().is_some_and(|m| m.contains_key(&h)) {
                cbv = h;
            } else if cbv == 0 {
                let get_desc: unsafe extern "system" fn(*mut c_void, *mut [u32; 4]) -> *mut [u32; 4] =
                    unsafe { core::mem::transmute(vslot(h as *mut c_void, 8)) };
                let mut d = [0u32; 4];
                unsafe { get_desc(h as *mut c_void, &mut d) };
                if d[0] == 0 { cbv = h; } else if d[0] == 1 { smp = h; }
            } else {
                smp = h;
            }
        }
        drop(hmap);
        let mut lmap = LISTS.lock().unwrap_or_else(|e| e.into_inner());
        let st = lmap.get_or_insert_with(HashMap::new).entry(this as usize).or_default();
        st.cbv_heap = cbv;
        st.sampler_heap = smp;
    }
}

unsafe extern "system" fn h_croot(this: *mut c_void, rs: *mut c_void) {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void) = unsafe { core::mem::transmute(PREV_CROOT.load(Ordering::Relaxed)) };
    unsafe { f(this, rs) };
    LISTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).entry(this as usize).or_default().croot = rs as usize;
    unsafe { bind_now(this, true, &[]) };
}

unsafe extern "system" fn h_groot(this: *mut c_void, rs: *mut c_void) {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void) = unsafe { core::mem::transmute(PREV_GROOT.load(Ordering::Relaxed)) };
    unsafe { f(this, rs) };
    LISTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).entry(this as usize).or_default().groot = rs as usize;
    unsafe { bind_now(this, false, &[]) };
}

pub fn on_close(list: *mut c_void) {
    if let Some(m) = LISTS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(&(list as usize));
    }
}

pub fn install(vt: &[AtomicUsize]) {
    PREV_HEAPS.store(vt[XBOX_SET_DESCRIPTOR_HEAPS].load(Ordering::Relaxed), Ordering::Relaxed);
    PREV_CROOT.store(vt[XBOX_SET_COMPUTE_ROOT_SIGNATURE].load(Ordering::Relaxed), Ordering::Relaxed);
    PREV_GROOT.store(vt[XBOX_SET_GRAPHICS_ROOT_SIGNATURE].load(Ordering::Relaxed), Ordering::Relaxed);
    vt[XBOX_SET_DESCRIPTOR_HEAPS].store(h_set_heaps as usize, Ordering::Relaxed);
    vt[XBOX_SET_COMPUTE_ROOT_SIGNATURE].store(h_croot as usize, Ordering::Relaxed);
    vt[XBOX_SET_GRAPHICS_ROOT_SIGNATURE].store(h_groot as usize, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_bind_is_opt_in() {
        assert!(!parse_enabled(None));
        assert!(!parse_enabled(Some("")));
        assert!(!parse_enabled(Some("0")));
        assert!(!parse_enabled(Some(" 0 ")));
        assert!(parse_enabled(Some("1")));
        assert!(parse_enabled(Some("on")));
    }

    #[test]
    fn global_bind_allows_shaders_when_enabled() {
        assert_eq!(refusal(false), None);
        assert_eq!(refusal(true), None);
    }

    #[test]
    fn root_signature_append_plan_uses_one_dword_and_respects_limits() {
        let ranges = descriptor_ranges();
        let (p, idx) = append_plan(&[], 0, ranges.as_ptr() as u64).expect("empty RS fits");
        assert_eq!(idx, 0);
        assert_eq!(p.len(), PARAM_SIZE);
        assert_eq!(u32_at(&p, 0), PARAM_TABLE);
        assert_eq!(u32_at(&p, 8), 3);
        assert_eq!(crate::gds::root_cost(&p), 1);

        assert!(append_plan(&[], ROOT_FLAG_LOCAL, ranges.as_ptr() as u64).is_none());

        let mut full = [0u8; PARAM_SIZE];
        put32(&mut full, 0, crate::gds::PARAM_CONSTANTS);
        put32(&mut full, 16, ROOT_DWORD_LIMIT);
        assert!(append_plan(&full, 0, ranges.as_ptr() as u64).is_none());
    }

    #[test]
    fn tb_global_ranges_use_valid_volatile_flags() {
        for r in descriptor_ranges() {
            assert_eq!(u32_at(&r, 16), 0x3);
        }
    }

    #[test]
    fn patch_root_signature_reserializes_through_the_host_serializer() {
        let ser = crate::host_proc(b"D3D12SerializeVersionedRootSignature\0");
        assert!(ser != 0);
        let ser: unsafe extern "system" fn(*const u8, *mut *mut c_void, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(ser) };
        let mut desc = [0u8; 48];
        put32(&mut desc, 0, VERSION_1_1);
        put32(&mut desc, 40, 0x1);
        let (mut b, mut e): (*mut c_void, *mut c_void) = (core::ptr::null_mut(), core::ptr::null_mut());
        assert!(unsafe { ser(desc.as_ptr(), &mut b, &mut e) } >= 0 && !b.is_null());
        let blob = unsafe {
            let ptr: unsafe extern "system" fn(*mut c_void) -> *const u8 = core::mem::transmute(vslot(b, 3));
            let size: unsafe extern "system" fn(*mut c_void) -> usize = core::mem::transmute(vslot(b, 4));
            let v = core::slice::from_raw_parts(ptr(b), size(b)).to_vec();
            let r: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(vslot(b, 2));
            r(b);
            v
        };
        let (patched, idx) = unsafe { patch_root_signature(blob.as_ptr(), blob.len()) }.expect("TbGlobal table appended");
        assert_eq!(idx, 0);
        assert!(patched.len() > blob.len());
    }

    #[test]
    fn heap_growth_only_touches_shader_visible_cbv_srv_uav() {
        let (g, orig) = grow_heap_desc(&[0, 1024, 1, 0]).expect("shader-visible CBV_SRV_UAV grows");
        assert_eq!(orig, 1024);
        assert_eq!(g[1], 1024 + TAIL_SLOTS);
        assert!(grow_heap_desc(&[0, 1024, 0, 0]).is_none()); // non-shader-visible
        assert!(grow_heap_desc(&[1, 1024, 1, 0]).is_none()); // sampler heap
        assert!(grow_heap_desc(&[0, MAX_TIER1_HEAP, 1, 0]).is_none()); // already at Tier 1 cap
    }

    #[test]
    fn window_selection_prioritizes_explicit_then_uav_then_ro() {
        let prio = [GlobalWindow { va: 0x2000, size: 64, res: 10, writable: false }];
        let tracked = [
            (crate::gpu_va::Entry { start: 0x1000, len: 128, res: 1, buffer: true }, false),
            (crate::gpu_va::Entry { start: 0x3000, len: 256, res: 2, buffer: true }, true),
            (crate::gpu_va::Entry { start: 0x2000, len: 64, res: 10, buffer: true }, false), // dup of prio
        ];
        let w = select_windows(&prio, &tracked);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].va, 0x2000);
        assert_eq!(w[1].va, 0x3000);
        assert!(w[1].writable);
        assert_eq!(w[2].va, 0x1000);
        assert!(!w[2].writable);
    }
}
