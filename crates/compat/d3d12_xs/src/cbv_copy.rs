use compat_common::record;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

pub const SLOT_ALIGN: u64 = 256;
pub const MAX_CB: u64 = 65536;
pub const CHUNK_BYTES: u64 = 1 << 20;
pub const ST_COMMON: u32 = 0;
pub const ST_VCB: u32 = 0x1;
pub const ST_COPY_DEST: u32 = 0x400;
pub const ST_COPY_SOURCE: u32 = 0x800;
const ST_WRITE_MASK: u32 = 0x4 | 0x8 | 0x10 | 0x100 | 0x400 | 0x1000;
pub const SIDE_COMPUTE: u8 = 0;
pub const SIDE_GRAPHICS: u8 = 1;
pub const MASK_COMPUTE: u8 = 1;
pub const MASK_GRAPHICS: u8 = 2;
const LIST_TYPE_BUNDLE: u32 = 1;

const X_RESET: usize = 10;
const X_CLEAR_STATE: usize = 11;
const X_DRAW: usize = 12;
const X_DRAW_INDEXED: usize = 13;
const X_DISPATCH: usize = 14;
const X_EXEC_BUNDLE: usize = 27;
const X_SET_COMPUTE_RS: usize = 33;
const X_SET_GRAPHICS_RS: usize = 34;
const X_COMPUTE_ROOT_CBV: usize = 41;
const X_GRAPHICS_ROOT_CBV: usize = 42;
const X_EXECUTE_INDIRECT: usize = 59;
const X_DISPATCH_X: usize = 78;
const X_DRAW_INDEXED_X: usize = 81;
const X_DISPATCH_MESH_X: usize = 98;
const X_DISPATCH_MESH: usize = 127;
const PC_COPY_BUFFER: usize = 15;
const PC_BARRIER: usize = 26;
const PC_SET_COMPUTE_ROOT_CBV: usize = 37;
const PC_SET_GRAPHICS_ROOT_CBV: usize = 38;
const PC_SET_PREDICATION: usize = 55;
const LIST_GET_DEVICE: usize = 7;
const LIST_GET_TYPE: usize = 8;
const DEV_CREATE_COMMITTED: usize = 27;
const RES_GET_GPU_VA: usize = 11;

const DRAW_SLOTS: [usize; 8] = [X_DRAW, X_DRAW_INDEXED, X_DISPATCH, X_EXECUTE_INDIRECT, X_DISPATCH_X, X_DRAW_INDEXED_X, X_DISPATCH_MESH_X, X_DISPATCH_MESH];
const WRITE_SLOTS: [usize; 22] = [15, 16, 17, 26, 27, 53, 54, 55, 60, 61, 62, 63, 69, 70, 71, 72, 74, 83, 84, 88, 89, 90];

#[repr(C)]
struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

const IID_DEVICE: Guid = Guid { d1: 0x1898_19f1, d2: 0x1db6, d3: 0x4b57, d4: [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7] };
const IID_RESOURCE: Guid = Guid { d1: 0x6964_42be, d2: 0xa72e, d3: 0x4059, d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad] };

pub fn round_up(n: u64, a: u64) -> u64 {
    n.div_ceil(a) * a
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizePlan {
    pub alloc: u64,
    pub copy: u64,
    pub zero: u64,
}

pub fn size_plan(declared: Option<u64>, left: u64, limit: Option<u64>) -> Option<SizePlan> {
    if left == 0 {
        return None;
    }
    let alloc = match declared {
        Some(d) => round_up(d.clamp(1, MAX_CB), SLOT_ALIGN),
        None => MAX_CB,
    };
    let copy = alloc.min(left).min(limit.unwrap_or(u64::MAX));
    Some(SizePlan { alloc, copy, zero: alloc - copy })
}

pub fn read_state(_list_type: u32) -> u32 {
    ST_VCB
}

pub fn source_barrier(state: u32) -> Option<(u32, u32)> {
    if state == ST_COMMON || state & ST_COPY_SOURCE != 0 {
        return None;
    }
    let during = if state & ST_WRITE_MASK == 0 { state | ST_COPY_SOURCE } else { ST_COPY_SOURCE };
    Some((state, during))
}

#[derive(Default, Clone)]
struct ListRing {
    chunks: Vec<usize>,
    used: u64,
    state: HashMap<usize, u32>,
    stamps: Vec<(usize, u64)>,
}

#[derive(Default)]
pub struct Ring {
    pub chunks: Vec<(usize, u64)>,
    free: Vec<usize>,
    retired: Vec<(usize, Vec<(usize, u64)>)>,
    lists: HashMap<usize, ListRing>,
}

impl Ring {
    pub fn add_chunk(&mut self, res: usize, va: u64) -> usize {
        self.chunks.push((res, va));
        let id = self.chunks.len() - 1;
        self.free.push(id);
        id
    }

    pub fn reclaim(&mut self, done: &dyn Fn(usize, u64) -> bool) {
        let mut back = Vec::new();
        self.retired.retain(|(c, s)| {
            let ready = s.iter().all(|&(f, v)| done(f, v));
            if ready {
                back.push(*c);
            }
            !ready
        });
        self.free.extend(back);
    }

    pub fn alloc(&mut self, list: usize, bytes: u64, done: &dyn Fn(usize, u64) -> bool) -> Option<(usize, u64)> {
        let bytes = round_up(bytes.max(1), SLOT_ALIGN);
        if bytes > CHUNK_BYTES {
            return None;
        }
        let lr = self.lists.entry(list).or_default();
        if let Some(&c) = lr.chunks.last() {
            if lr.used + bytes <= CHUNK_BYTES {
                let off = lr.used;
                lr.used += bytes;
                return Some((c, off));
            }
        }
        if self.free.is_empty() {
            self.reclaim(done);
        }
        let c = self.free.pop()?;
        let lr = self.lists.entry(list).or_default();
        lr.chunks.push(c);
        lr.used = bytes;
        Some((c, 0))
    }

    pub fn loc(&self, chunk: usize, off: u64) -> (usize, u64) {
        let (r, va) = self.chunks[chunk];
        (r, va + off)
    }

    pub fn state(&self, list: usize, chunk: usize) -> u32 {
        self.lists.get(&list).and_then(|l| l.state.get(&chunk).copied()).unwrap_or(ST_COMMON)
    }

    pub fn set_state(&mut self, list: usize, chunk: usize, st: u32) {
        self.lists.entry(list).or_default().state.insert(chunk, st);
    }

    pub fn holds(&self, list: usize) -> bool {
        self.lists.get(&list).is_some_and(|l| !l.chunks.is_empty())
    }

    pub fn stamp(&mut self, list: usize, fence: usize, value: u64) {
        if let Some(l) = self.lists.get_mut(&list) {
            match l.stamps.iter_mut().find(|s| s.0 == fence) {
                Some(s) => s.1 = s.1.max(value),
                None => l.stamps.push((fence, value)),
            }
            l.state.clear();
        }
    }

    pub fn release(&mut self, list: usize) {
        let Some(l) = self.lists.remove(&list) else { return };
        if l.stamps.is_empty() {
            self.free.extend(l.chunks);
        } else {
            for c in l.chunks {
                self.retired.push((c, l.stamps.clone()));
            }
        }
    }

    #[cfg(test)]
    pub fn free_count(&self) -> usize {
        self.free.len()
    }

    #[cfg(test)]
    pub fn retired_count(&self) -> usize {
        self.retired.len()
    }
}

#[derive(Default)]
struct ListSt {
    pending: BTreeMap<(u8, u32), u64>,
    dedupe: HashMap<(u64, u64), u64>,
    rs: [usize; 2],
    dev: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Forward,
    Hold,
    Provisional(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyOp {
    pub dst: usize,
    pub dst_off: u64,
    pub src: usize,
    pub src_off: u64,
    pub n: u64,
    pub zero: u64,
}

#[derive(Default, Debug, PartialEq)]
pub struct Job {
    pub copies: Vec<CopyOp>,
    pub chunks: Vec<(usize, u32, u32)>,
    pub sources: Vec<(usize, u32, u32)>,
    pub binds: Vec<(u8, u32, u64)>,
    pub unresolved: Vec<(u8, u32, u64)>,
    pub no_room: Vec<(u8, u32, u64)>,
    pub detail: Vec<Detail>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detail {
    pub side: u8,
    pub idx: u32,
    pub va: u64,
    pub reg: Option<(u32, u32)>,
    pub declared: Option<u64>,
    pub limit: Option<u64>,
    pub left: u64,
    pub copy: u64,
    pub zero: u64,
}

pub struct Src {
    pub res: usize,
    pub off: u64,
    pub left: u64,
    pub state: u32,
}

#[derive(Default)]
pub struct Engine {
    lists: HashMap<usize, ListSt>,
    rings: HashMap<usize, Ring>,
    pub rs_params: HashMap<usize, Vec<(usize, u32, u32, u32)>>,
    pub pso_sizes: HashMap<usize, Vec<(u32, u32, u32, u64)>>,
    pub cb_limits: HashMap<usize, Vec<(u32, u32, u32, u64)>>,
    pub pso_stages: HashMap<usize, Vec<(u32, u64)>>,
}

impl Engine {
    pub fn on_root_cbv(&mut self, list: usize, side: u8, idx: u32, va: u64, bundle: bool) -> Action {
        let st = self.lists.entry(list).or_default();
        if va % SLOT_ALIGN == 0 || bundle {
            st.pending.remove(&(side, idx));
            return Action::Forward;
        }
        st.pending.insert((side, idx), va);
        match st.dedupe.iter().filter(|(k, _)| k.0 == va).max_by_key(|(k, _)| k.1) {
            Some((_, &slot)) => Action::Provisional(slot),
            None => Action::Hold,
        }
    }

    pub fn on_root_signature(&mut self, list: usize, side: u8, rs: usize) {
        let st = self.lists.entry(list).or_default();
        if st.rs[side as usize] != rs {
            st.pending.retain(|k, _| k.0 != side);
            st.rs[side as usize] = rs;
        }
    }

    pub fn invalidate(&mut self, list: usize) {
        if let Some(st) = self.lists.get_mut(&list) {
            st.dedupe.clear();
        }
    }

    pub fn clear_state(&mut self, list: usize) {
        if let Some(st) = self.lists.get_mut(&list) {
            st.pending.clear();
            st.dedupe.clear();
            st.rs = [0, 0];
        }
    }

    pub fn forget(&mut self, list: usize) {
        if let Some(st) = self.lists.remove(&list) {
            if let Some(r) = self.rings.get_mut(&st.dev) {
                r.release(list);
            }
        }
        for r in self.rings.values_mut() {
            r.release(list);
        }
    }

    #[cfg(test)]
    pub fn pending(&self, list: usize) -> Vec<((u8, u32), u64)> {
        self.lists.get(&list).map(|s| s.pending.iter().map(|(k, v)| (*k, *v)).collect()).unwrap_or_default()
    }

    pub fn has_pending(&self, list: usize, mask: u8) -> bool {
        self.lists.get(&list).is_some_and(|s| s.pending.keys().any(|k| mask & (1 << k.0) != 0))
    }

    pub fn ring(&mut self, dev: usize) -> &mut Ring {
        self.rings.entry(dev).or_default()
    }

    pub fn holds(&self, list: usize) -> bool {
        self.rings.values().any(|r| r.holds(list))
    }

    pub fn stamp(&mut self, list: usize, fence: usize, value: u64) {
        for r in self.rings.values_mut() {
            r.stamp(list, fence, value);
        }
    }

    pub fn materialize(&mut self, pso: usize, meta: &dyn Fn(u64) -> Option<ShaderCb>) -> bool {
        if self.pso_sizes.contains_key(&pso) {
            return true;
        }
        let Some(stages) = self.pso_stages.get(&pso) else { return false };
        let mut sizes = Vec::new();
        let mut limits = Vec::new();
        for &(vis, h) in stages {
            let Some(m) = meta(h) else { return false };
            sizes.extend(m.sizes.iter().map(|&(r, sp, b)| (vis, r, sp, b)));
            limits.extend(m.limits.iter().map(|&(r, sp, b)| (vis, r, sp, b)));
        }
        self.pso_sizes.insert(pso, sizes);
        if !limits.is_empty() {
            self.cb_limits.insert(pso, limits);
        }
        true
    }

    pub fn declared(&self, pso: Option<usize>, rs: usize, idx: u32) -> Option<u64> {
        Self::per_stage(&self.pso_sizes, &self.rs_params, pso, rs, idx).into_iter().max()
    }

    pub fn limit(&self, pso: Option<usize>, rs: usize, idx: u32) -> Option<u64> {
        let v = Self::per_stage(&self.cb_limits, &self.rs_params, pso, rs, idx);
        let stages = Self::per_stage(&self.pso_sizes, &self.rs_params, pso, rs, idx).len();
        (!v.is_empty() && v.len() >= stages).then(|| v.into_iter().max().unwrap_or(0))
    }

    pub fn reg_of(&self, rs: usize, idx: u32) -> Option<(u32, u32)> {
        self.rs_params.get(&rs)?.iter().find(|p| p.0 == idx as usize).map(|p| (p.1, p.2))
    }

    fn per_stage(m: &HashMap<usize, Vec<(u32, u32, u32, u64)>>, rsp: &HashMap<usize, Vec<(usize, u32, u32, u32)>>, pso: Option<usize>, rs: usize, idx: u32) -> Vec<u64> {
        let Some(&(_, reg, space, vis)) = rsp.get(&rs).and_then(|v| v.iter().find(|p| p.0 == idx as usize)) else { return Vec::new() };
        let Some(l) = pso.and_then(|k| m.get(&k)) else { return Vec::new() };
        l.iter().filter(|e| e.1 == reg && e.2 == space && (vis == 0 || e.0 == 0 || e.0 == vis)).map(|e| e.3).collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn plan(
        &mut self,
        list: usize,
        dev: usize,
        list_type: u32,
        mask: u8,
        pso: Option<usize>,
        lookup: &dyn Fn(u64) -> Option<Src>,
        mk_chunk: &mut dyn FnMut() -> Option<(usize, u64)>,
        done: &dyn Fn(usize, u64) -> bool,
    ) -> Job {
        let mut job = Job::default();
        let st = self.lists.entry(list).or_default();
        st.dev = dev;
        let todo: Vec<((u8, u32), u64)> = st.pending.iter().filter(|(k, _)| mask & (1 << k.0) != 0).map(|(k, v)| (*k, *v)).collect();
        if todo.is_empty() {
            return job;
        }
        st.pending.retain(|k, _| mask & (1 << k.0) == 0);
        let rs = st.rs;
        if let Some(p) = pso {
            if !self.materialize(p, &shader_cb) && self.pso_stages.contains_key(&p) {
                note(&SIZE_UNKNOWN, 4, &|| format!("pso {p:#x}: a stage's constant buffer sizes were not recorded by its translation; its unaligned root CBVs use a 64 KiB slot zero-filled past the source"));
            }
        }
        let sizes: Vec<(Option<u64>, Option<u64>, Option<(u32, u32)>)> = todo.iter().map(|((side, idx), _)| {
            let r = rs[*side as usize];
            (self.declared(pso, r, *idx), self.limit(pso, r, *idx), self.reg_of(r, *idx))
        }).collect();
        let rstate = read_state(list_type);
        for (((side, idx), va), (declared, limit, reg)) in todo.into_iter().zip(sizes) {
            let Some(src) = lookup(va) else {
                job.unresolved.push((side, idx, va));
                continue;
            };
            let Some(p) = size_plan(declared, src.left, limit) else {
                job.unresolved.push((side, idx, va));
                continue;
            };
            if let Some(&slot) = self.lists.get(&list).and_then(|s| s.dedupe.get(&(va, p.alloc))) {
                job.binds.push((side, idx, slot));
                continue;
            }
            let ring = self.rings.entry(dev).or_default();
            let mut got = ring.alloc(list, p.alloc, done);
            if got.is_none() {
                if let Some((res, cva)) = mk_chunk() {
                    ring.add_chunk(res, cva);
                    got = ring.alloc(list, p.alloc, done);
                }
            }
            let Some((chunk, off)) = got else {
                job.no_room.push((side, idx, va));
                continue;
            };
            let (cres, slot_va) = ring.loc(chunk, off);
            if !job.chunks.iter().any(|c| c.0 == cres) {
                job.chunks.push((cres, ring.state(list, chunk), rstate));
                ring.set_state(list, chunk, rstate);
            }
            if let Some((before, during)) = source_barrier(src.state) {
                if !job.sources.iter().any(|s| s.0 == src.res) {
                    job.sources.push((src.res, before, during));
                }
            }
            job.copies.push(CopyOp { dst: cres, dst_off: off, src: src.res, src_off: src.off, n: p.copy, zero: p.zero });
            job.detail.push(Detail { side, idx, va, reg, declared, limit, left: src.left, copy: p.copy, zero: p.zero });
            if let Some(s) = self.lists.get_mut(&list) {
                s.dedupe.insert((va, p.alloc), slot_va);
            }
            job.binds.push((side, idx, slot_va));
        }
        job
    }
}

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    f(ENGINE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Engine::default))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShaderCb {
    pub sizes: Vec<(u32, u32, u64)>,
    pub limits: Vec<(u32, u32, u64)>,
}

pub fn slot_limits(m: &std::collections::BTreeMap<u32, u64>) -> Vec<(u32, u32, u64)> {
    m.iter().filter(|(slot, _)| **slot & 0x8000_0000 == 0).map(|(slot, b)| (slot & 0xffff, slot >> 16, *b)).collect()
}

static SHADER_CB: Mutex<Option<HashMap<u64, ShaderCb>>> = Mutex::new(None);

pub fn note_shader_cb(hash: u64, meta: ShaderCb) {
    SHADER_CB.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(hash, meta);
}

pub fn shader_cb(hash: u64) -> Option<ShaderCb> {
    SHADER_CB.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&hash).cloned()
}

pub fn meta_of(t: &tombox_xshader::hlsl::Translated) -> ShaderCb {
    ShaderCb { sizes: tombox_xshader::cbv_raw_hlsl::declared_sizes(&t.hlsl), limits: slot_limits(&t.cb_limits) }
}

pub fn encode_meta(m: &ShaderCb) -> Vec<u8> {
    let mut v = b"TBCBM1".to_vec();
    for part in [&m.sizes, &m.limits] {
        v.extend_from_slice(&(part.len() as u32).to_le_bytes());
        for &(r, sp, b) in part.iter() {
            v.extend_from_slice(&r.to_le_bytes());
            v.extend_from_slice(&sp.to_le_bytes());
            v.extend_from_slice(&b.to_le_bytes());
        }
    }
    v
}

pub fn decode_meta(b: &[u8]) -> Option<ShaderCb> {
    let mut at = 6;
    if b.get(..6)? != b"TBCBM1" {
        return None;
    }
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = b.get(at..at + n)?;
        at += n;
        Some(s)
    };
    let mut parts = Vec::new();
    for _ in 0..2 {
        let n = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
        if n > 4096 {
            return None;
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let r = u32::from_le_bytes(take(4)?.try_into().ok()?);
            let sp = u32::from_le_bytes(take(4)?.try_into().ok()?);
            let bytes = u64::from_le_bytes(take(8)?.try_into().ok()?);
            v.push((r, sp, bytes));
        }
        parts.push(v);
    }
    let limits = parts.pop()?;
    let sizes = parts.pop()?;
    Some(ShaderCb { sizes, limits })
}

unsafe fn stage_hashes(desc: *const u8, stages: &[(u32, usize)]) -> Vec<(u32, u64)> {
    let mut out = Vec::new();
    for &(vis, off) in stages {
        let p = unsafe { core::ptr::read_unaligned(desc.add(off) as *const u64) } as *const u8;
        let n = unsafe { core::ptr::read_unaligned(desc.add(off + 8) as *const u64) } as usize;
        if p.is_null() || !(32..=0x40_0000).contains(&n) {
            continue;
        }
        out.push((vis, crate::shader_xlat::fnv1a(unsafe { core::slice::from_raw_parts(p, n) })));
    }
    out
}

pub fn store_pso(pso: usize, stages: Vec<(u32, u64)>) {
    with(|e| {
        e.pso_sizes.remove(&pso);
        e.cb_limits.remove(&pso);
        e.pso_stages.insert(pso, stages);
    });
}

const GFX_STAGES: [(u32, usize); 5] = [(1, 0x08), (5, 0x18), (3, 0x28), (2, 0x38), (4, 0x48)];

pub fn after_gfx_pso(hr: i32, pdesc: *const u8, ppv: *mut *mut c_void) -> i32 {
    if crate::cbv_raw::copy_on() {
        unsafe { record_pso(hr, pdesc, ppv, &GFX_STAGES) };
    }
    hr
}

unsafe fn record_pso(hr: i32, pdesc: *const u8, ppv: *mut *mut c_void, stages: &[(u32, usize)]) {
    if hr >= 0 && !pdesc.is_null() && !ppv.is_null() {
        let pso = unsafe { *ppv } as usize;
        if pso != 0 {
            store_pso(pso, unsafe { stage_hashes(pdesc, stages) });
        }
    }
}

pub fn alias_pso(from: usize, to: usize) {
    if from == 0 || to == 0 || from == to {
        return;
    }
    with(|e| {
        if let Some(v) = e.pso_sizes.get(&from).cloned() {
            e.pso_sizes.entry(to).or_insert(v);
        }
        if let Some(v) = e.cb_limits.get(&from).cloned() {
            e.cb_limits.entry(to).or_insert(v);
        }
        if let Some(v) = e.pso_stages.get(&from).cloned() {
            e.pso_stages.entry(to).or_insert(v);
        }
    });
}

pub fn after_compute_pso(hr: i32, pdesc: *const u8, ppv: *mut *mut c_void) -> i32 {
    if crate::cbv_raw::copy_on() {
        unsafe { record_pso(hr, pdesc, ppv, &[(0, 0x08)]) };
    }
    hr
}

fn rs_params_of(rs: usize) -> Option<Vec<(usize, u32, u32, u32)>> {
    let (_, blob, _) = crate::pso_lib::root_info(rs)?;
    Some(crate::cbv_raw::root_cbv_params(&blob))
}

static SIZE_UNKNOWN: AtomicUsize = AtomicUsize::new(0);
static UNRESOLVED: AtomicUsize = AtomicUsize::new(0);
static NO_ROOM: AtomicUsize = AtomicUsize::new(0);
static BUNDLE_REFUSED: AtomicUsize = AtomicUsize::new(0);
static COPIES: AtomicUsize = AtomicUsize::new(0);
static REUSED: AtomicUsize = AtomicUsize::new(0);
static NO_FENCE: AtomicUsize = AtomicUsize::new(0);
static ILLEGAL: AtomicUsize = AtomicUsize::new(0);

fn note(c: &AtomicUsize, first: usize, text: &dyn Fn() -> String) {
    let n = c.fetch_add(1, Ordering::Relaxed);
    if n < first || (n + 1).is_power_of_two() {
        record("d3d12_xs", "cbv_copy", &format!("#{} {}", n + 1, text()));
    }
}

pub use tombox_xshader::cbv_raw_hlsl::{in_table, table_cbv_ranges};

unsafe fn vslot(o: *mut c_void, i: usize) -> usize {
    unsafe { *(*(o as *const *const usize)).add(i) }
}

pub unsafe fn device_of(list: *mut c_void) -> usize {
    let get: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(list, LIST_GET_DEVICE)) };
    let mut d: *mut c_void = core::ptr::null_mut();
    if unsafe { get(list, &IID_DEVICE, &mut d) } < 0 || d.is_null() {
        return 0;
    }
    let rel: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(vslot(d, 2)) };
    unsafe { rel(d) };
    d as usize
}

pub unsafe fn list_type(list: *mut c_void) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(vslot(list, LIST_GET_TYPE)) };
    unsafe { f(list) }
}

pub unsafe fn create_buffer(dev: usize, size: u64) -> Option<(usize, u64)> {
    if dev == 0 {
        return None;
    }
    let heap: [u32; 5] = [1, 0, 0, 1, 1];
    let mut rd = [0u8; 56];
    rd[0..4].copy_from_slice(&1u32.to_le_bytes());
    rd[0x10..0x18].copy_from_slice(&size.to_le_bytes());
    rd[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
    rd[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
    rd[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
    rd[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
    rd[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
    let d = dev as *mut c_void;
    let cc: unsafe extern "system" fn(*mut c_void, *const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(d, DEV_CREATE_COMMITTED)) };
    let mut buf: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { cc(d, heap.as_ptr(), 0, rd.as_ptr(), ST_COMMON, core::ptr::null(), &IID_RESOURCE, &mut buf) };
    if hr < 0 || buf.is_null() {
        record("d3d12_xs", "cbv_copy", &format!("DEFAULT-heap buffer of {size:#x} bytes FAILED hr={hr:#010x}"));
        return None;
    }
    let va: unsafe extern "system" fn(*mut c_void) -> u64 = unsafe { core::mem::transmute(vslot(buf, RES_GET_GPU_VA)) };
    Some((buf as usize, unsafe { va(buf) }))
}

static ZERO: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);

unsafe fn zero_block(dev: usize) -> Option<usize> {
    let mut g = ZERO.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if let Some(&z) = m.get(&dev) {
        return Some(z);
    }
    let (z, _) = unsafe { create_buffer(dev, MAX_CB) }?;
    m.insert(dev, z);
    Some(z)
}

pub fn lookup_source(va: u64) -> Option<Src> {
    if let Some((e, off)) = crate::gpu_va::lookup_buffer(va) {
        if off < e.len {
            return Some(Src { res: e.res, off, left: e.len - off, state: crate::gpu_va::state(e.res) });
        }
    }
    let (base, res, size, _) = crate::bindtrace::buffer_at(va)?;
    let off = va - base;
    (off < size).then(|| Src { res, off, left: size - off, state: crate::gpu_va::state(res) })
}

fn trace_targets() -> &'static Vec<u64> {
    static T: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    T.get_or_init(|| crate::hang_cap::parse_targets(std::env::var("TOMBOX_CBRAW_CAP_PS").ok().as_deref()))
}

fn ps_hash(pso: usize) -> u64 {
    if let Some(h) = crate::dred_marks::hashes(pso) {
        return h.ps;
    }
    crate::device_wrap::pso_summary(pso)
        .map(|(_, p, _)| if p == 0 { 0 } else { crate::shader_xlat::origin_hash(p).unwrap_or(0) })
        .unwrap_or(0)
}

pub fn detail_text(d: &Detail) -> String {
    let reg = d.reg.map(|(r, s)| format!("b{r} space{s}")).unwrap_or_else(|| "b?".into());
    let lim = d.limit.map(|l| format!("{l:#x}")).unwrap_or_else(|| "unknown (the translator does not export it yet)".into());
    let decl = d.declared.map(|l| format!("{l:#x}")).unwrap_or_else(|| "unknown".into());
    let past = if d.declared.is_some_and(|m| m > d.left) { " SHADER MAX READ PASSES THE SOURCE END" } else { "" };
    format!("param {} side {} {reg}: va={:#x} shader max read (declared cbuffer bytes)={decl} NR*stride={lim} source bytes left={:#x} copied={:#x} zero tail={:#x}{past}", d.idx, d.side, d.va, d.left, d.copy, d.zero)
}

static TRACED: AtomicUsize = AtomicUsize::new(0);

fn trace_cap() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| std::env::var("TOMBOX_CBRAW_CAP_N").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(64))
}

unsafe fn trace_target(list: *mut c_void, pso: Option<usize>, job: &Job) {
    if TRACED.load(Ordering::Relaxed) >= trace_cap() {
        return;
    }
    let Some(pso) = pso else { return };
    let ps = ps_hash(pso);
    if ps == 0 || !trace_targets().contains(&ps) {
        return;
    }
    for d in &job.detail {
        if TRACED.fetch_add(1, Ordering::Relaxed) >= trace_cap() {
            return;
        }
        record("d3d12_xs", "cbv_copy", &format!("target PS {ps:016x} draw on list {list:p}: {}", detail_text(d)));
    }
}

fn describe_miss(va: u64) -> String {
    let near: Vec<String> = crate::gpu_va::near(va, 2).iter().map(|e| format!("{}{:#x}..{:#x} res {:#x}", if e.buffer { "buffer " } else { "texture " }, e.start, e.start.saturating_add(e.len), e.res)).collect();
    let hit = match crate::gpu_va::lookup(va) {
        Some((e, off)) => format!("the address map has only a {} entry here (res {:#x} +{off:#x} of {:#x})", if e.buffer { "buffer past-the-end" } else { "texture" }, e.res, e.len),
        None => "no address-map entry".to_string(),
    };
    format!("{hit}; neighbours [{}]", near.join(", "))
}

fn transition(res: usize, before: u32, after: u32) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[8..16].copy_from_slice(&(res as u64).to_le_bytes());
    b[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    b[20..24].copy_from_slice(&before.to_le_bytes());
    b[24..28].copy_from_slice(&after.to_le_bytes());
    b
}

unsafe fn barriers(list: *mut c_void, b: &[[u8; 32]]) {
    if b.is_empty() {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, u32, *const [u8; 32]) = unsafe { core::mem::transmute(crate::cmdlist_wrap::pc_method(list, PC_BARRIER)) };
    unsafe { f(list, b.len() as u32, b.as_ptr()) };
}

pub unsafe fn bind(list: *mut c_void, side: u8, idx: u32, va: u64) {
    let slot = if side == SIDE_COMPUTE { PC_SET_COMPUTE_ROOT_CBV } else { PC_SET_GRAPHICS_ROOT_CBV };
    let f: unsafe extern "system" fn(*mut c_void, u32, u64) = unsafe { core::mem::transmute(crate::cmdlist_wrap::pc_method(list, slot)) };
    unsafe { f(list, idx, va) };
}

pub unsafe fn run(list: *mut c_void, job: &Job, zero: Option<usize>, ty: u32, pred: Option<(usize, u64, u32)>) {
    let _ij = crate::inject::mark(crate::inject::COPY);
    if !job.copies.is_empty() {
        let setp: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, u32) = unsafe { core::mem::transmute(crate::cmdlist_wrap::pc_method(list, PC_SET_PREDICATION)) };
        if pred.is_some() {
            unsafe { setp(list, core::ptr::null_mut(), 0, 0) };
        }
        let legal = |s: u32| crate::fill::state_legal_on(ty, s);
        let srcs: Vec<&(usize, u32, u32)> = job.sources.iter().filter(|s| {
            let ok = legal(s.1) && legal(s.2);
            if !ok {
                note(&ILLEGAL, 4, &|| format!("copy source {:#x} is in state {:#x}, which a type-{ty} list cannot transition; copied without a barrier", s.0, s.1));
            }
            ok
        }).collect();
        let mut pre: Vec<[u8; 32]> = job.chunks.iter().filter(|c| c.1 != ST_COPY_DEST).map(|c| transition(c.0, c.1, ST_COPY_DEST)).collect();
        pre.extend(srcs.iter().map(|s| transition(s.0, s.1, s.2)));
        unsafe { barriers(list, &pre) };
        let cp: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *mut c_void, u64, u64) = unsafe { core::mem::transmute(crate::cmdlist_wrap::pc_method(list, PC_COPY_BUFFER)) };
        for c in &job.copies {
            if c.n != 0 {
                unsafe { cp(list, c.dst as *mut c_void, c.dst_off, c.src as *mut c_void, c.src_off, c.n) };
            }
            if c.zero != 0 {
                match zero {
                    Some(z) => unsafe { cp(list, c.dst as *mut c_void, c.dst_off + c.n, z as *mut c_void, 0, c.zero) },
                    None => note(&UNRESOLVED, 8, &|| format!("no zero block for a {}-byte tail; the tail keeps old ring contents", c.zero)),
                }
            }
        }
        let mut post: Vec<[u8; 32]> = job.chunks.iter().map(|c| transition(c.0, ST_COPY_DEST, c.2)).collect();
        post.extend(srcs.iter().map(|s| transition(s.0, s.2, s.1)));
        unsafe { barriers(list, &post) };
        if let Some((r, o, op)) = pred {
            unsafe { setp(list, r as *mut c_void, o, op) };
        }
    }
    for &(side, idx, va) in &job.binds {
        unsafe { bind(list, side, idx, va) };
    }
}

pub unsafe fn resolve_with(list: *mut c_void, mask: u8, pso: Option<usize>, pred: Option<(usize, u64, u32)>) {
    if !with(|e| e.has_pending(list as usize, mask)) {
        return;
    }
    if crate::dred::removed() {
        with(|e| {
            if let Some(s) = e.lists.get_mut(&(list as usize)) {
                s.pending.retain(|k, _| mask & (1 << k.0) == 0);
            }
        });
        return;
    }
    let dev = unsafe { device_of(list) };
    let ty = unsafe { list_type(list) };
    let rs_missing: Vec<usize> = with(|e| {
        let rs = e.lists.get(&(list as usize)).map(|s| s.rs).unwrap_or_default();
        rs.into_iter().filter(|r| *r != 0 && !e.rs_params.contains_key(r)).collect()
    });
    for rs in rs_missing {
        if let Some(p) = rs_params_of(rs) {
            with(|e| e.rs_params.insert(rs, p));
        }
    }
    let job = with(|e| {
        e.plan(list as usize, dev, ty, mask, pso, &lookup_source, &mut || unsafe { create_buffer(dev, CHUNK_BYTES) }, &|f, v| unsafe { crate::cl2_state::fence_passed(f, v) })
    });
    let zero = if job.copies.iter().any(|c| c.zero != 0) { unsafe { zero_block(dev) } } else { None };
    if !job.copies.is_empty() {
        let n = COPIES.fetch_add(job.copies.len(), Ordering::Relaxed);
        if n == 0 {
            let c = job.copies[0];
            record("d3d12_xs", "cbv_copy", &format!("first unaligned root CBV copy: {} bytes from res {:#x}+{:#x} into ring {:#x}+{:#x} (zero tail {}), list type {ty}, pso {pso:x?}", c.n, c.src, c.src_off, c.dst, c.dst_off, c.zero));
        }
    }
    if job.copies.len() < job.binds.len() {
        REUSED.fetch_add(job.binds.len() - job.copies.len(), Ordering::Relaxed);
    }
    if !job.detail.is_empty() {
        unsafe { trace_target(list, pso, &job) };
    }
    for &(side, idx, va) in &job.unresolved {
        note(&UNRESOLVED, 8, &|| format!("root CBV param {idx} side {side} va={va:#x} is not inside any known buffer ({}); bound as given, as cbv_raw=off does", describe_miss(va)));
    }
    for &(side, idx, va) in &job.no_room {
        note(&NO_ROOM, 4, &|| format!("root CBV param {idx} side {side} va={va:#x}: the copy ring could not grow (DEFAULT-heap chunk creation failed); bound as given, as cbv_raw=off does"));
    }
    let timed = !job.copies.is_empty();
    if timed {
        unsafe { crate::gpuprof::pass_begin(list, crate::gpuprof::TAG_CBV_COPY) };
    }
    unsafe { run(list, &job, zero, ty, pred) };
    if timed {
        unsafe { crate::gpuprof::pass_end(list) };
    }
    for &(side, idx, va) in job.unresolved.iter().chain(job.no_room.iter()) {
        unsafe { bind(list, side, idx, va) };
    }
}

pub unsafe fn resolve(list: *mut c_void, mask: u8) {
    let pso = crate::cl2_state::bound_pso(list);
    let pred = crate::cl2_state::predication_of(list);
    unsafe { resolve_with(list, mask, pso, pred) };
}

pub unsafe fn set_root_cbv(list: *mut c_void, side: u8, idx: u32, va: u64, fwd: &dyn Fn(u64)) {
    let bundle = va % SLOT_ALIGN != 0 && unsafe { list_type(list) } == LIST_TYPE_BUNDLE;
    if bundle {
        note(&BUNDLE_REFUSED, 8, &|| format!("bundle {list:p} sets root CBV param {idx} to unaligned {va:#x}: a bundle cannot record the copy or its barriers, so it is bound as given, as cbv_raw=off does (unsupported)"));
    }
    match with(|e| e.on_root_cbv(list as usize, side, idx, va, bundle)) {
        Action::Forward => fwd(va),
        Action::Hold => {}
        Action::Provisional(slot) => {
            if !crate::dred::removed() {
                unsafe { bind(list, side, idx, slot) };
            }
        }
    }
}

pub unsafe fn note_executed(queue: *mut c_void, lists: &[*mut c_void]) {
    if !crate::cbv_raw::copy_on() {
        return;
    }
    unsafe { stamp_executed(queue, lists) };
}

pub unsafe fn stamp_executed(queue: *mut c_void, lists: &[*mut c_void]) {
    let held: Vec<usize> = with(|e| lists.iter().map(|l| *l as usize).filter(|l| e.holds(*l)).collect());
    let Some(&first) = held.first() else { return };
    match unsafe { crate::cl2_state::signal_queue(queue, first as *mut c_void) } {
        Some((f, v)) => with(|e| held.iter().for_each(|&l| e.stamp(l, f, v))),
        None => {
            note(&NO_FENCE, 4, &|| "no queue fence for lists holding ring slots; their chunks are never reused".to_string());
            with(|e| held.iter().for_each(|&l| e.stamp(l, usize::MAX, u64::MAX)));
        }
    }
}

static PREV: [AtomicUsize; 128] = [const { AtomicUsize::new(0) }; 128];

fn prev(slot: usize) -> usize {
    PREV[slot].load(Ordering::Relaxed)
}

unsafe extern "system" fn h_compute_root_cbv(this: *mut c_void, i: u32, va: u64) {
    let f: unsafe extern "system" fn(*mut c_void, u32, u64) = unsafe { core::mem::transmute(prev(X_COMPUTE_ROOT_CBV)) };
    unsafe { set_root_cbv(this, SIDE_COMPUTE, i, va, &|v| f(this, i, v)) }
}

unsafe extern "system" fn h_graphics_root_cbv(this: *mut c_void, i: u32, va: u64) {
    let f: unsafe extern "system" fn(*mut c_void, u32, u64) = unsafe { core::mem::transmute(prev(X_GRAPHICS_ROOT_CBV)) };
    unsafe { set_root_cbv(this, SIDE_GRAPHICS, i, va, &|v| f(this, i, v)) }
}

unsafe extern "system" fn h_compute_rs(this: *mut c_void, rs: *mut c_void) {
    with(|e| e.on_root_signature(this as usize, SIDE_COMPUTE, rs as usize));
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void) = unsafe { core::mem::transmute(prev(X_SET_COMPUTE_RS)) };
    unsafe { f(this, rs) }
}

unsafe extern "system" fn h_graphics_rs(this: *mut c_void, rs: *mut c_void) {
    with(|e| e.on_root_signature(this as usize, SIDE_GRAPHICS, rs as usize));
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void) = unsafe { core::mem::transmute(prev(X_SET_GRAPHICS_RS)) };
    unsafe { f(this, rs) }
}

static LATE_PREV: [AtomicUsize; 128] = [const { AtomicUsize::new(0) }; 128];

unsafe extern "system" fn h_reset(this: *mut c_void, alloc: *mut c_void, pso: *mut c_void) -> i32 {
    let chain = crate::list_split::expand(&[this]);
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32 = unsafe { core::mem::transmute(LATE_PREV[X_RESET].load(Ordering::Relaxed)) };
    let hr = unsafe { f(this, alloc, pso) };
    if hr >= 0 {
        with(|e| {
            e.forget(this as usize);
            for l in &chain {
                e.forget(*l as usize);
            }
        });
    }
    hr
}

pub fn slot_mask(slot: usize) -> u8 {
    match slot {
        X_DRAW | X_DRAW_INDEXED | X_DRAW_INDEXED_X | X_DISPATCH_MESH_X | X_DISPATCH_MESH => MASK_GRAPHICS,
        X_DISPATCH | X_DISPATCH_X => MASK_COMPUTE,
        X_EXECUTE_INDIRECT | X_EXEC_BUNDLE => MASK_COMPUTE | MASK_GRAPHICS,
        _ => 0,
    }
}

pub fn slot_writes(slot: usize) -> bool {
    WRITE_SLOTS.contains(&slot) || matches!(slot, X_DISPATCH | X_DISPATCH_X | X_EXECUTE_INDIRECT)
}

extern "system" fn hook_pre(list: *mut c_void, slot: usize) {
    if slot == X_CLEAR_STATE {
        with(|e| e.clear_state(list as usize));
        return;
    }
    let mask = slot_mask(slot);
    if mask != 0 {
        unsafe { resolve(list, mask) };
    }
    if slot_writes(slot) {
        with(|e| e.invalidate(list as usize));
    }
}

macro_rules! late_thunks {
    ($(($name:ident, $slot:literal)),* $(,)?) => {
        $(
            #[unsafe(naked)]
            unsafe extern "system" fn $name() {
                core::arch::naked_asm!(
                    "push rcx",
                    "push rdx",
                    "push r8",
                    "push r9",
                    "sub rsp, 104",
                    "movdqu [rsp + 32], xmm0",
                    "movdqu [rsp + 48], xmm1",
                    "movdqu [rsp + 64], xmm2",
                    "movdqu [rsp + 80], xmm3",
                    "mov edx, {s}",
                    "call {pre}",
                    "movdqu xmm0, [rsp + 32]",
                    "movdqu xmm1, [rsp + 48]",
                    "movdqu xmm2, [rsp + 64]",
                    "movdqu xmm3, [rsp + 80]",
                    "add rsp, 104",
                    "pop r9",
                    "pop r8",
                    "pop rdx",
                    "pop rcx",
                    "lea rax, [rip + {tbl}]",
                    "mov rax, [rax + {off}]",
                    "jmp rax",
                    s = const $slot,
                    off = const $slot * 8,
                    pre = sym hook_pre,
                    tbl = sym LATE_PREV,
                );
            }
        )*
        fn late_thunk(slot: usize) -> usize {
            match slot {
                $( $slot => $name as usize, )*
                _ => 0,
            }
        }
    };
}

late_thunks!(
    (t11, 11), (t12, 12), (t13, 13), (t14, 14), (t15, 15), (t16, 16), (t17, 17), (t26, 26), (t27, 27),
    (t53, 53), (t54, 54), (t55, 55), (t59, 59), (t60, 60), (t61, 61), (t62, 62), (t63, 63), (t69, 69),
    (t70, 70), (t71, 71), (t72, 72), (t74, 74), (t78, 78), (t81, 81), (t83, 83), (t84, 84), (t88, 88),
    (t89, 89), (t90, 90), (t98, 98), (t127, 127),
);

pub fn install_inner(vt: &[AtomicUsize]) -> usize {
    let mut n = 0;
    for (slot, f) in [
        (X_COMPUTE_ROOT_CBV, h_compute_root_cbv as usize),
        (X_GRAPHICS_ROOT_CBV, h_graphics_root_cbv as usize),
        (X_SET_COMPUTE_RS, h_compute_rs as usize),
        (X_SET_GRAPHICS_RS, h_graphics_rs as usize),
    ] {
        let p = vt[slot].load(Ordering::Relaxed);
        if p == 0 || p == f {
            continue;
        }
        PREV[slot].store(p, Ordering::Relaxed);
        vt[slot].store(f, Ordering::Relaxed);
        n += 1;
    }
    n
}

pub fn install_late(vt: &[AtomicUsize]) -> usize {
    let mut n = 0;
    let mut slots: Vec<usize> = DRAW_SLOTS.iter().chain(WRITE_SLOTS.iter()).copied().collect();
    slots.push(X_CLEAR_STATE);
    slots.sort_unstable();
    slots.dedup();
    for slot in slots {
        let f = late_thunk(slot);
        if f == 0 || slot >= vt.len() {
            continue;
        }
        let p = vt[slot].load(Ordering::Relaxed);
        if p == 0 || p == f {
            continue;
        }
        LATE_PREV[slot].store(p, Ordering::Relaxed);
        vt[slot].store(f, Ordering::Relaxed);
        n += 1;
    }
    let p = vt[X_RESET].load(Ordering::Relaxed);
    if p != 0 && p != h_reset as usize {
        LATE_PREV[X_RESET].store(p, Ordering::Relaxed);
        vt[X_RESET].store(h_reset as usize, Ordering::Relaxed);
        n += 1;
    }
    n
}

pub fn install(vt: &[AtomicUsize], late: bool) {
    if !crate::cbv_raw::copy_on() || vt.len() < 128 {
        return;
    }
    let n = if late { install_late(vt) } else { install_inner(vt) };
    record("d3d12_xs", "cbv_copy", &format!("cbv_raw=copy: {} hooks on {n} command-list slots", if late { "draw/dispatch resolve, title-write dedupe invalidation and Reset" } else { "root CBV and root signature" }));
}

#[cfg(test)]
#[path = "cbv_copy_hw.rs"]
mod hw;

#[cfg(test)]
mod tests {
    use super::*;

    fn src(res: usize, base: u64, len: u64) -> impl Fn(u64) -> Option<Src> {
        move |va| (va >= base && va < base + len).then(|| Src { res, off: va - base, left: base + len - va, state: 0xac3 })
    }

    #[test]
    fn copy_mode_pso_create_reuses_the_translation_metadata_and_never_translates_again() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../input/shader_cache/titles/GbxGame/src/f861c56a34758d84_0200000000000000.xbox");
        let Ok(blob) = std::fs::read(&path) else {
            println!("Trackmania PS f861c56a34758d84 not on disk ({}): check not run", path.display());
            return;
        };
        let hash = crate::shader_xlat::fnv1a(&blob);
        let mode = tombox_xshader::pack_key::Mode { cbv_raw: tombox_xshader::pack_key::CbvRaw::Copy, loop_grad: false, fma: Default::default(), wave64: false, wave64_path: true };
        let count = || crate::shader_xlat::RUNTIME_SOURCES.with(|c| c.get());
        let n0 = count();
        let _ = crate::shader_xlat::runtime_source(&blob, hash, &[2, 0, 0, 0, 0, 0, 0, 0], &tombox_xshader::hlsl::InputInterp::default(), None, &mode);
        assert_eq!(count(), n0 + 1);
        let meta = shader_cb(hash).expect("the copy-mode translation records the shader's constant buffer sizes");
        let &(reg, space, bytes) = meta.sizes.first().expect("f861 declares constant buffers");
        let mut desc = vec![0u8; 0x58];
        desc[0x18..0x20].copy_from_slice(&(blob.as_ptr() as u64).to_le_bytes());
        desc[0x20..0x28].copy_from_slice(&(blob.len() as u64).to_le_bytes());
        let (pso, rs) = (0x7f00_c0de_0001usize, 0x7f00_c0de_0002usize);
        let mut out = pso as *mut c_void;
        let n1 = count();
        unsafe { record_pso(0, desc.as_ptr(), &mut out, &GFX_STAGES) };
        let got = with(|e| {
            e.rs_params.insert(rs, vec![(5, reg, space, 0)]);
            let ok = e.materialize(pso, &shader_cb);
            let r = (ok, e.pso_stages.get(&pso).cloned(), e.declared(Some(pso), rs, 5));
            e.rs_params.remove(&rs);
            e.pso_stages.remove(&pso);
            e.pso_sizes.remove(&pso);
            e.cb_limits.remove(&pso);
            r
        });
        assert_eq!(count(), n1, "a PSO create in copy mode must not run the translator again");
        assert_eq!(got.0, true);
        assert_eq!(got.1, Some(vec![(5, hash)]));
        assert_eq!(got.2, Some(bytes));
    }

    #[test]
    fn pso_sizes_come_from_the_recorded_translation_metadata() {
        let m = ShaderCb { sizes: vec![(13, 0, 0x140), (2, 1, 0x40)], limits: vec![(13, 0, 0x44)] };
        assert_eq!(decode_meta(&encode_meta(&m)), Some(m.clone()));
        assert_eq!(decode_meta(b"TBCBM1\x01"), None);
        assert_eq!(decode_meta(b"junk"), None);
        let mut e = Engine::default();
        e.rs_params.insert(0x77, vec![(3, 13, 0, 0)]);
        e.pso_stages.insert(0x99, vec![(1, 0xaaa), (5, 0xbbb)]);
        let only_ps = |h: u64| (h == 0xbbb).then(|| m.clone());
        assert!(!e.materialize(0x99, &only_ps));
        assert_eq!(e.declared(Some(0x99), 0x77, 3), None);
        let both = |h: u64| match h {
            0xaaa => Some(ShaderCb::default()),
            0xbbb => Some(m.clone()),
            _ => None,
        };
        assert!(e.materialize(0x99, &both));
        assert_eq!(e.declared(Some(0x99), 0x77, 3), Some(0x140));
        assert_eq!(e.limit(Some(0x99), 0x77, 3), Some(0x44));
    }

    #[test]
    fn translator_limits_map_to_register_and_space_and_skip_bindless() {
        let mut m = std::collections::BTreeMap::new();
        m.insert(13, 0xffff_ffffu64 * 16);
        m.insert((1 << 16) | 2, 0x20);
        m.insert(0x8000_0005, 0x40);
        assert_eq!(slot_limits(&m), vec![(13, 0, 0xffff_ffff * 16), (2, 1, 0x20)]);
        let mut e = Engine::default();
        e.rs_params.insert(0x77, vec![(3, 13, 0, 5), (4, 2, 1, 0)]);
        e.pso_sizes.insert(0x99, vec![(5, 13, 0, 0x1000), (5, 2, 1, 0x100), (1, 2, 1, 0x80)]);
        e.cb_limits.insert(0x99, vec![(5, 13, 0, 0xffff_ffff * 16), (5, 2, 1, 0x20)]);
        assert_eq!(e.limit(Some(0x99), 0x77, 3), Some(0xffff_ffff * 16));
        assert_eq!(e.limit(Some(0x99), 0x77, 4), None);
        e.cb_limits.insert(0x99, vec![(5, 13, 0, 0xffff_ffff * 16), (5, 2, 1, 0x20), (1, 2, 1, 0x30)]);
        assert_eq!(e.limit(Some(0x99), 0x77, 4), Some(0x30));
        let p = size_plan(e.declared(Some(0x99), 0x77, 3), 0x400, e.limit(Some(0x99), 0x77, 3)).unwrap();
        assert_eq!(p, SizePlan { alloc: 0x1000, copy: 0x400, zero: 0xc00 });
    }

    #[test]
    fn size_and_clamp_maths() {
        assert_eq!(size_plan(Some(64), 0x1000, None), Some(SizePlan { alloc: 256, copy: 256, zero: 0 }));
        assert_eq!(size_plan(Some(336), 100, None), Some(SizePlan { alloc: 512, copy: 100, zero: 412 }));
        assert_eq!(size_plan(Some(0), 8, None), Some(SizePlan { alloc: 256, copy: 8, zero: 248 }));
        assert_eq!(size_plan(Some(1 << 20), 1 << 20, None), Some(SizePlan { alloc: MAX_CB, copy: MAX_CB, zero: 0 }));
        assert_eq!(size_plan(None, 1 << 30, None), Some(SizePlan { alloc: MAX_CB, copy: MAX_CB, zero: 0 }));
        assert_eq!(size_plan(None, 0x130, None), Some(SizePlan { alloc: MAX_CB, copy: 0x130, zero: MAX_CB - 0x130 }));
        assert_eq!(size_plan(None, 0, None), None);
        assert_eq!(size_plan(Some(MAX_CB), 1 << 20, Some(0x340)), Some(SizePlan { alloc: MAX_CB, copy: 0x340, zero: MAX_CB - 0x340 }));
        assert_eq!(size_plan(Some(0x200), 0x1000, Some(0)), Some(SizePlan { alloc: 0x200, copy: 0, zero: 0x200 }));
        assert_eq!(size_plan(Some(0x200), 0x80, Some(0x100)), Some(SizePlan { alloc: 0x200, copy: 0x80, zero: 0x180 }));
        assert_eq!(round_up(257, 256), 512);
        assert_eq!(source_barrier(0), None);
        assert_eq!(source_barrier(0xac3), None);
        assert_eq!(source_barrier(0x800), None);
        assert_eq!(source_barrier(0x1), Some((0x1, 0x801)));
        assert_eq!(source_barrier(0x8), Some((0x8, 0x800)));
    }

    #[test]
    fn ring_allocates_aligned_slots_grows_and_reuses_after_the_fence() {
        let mut r = Ring::default();
        let never = |_: usize, _: u64| false;
        assert_eq!(r.alloc(1, 64, &never), None);
        let c0 = r.add_chunk(0xa, 0x10000);
        assert_eq!(r.alloc(1, 64, &never), Some((c0, 0)));
        assert_eq!(r.alloc(1, 300, &never), Some((c0, 256)));
        assert_eq!(r.alloc(1, 256, &never), Some((c0, 768)));
        assert_eq!(r.loc(c0, 768), (0xa, 0x10000 + 768));
        assert_eq!(r.alloc(2, 64, &never), None);
        let c1 = r.add_chunk(0xb, 0x900000);
        assert_eq!(r.alloc(2, 64, &never), Some((c1, 0)));
        assert_eq!(r.alloc(1, CHUNK_BYTES, &never), None);
        let c2 = r.add_chunk(0xc, 0x400000);
        assert_eq!(r.alloc(1, CHUNK_BYTES, &never), Some((c2, 0)));
        assert_eq!(r.alloc(1, 1, &never), None);
        assert_eq!(r.alloc(1, CHUNK_BYTES + 1, &never), None);
        r.set_state(1, c0, ST_VCB);
        assert_eq!(r.state(1, c0), ST_VCB);
        r.stamp(1, 0xf, 5);
        r.stamp(1, 0xf, 7);
        r.stamp(1, 0xf, 6);
        assert_eq!(r.state(1, c0), ST_COMMON);
        r.release(1);
        assert_eq!(r.retired_count(), 2);
        assert_eq!(r.alloc(3, 64, &|_, v| v <= 6), None);
        assert_eq!(r.retired_count(), 2);
        let got = r.alloc(3, 64, &|f, v| f == 0xf && v <= 7).unwrap();
        assert!(got.0 == c0 || got.0 == c2);
        assert_eq!(got.1, 0);
        assert_eq!(r.retired_count(), 0);
        r.release(2);
        assert!(r.free_count() >= 2);
        let back = r.alloc(4, 64, &never).unwrap();
        assert_eq!(back, (c1, 0));
    }

    #[test]
    fn unaligned_root_cbvs_pend_and_aligned_ones_forward() {
        let mut e = Engine::default();
        assert_eq!(e.on_root_cbv(1, SIDE_GRAPHICS, 3, 0x1000, false), Action::Forward);
        assert_eq!(e.on_root_cbv(1, SIDE_GRAPHICS, 3, 0x10b0, false), Action::Hold);
        assert_eq!(e.on_root_cbv(1, SIDE_COMPUTE, 0, 0x2030, false), Action::Hold);
        assert_eq!(e.on_root_cbv(1, SIDE_GRAPHICS, 4, 0x2030, true), Action::Forward);
        assert_eq!(e.pending(1), vec![((0, 0), 0x2030), ((1, 3), 0x10b0)]);
        assert_eq!(e.on_root_cbv(1, SIDE_GRAPHICS, 3, 0x2000, false), Action::Forward);
        assert_eq!(e.pending(1), vec![((0, 0), 0x2030)]);
        e.on_root_signature(1, SIDE_COMPUTE, 0x77);
        assert!(e.pending(1).is_empty());
        e.on_root_cbv(1, SIDE_COMPUTE, 0, 0x2030, false);
        e.on_root_signature(1, SIDE_COMPUTE, 0x77);
        assert_eq!(e.pending(1).len(), 1);
        e.on_root_signature(1, SIDE_GRAPHICS, 0x88);
        assert_eq!(e.pending(1).len(), 1);
        e.clear_state(1);
        assert!(e.pending(1).is_empty());
    }

    #[test]
    fn draws_copy_once_and_dedupe_until_a_title_write() {
        let mut e = Engine::default();
        let look = src(0x5550, 0xbe2_0000, 0x1000);
        let mut made = Vec::new();
        let mut mk = || {
            made.push(());
            Some((0xc000usize + made.len(), 0x7000_0000u64 * made.len() as u64))
        };
        let done = |_: usize, _: u64| true;
        e.rs_params.insert(0x77, vec![(2, 13, 0, 5)]);
        e.pso_sizes.insert(0x99, vec![(5, 13, 0, 0x470), (1, 0, 0, 0x40)]);
        assert_eq!(e.declared(Some(0x99), 0x77, 2), Some(0x470));
        assert_eq!(e.declared(Some(0x99), 0x77, 3), None);
        assert_eq!(e.declared(None, 0x77, 2), None);
        e.on_root_signature(1, SIDE_GRAPHICS, 0x77);
        e.on_root_cbv(1, SIDE_GRAPHICS, 2, 0xbe2_00b0, false);
        let j = e.plan(1, 0xd, 0, MASK_GRAPHICS, Some(0x99), &look, &mut mk, &done);
        assert_eq!(j.copies, vec![CopyOp { dst: 0xc001, dst_off: 0, src: 0x5550, src_off: 0xb0, n: 0x500, zero: 0 }]);
        assert_eq!(j.chunks, vec![(0xc001, ST_COMMON, ST_VCB)]);
        assert_eq!(j.binds, vec![(SIDE_GRAPHICS, 2, 0x7000_0000)]);
        assert!(j.sources.is_empty());
        assert!(!e.has_pending(1, MASK_GRAPHICS));
        assert_eq!(e.plan(1, 0xd, 0, MASK_GRAPHICS, Some(0x99), &look, &mut mk, &done), Job::default());
        assert_eq!(e.on_root_cbv(1, SIDE_GRAPHICS, 2, 0xbe2_00b0, false), Action::Provisional(0x7000_0000));
        let j = e.plan(1, 0xd, 0, MASK_GRAPHICS, Some(0x99), &look, &mut mk, &done);
        assert!(j.copies.is_empty());
        assert_eq!(j.binds, vec![(SIDE_GRAPHICS, 2, 0x7000_0000)]);
        e.invalidate(1);
        assert_eq!(e.on_root_cbv(1, SIDE_GRAPHICS, 2, 0xbe2_00b0, false), Action::Hold);
        let j = e.plan(1, 0xd, 0, MASK_GRAPHICS, Some(0x99), &look, &mut mk, &done);
        assert_eq!(j.copies.len(), 1);
        assert_eq!(j.copies[0].dst_off, 0x500);
        assert_eq!(j.chunks, vec![(0xc001, ST_VCB, ST_VCB)]);
        assert_eq!(j.binds, vec![(SIDE_GRAPHICS, 2, 0x7000_0500)]);
        e.on_root_cbv(1, SIDE_GRAPHICS, 2, 0xbe2_0ff0, false);
        let j = e.plan(1, 0xd, 0, MASK_GRAPHICS, Some(0x99), &look, &mut mk, &done);
        assert_eq!((j.copies[0].n, j.copies[0].zero), (0x10, 0x4f0));
        e.on_root_cbv(1, SIDE_GRAPHICS, 2, 0x4440, false);
        let j = e.plan(1, 0xd, 0, MASK_GRAPHICS, Some(0x99), &look, &mut mk, &done);
        assert_eq!(j.unresolved, vec![(SIDE_GRAPHICS, 2, 0x4440)]);
        e.on_root_cbv(1, SIDE_COMPUTE, 0, 0xbe2_0010, false);
        let j = e.plan(1, 0xd, 0, MASK_GRAPHICS, None, &look, &mut mk, &done);
        assert_eq!(j, Job::default());
        let j = e.plan(1, 0xd, 0, MASK_COMPUTE, None, &look, &mut mk, &done);
        assert_eq!((j.copies[0].n, j.copies[0].zero), (0xff0, MAX_CB - 0xff0));
        assert_eq!(made.len(), 1);
        assert!(e.holds(1));
        e.stamp(1, 0xf, 9);
        e.forget(1);
        assert!(!e.holds(1));
        assert!(e.pending(1).is_empty());
    }

    #[test]
    fn default_heap_sources_get_a_copy_source_barrier() {
        let mut e = Engine::default();
        let look = |va: u64| Some(Src { res: 0x42, off: va & 0xff, left: 0x100, state: ST_VCB });
        let mut mk = || Some((0xc0, 0x9000u64));
        e.on_root_cbv(1, SIDE_GRAPHICS, 0, 0x30, false);
        e.on_root_cbv(1, SIDE_GRAPHICS, 1, 0x50, false);
        let j = e.plan(1, 0xd, 0, MASK_GRAPHICS, None, &look, &mut mk, &|_, _| true);
        assert_eq!(j.sources, vec![(0x42, ST_VCB, ST_VCB | ST_COPY_SOURCE)]);
        assert_eq!(j.copies.len(), 2);
        assert_eq!(j.chunks.len(), 1);
    }

    #[test]
    fn every_hooked_slot_has_a_thunk_and_draws_resolve_the_right_side() {
        for s in DRAW_SLOTS.iter().chain(WRITE_SLOTS.iter()).chain([X_CLEAR_STATE].iter()) {
            assert_ne!(late_thunk(*s), 0, "slot {s}");
        }
        assert_eq!(slot_mask(X_DRAW), MASK_GRAPHICS);
        assert_eq!(slot_mask(X_DISPATCH), MASK_COMPUTE);
        assert_eq!(slot_mask(X_EXECUTE_INDIRECT), MASK_COMPUTE | MASK_GRAPHICS);
        assert_eq!(slot_mask(15), 0);
        assert!(slot_writes(15) && slot_writes(26) && slot_writes(X_DISPATCH) && !slot_writes(X_DRAW));
        assert_eq!(crate::cmdlist_wrap::map_slot(X_COMPUTE_ROOT_CBV), Some(PC_SET_COMPUTE_ROOT_CBV));
        assert_eq!(crate::cmdlist_wrap::map_slot(X_GRAPHICS_ROOT_CBV), Some(PC_SET_GRAPHICS_ROOT_CBV));
        assert_eq!(crate::cmdlist_wrap::map_slot(X_SET_GRAPHICS_RS), Some(30));
    }

    static T_PREV: AtomicUsize = AtomicUsize::new(0);
    static T_SEEN: Mutex<Vec<(usize, u64, u32, f32, u64, u64)>> = Mutex::new(Vec::new());

    unsafe extern "system" fn t_target(this: *mut c_void, a: u64, b: u32, x: f32, e: u64, f: u64) {
        T_SEEN.lock().unwrap().push((this as usize, a, b, x, e, f));
    }

    #[test]
    fn late_thunks_forward_every_argument_unchanged() {
        T_PREV.store(t_target as usize, Ordering::Relaxed);
        LATE_PREV[88].store(t_target as usize, Ordering::Relaxed);
        let f: unsafe extern "system" fn(*mut c_void, u64, u32, f32, u64, u64) = unsafe { core::mem::transmute(late_thunk(88)) };
        unsafe { f(0x1234 as *mut c_void, 0xdead_beef_0000_1111, 77, 2.5, 0x5555, 0x6666) };
        assert_eq!(T_SEEN.lock().unwrap().pop(), Some((0x1234, 0xdead_beef_0000_1111, 77, 2.5, 0x5555, 0x6666)));
    }
}
