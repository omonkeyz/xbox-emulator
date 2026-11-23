//! Binding trace (diagnostic, only with the render-target capture enabled).
//!
//! Records every descriptor the title writes (constant-buffer, shader-resource,
//! unordered-access views and samplers, including copies between heaps) and the descriptor
//! tables bound on each command list, so that a traced draw can be reported with what its
//! shaders actually see: the bytes of each constant buffer, and which resource every view
//! refers to (with the view's own stride/format).
//!
//! Limits: constant-buffer contents are read from upload-heap buffers only (the title maps
//! those itself; a default-heap buffer has no CPU-visible memory). A slot is reported only
//! if its descriptor was written during the current or the previous frame, because the
//! title leaves unused slots of a table untouched and stale views from an older draw would
//! otherwise be listed.

use core::ffi::c_void;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

#[derive(Clone, PartialEq, Eq)]
pub enum Desc {
    Cbv { va: u64, size: u32 },
    Srv { res: usize, desc: Option<[u8; 40]> },
    Uav { res: usize, desc: Option<[u8; 40]> },
    Sampler { desc: [u8; 0x34] },
}

#[derive(Clone)]
struct Entry {
    d: Desc,
    frame: u32,
}

struct Heap {
    owner: Option<usize>,
    heap_type: u32,
    cpu: usize,
    gpu: u64,
    count: usize,
    inc: usize,
}

struct Buf {
    size: u64,
    res: usize,
    /// D3D12_HEAP_TYPE (1 default, 2 upload, 3 readback); 0 = unknown.
    heap_type: u32,
    mapped: usize,
}

#[derive(Default)]
struct State {
    descs: HashMap<usize, Entry>,
    heaps: Vec<Heap>,
    /// descriptor increment per D3D12_DESCRIPTOR_HEAP_TYPE
    inc: [usize; 4],
    buffers: BTreeMap<u64, Buf>,
    /// heap object -> D3D12_HEAP_TYPE, for buffers placed in heaps
    heap_types: HashMap<usize, u32>,
    /// (list, compute?) -> root parameter index -> GPU descriptor handle
    tables: HashMap<(usize, bool), Vec<u64>>,
    root_cbvs: HashMap<(usize, bool), BTreeMap<u32, u64>>,
    frame: u32,
    by_bytes: HashMap<Vec<u8>, Desc>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap();
    f(g.get_or_insert_with(State::default))
}

unsafe fn vt(obj: *mut c_void) -> *const usize {
    *(obj as *const *const usize)
}

pub fn set_increment(heap_type: u32, inc: u32) {
    with(|s| {
        if (heap_type as usize) < 4 {
            s.inc[heap_type as usize] = inc as usize;
        }
    });
}

pub fn increment(heap_type: u32) -> usize {
    with(|s| s.inc.get(heap_type as usize).copied().unwrap_or(0))
}

pub fn note_heap(owner: usize, cpu: usize, gpu: u64, count: u32, heap_type: u32) {
    with(|s| {
        let inc = s.inc.get(heap_type as usize).copied().unwrap_or(0);
        if crate::tbuf_compute::enabled() && heap_type == 0 {
            if let Some(end) = (count as usize).checked_mul(inc).and_then(|n| cpu.checked_add(n)) {
                s.descs.retain(|&handle, _| handle < cpu || handle >= end);
            }
        }
        s.heaps.retain(|h| h.cpu != cpu && h.owner != Some(owner));
        if gpu != 0 {
            s.heaps.push(Heap { owner: Some(owner), heap_type, cpu, gpu, count: count as usize, inc });
        }
    });
}

pub fn typed_buffer_format(heaps: &[usize], table: u64, index: u32, uav: bool) -> Option<u32> {
    if table == 0 || crate::title_heap::locate(table).is_some() {
        return None;
    }
    with(|s| s.typed_buffer_format(heaps, table, index, uav, |cpu, inc| unsafe { desc_bytes(cpu, inc) }))
}

impl State {
    fn typed_buffer_format(
        &self,
        heaps: &[usize],
        table: u64,
        index: u32,
        uav: bool,
        bytes_at: impl FnOnce(usize, usize) -> Option<Vec<u8>>,
    ) -> Option<u32> {
        let (heap, slot) = self.heaps.iter().filter(|h| h.heap_type == 0 && h.owner.is_some_and(|owner| heaps.contains(&owner)))
            .find_map(|heap| {
                let bytes = usize::try_from(table.checked_sub(heap.gpu)?).ok()?;
                if heap.inc == 0 || bytes % heap.inc != 0 || bytes / heap.inc >= heap.count {
                    return None;
                }
                let slot = (bytes / heap.inc).checked_add(index as usize)?;
                (slot < heap.count).then_some((heap, slot))
            })?;
        let cpu = heap.cpu.checked_add(slot.checked_mul(heap.inc)?)?;
        let observed = &self.descs.get(&cpu)?.d;
        if self.by_bytes.get(&bytes_at(cpu, heap.inc)?)? != observed {
            return None;
        }
        let (desc, stride) = match (uav, observed) {
            (false, Desc::Srv { res, desc: Some(desc) }) if *res != 0 => (desc, 28),
            (true, Desc::Uav { res, desc: Some(desc) }) if *res != 0 => (desc, 20),
            _ => return None,
        };
        let word = |i: usize| u32::from_le_bytes(desc[i..i + 4].try_into().unwrap());
        (word(4) == 1 && word(stride) == 0 && word(32) == 0).then(|| word(0))
    }
}

#[cfg(test)]
mod typed_buffer_tests {
    use super::*;

    #[test]
    fn typed_format_requires_a_current_matching_descriptor_on_the_bound_heap() {
        let mut desc = [0u8; 40];
        desc[..4].copy_from_slice(&42u32.to_le_bytes());
        desc[4..8].copy_from_slice(&1u32.to_le_bytes());
        let view = Desc::Uav { res: 1, desc: Some(desc) };
        let mut state = State::default();
        state.heaps.push(Heap { owner: Some(42), heap_type: 0, cpu: 0x1000, gpu: 0x8000, count: 3, inc: 32 });
        state.descs.insert(0x1040, Entry { d: view.clone(), frame: 0 });
        state.by_bytes.insert(vec![4; 32], view);
        let lookup = |heaps: &[usize], table, uav, live_byte| state.typed_buffer_format(heaps, table, 1, uav,
            |cpu, inc| (cpu == 0x1040 && inc == 32).then(|| vec![live_byte; inc]));
        assert_eq!(lookup(&[42], 0x8020, true, 4), Some(42));
        assert_eq!(lookup(&[42], 0x8020, true, 5), None);
        assert_eq!(lookup(&[42], 0x8020, false, 4), None);
        assert_eq!(lookup(&[7], 0x8020, true, 4), None);
        assert_eq!(lookup(&[42], 0x8021, true, 4), None);
        assert_eq!(lookup(&[42], 0x8040, true, 4), None);
        state.heaps[0].owner = None;
        assert_eq!(state.typed_buffer_format(&[42], 0x8020, 1, true, |_, inc| Some(vec![4; inc])), None);
    }
}

extern "system" {
    fn IsBadReadPtr(p: *const c_void, n: usize) -> i32;
}

unsafe fn desc_bytes(addr: usize, inc: usize) -> Option<Vec<u8>> {
    if addr < 0x10000 || inc == 0 || inc > 256 || IsBadReadPtr(addr as *const c_void, inc) != 0 {
        return None;
    }
    let v = core::slice::from_raw_parts(addr as *const u8, inc).to_vec();
    if v.iter().all(|&b| b == 0) { None } else { Some(v) }
}

pub fn note_desc_after(dest: usize, heap_type: u32, d: Desc) {
    let inc = increment(heap_type);
    if let Some(bytes) = unsafe { desc_bytes(dest, inc) } {
        with(|s| {
            if s.by_bytes.len() > 400_000 {
                s.by_bytes.clear();
            }
            s.by_bytes.insert(bytes, d);
        });
    }
}

pub fn all_views() -> Vec<(usize, bool)> {
    with(|s| {
        s.descs.values().filter_map(|e| match &e.d {
            Desc::Srv { res, .. } if *res != 0 => Some((*res, false)),
            Desc::Uav { res, .. } if *res != 0 => Some((*res, true)),
            _ => None,
        }).collect()
    })
}

pub fn note_desc(dest: usize, d: Desc) {
    match &d {
        Desc::Srv { res, .. } => crate::firstuse::note(*res, crate::firstuse::SRV),
        Desc::Uav { res, .. } => crate::firstuse::note(*res, crate::firstuse::UAV),
        _ => {}
    }
    with(|s| {
        let frame = s.frame;
        s.descs.insert(dest, Entry { d, frame });
    });
}

/// Mirror `CopyDescriptorsSimple` / one range pair of `CopyDescriptors`.
pub fn copy_desc(dest: usize, src: usize, n: u32, heap_type: u32) {
    with(|s| {
        let inc = s.inc.get(heap_type as usize).copied().unwrap_or(0);
        if inc == 0 {
            return;
        }
        let frame = s.frame;
        for i in 0..n as usize {
            match s.descs.get(&(src + i * inc)).cloned() {
                Some(mut e) => {
                    e.frame = frame;
                    s.descs.insert(dest + i * inc, e);
                }
                None => {
                    s.descs.remove(&(dest + i * inc));
                }
            }
        }
    });
}

pub fn note_heap_type(heap: usize, heap_type: u32) {
    with(|s| {
        s.heap_types.insert(heap, heap_type);
    });
}
pub fn heap_type_of(heap: usize) -> u32 {
    with(|s| s.heap_types.get(&heap).copied().unwrap_or(0))
}

/// Remember a buffer resource (takes one reference so the pointer stays valid; released by
/// `end_frame` once the title has dropped the buffer).
pub unsafe fn note_buffer(res: *mut c_void, size: u64, heap_type: u32) {
    if res.is_null() {
        return;
    }
    let get_va: unsafe extern "system" fn(*mut c_void) -> u64 = core::mem::transmute(*vt(res).add(11));
    let va = get_va(res);
    if va == 0 {
        return;
    }
    let addref: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt(res).add(1));
    addref(res);
    let old = with(|s| s.buffers.insert(va, Buf { size, res: res as usize, heap_type, mapped: 0 }));
    if let Some(b) = old {
        let release: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt(b.res as *mut c_void).add(2));
        release(b.res as *mut c_void);
    }
}

pub fn heaps() -> Vec<(usize, u64, usize, usize)> {
    with(|s| s.heaps.iter().map(|h| (h.cpu, h.gpu, h.count, h.inc)).collect())
}

pub fn descs_in(cpu: usize, len: usize) -> Vec<(usize, Desc)> {
    with(|s| {
        let mut v: Vec<(usize, Desc)> = s.descs.iter().filter(|(k, _)| **k >= cpu && **k < cpu + len).map(|(k, e)| (*k, e.d.clone())).collect();
        v.sort_by_key(|e| e.0);
        v
    })
}

pub fn buffer_at(va: u64) -> Option<(u64, usize, u64, u32)> {
    with(|s| s.buffers.range(..=va).next_back().filter(|(base, b)| va < **base + b.size).map(|(base, b)| (*base, b.res, b.size, b.heap_type)))
}

pub unsafe fn read_upload(base: u64) -> Option<Vec<u8>> {
    let (res, size, heap_type, mut mapped) = with(|s| s.buffers.get(&base).map(|b| (b.res, b.size, b.heap_type, b.mapped)))?;
    if heap_type != 2 {
        return None;
    }
    if mapped == 0 {
        let map: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut u8) -> i32 = core::mem::transmute(*vt(res as *mut c_void).add(8));
        let mut p: *mut u8 = core::ptr::null_mut();
        if map(res as *mut c_void, 0, core::ptr::null(), &mut p) < 0 || p.is_null() {
            return None;
        }
        mapped = p as usize;
        with(|s| {
            if let Some(b) = s.buffers.get_mut(&base) {
                b.mapped = mapped;
            }
        });
    }
    Some(core::slice::from_raw_parts(mapped as *const u8, size as usize).to_vec())
}

pub unsafe fn read_va(va: u64, n: usize) -> Option<Vec<u8>> {
    let (base, _, size, heap_type) = buffer_at(va)?;
    if heap_type != 2 {
        return None;
    }
    let mut mapped = with(|s| s.buffers.get(&base).map(|b| b.mapped))?;
    if mapped == 0 {
        read_upload(base).map(|_| ())?;
        mapped = with(|s| s.buffers.get(&base).map(|b| b.mapped))?;
    }
    if mapped == 0 {
        return None;
    }
    let off = (va - base) as usize;
    let n = n.min((size as usize).saturating_sub(off));
    Some(core::slice::from_raw_parts((mapped + off) as *const u8, n).to_vec())
}

pub fn note_heap_inc(cpu: usize, gpu: u64, count: u32, inc: usize) {
    with(|s| {
        s.heaps.retain(|h| h.cpu != cpu);
        s.heaps.push(Heap { owner: None, heap_type: 0, cpu, gpu, count: count as usize, inc });
    });
}

pub fn set_table(list: usize, compute: bool, index: u32, handle: u64) {
    let handle = crate::title_heap::locate(handle).map_or(handle, |t| t.2);
    with(|s| {
        let v = s.tables.entry((list, compute)).or_default();
        if (index as usize) < 64 {
            if v.len() <= index as usize {
                v.resize(index as usize + 1, 0);
            }
            v[index as usize] = handle;
        }
    });
}

pub fn set_root_cbv(list: usize, compute: bool, index: u32, va: u64) {
    with(|s| {
        s.root_cbvs.entry((list, compute)).or_default().insert(index, va);
    });
}

static REAL_ROOT_CBV: [core::sync::atomic::AtomicUsize; 2] = [const { core::sync::atomic::AtomicUsize::new(0) }; 2];
pub const XBOX_SLOT_SET_COMPUTE_ROOT_CBV: usize = 41;
pub const XBOX_SLOT_SET_GRAPHICS_ROOT_CBV: usize = 42;
const PC_SLOT_SET_COMPUTE_ROOT_CBV: usize = 37;
const PC_SLOT_SET_GRAPHICS_ROOT_CBV: usize = 38;

unsafe extern "system" fn set_compute_root_cbv_x(this: *mut c_void, index: u32, va: u64) {
    set_root_cbv(this as usize, true, index, va);
    let f: unsafe extern "system" fn(*mut c_void, u32, u64) = core::mem::transmute(REAL_ROOT_CBV[0].load(core::sync::atomic::Ordering::Relaxed));
    f(this, index, va)
}

unsafe extern "system" fn set_graphics_root_cbv_x(this: *mut c_void, index: u32, va: u64) {
    set_root_cbv(this as usize, false, index, va);
    let f: unsafe extern "system" fn(*mut c_void, u32, u64) = core::mem::transmute(REAL_ROOT_CBV[1].load(core::sync::atomic::Ordering::Relaxed));
    f(this, index, va)
}

pub unsafe fn install_root_cbv_hooks(xbox_vt: &[core::sync::atomic::AtomicUsize], real_vt: *const usize) {
    use core::sync::atomic::Ordering::Relaxed;
    if !crate::rtcap::enabled() {
        return;
    }
    REAL_ROOT_CBV[0].store(*real_vt.add(PC_SLOT_SET_COMPUTE_ROOT_CBV), Relaxed);
    REAL_ROOT_CBV[1].store(*real_vt.add(PC_SLOT_SET_GRAPHICS_ROOT_CBV), Relaxed);
    xbox_vt[XBOX_SLOT_SET_COMPUTE_ROOT_CBV].store(set_compute_root_cbv_x as usize, Relaxed);
    xbox_vt[XBOX_SLOT_SET_GRAPHICS_ROOT_CBV].store(set_graphics_root_cbv_x as usize, Relaxed);
}

unsafe fn cbv_contents(va: u64, size: usize) -> String {
    let found = with(|s| {
        s.buffers.range(..=va).next_back().filter(|(base, b)| va < **base + b.size).map(|(base, b)| (*base, b.res, b.heap_type, b.mapped, b.size))
    });
    match found {
        Some((base, res, heap_type, mut mapped, bsize)) => {
            if mapped == 0 && heap_type == 2 {
                let map: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut u8) -> i32 = core::mem::transmute(*vt(res as *mut c_void).add(8));
                let mut p: *mut u8 = core::ptr::null_mut();
                if map(res as *mut c_void, 0, core::ptr::null(), &mut p) >= 0 && !p.is_null() {
                    mapped = p as usize;
                    with(|s| {
                        if let Some(b) = s.buffers.get_mut(&base) {
                            b.mapped = mapped;
                        }
                    });
                }
            }
            if mapped != 0 {
                let off = (va - base) as usize;
                let n = size.min((bsize as usize).saturating_sub(off)).min(8192);
                let bytes = core::slice::from_raw_parts((mapped + off) as *const u8, n);
                format!(" (buffer {res:#x}+{off:#x})\n{}", fmt_words(bytes))
            } else {
                format!(" (buffer {res:#x} heapType={heap_type}: not CPU-readable)\n")
            }
        }
        None => {
            let n = size.min(8192);
            if va >= 0x10000 && va < 0x8000_0000_0000 && IsBadReadPtr(va as *const c_void, n) == 0 {
                let bytes = core::slice::from_raw_parts(va as *const u8, n);
                format!(" (no PC buffer; read from title memory at that address)\n{}", fmt_words(bytes))
            } else {
                " (no known buffer at that address, not readable as title memory)\n".to_string()
            }
        }
    }
}

/// Called at every present: advances the frame counter and, now and then, drops buffers the
/// title has released (our reference is the only one left).
pub unsafe fn end_frame() {
    let dead: Vec<usize> = with(|s| {
        s.frame = s.frame.wrapping_add(1);
        if s.frame % 64 != 0 {
            return Vec::new();
        }
        let mut dead = Vec::new();
        s.buffers.retain(|_, b| {
            let r = b.res as *mut c_void;
            let addref: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt(r).add(1));
            let release: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt(r).add(2));
            addref(r);
            if release(r) == 1 {
                dead.push(b.res);
                false
            } else {
                true
            }
        });
        // Descriptor records for heaps are kept; only very old ones are dropped.
        if s.frame % 1024 == 0 {
            let f = s.frame;
            s.descs.retain(|_, e| f.wrapping_sub(e.frame) < 4096);
        }
        dead
    });
    for r in dead {
        let r = r as *mut c_void;
        let release: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt(r).add(2));
        release(r);
    }
}

fn fmt_words(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (i, row) in bytes.chunks(16).enumerate() {
        out.push_str(&format!("      [{i:3}]"));
        for w in row.chunks(4) {
            if w.len() < 4 {
                break;
            }
            let u = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            let f = f32::from_bits(u);
            // Show as float when it looks like one, else as hex (integers, bit patterns).
            if u == 0 {
                out.push_str("            0");
            } else if f.is_finite() && f.abs() >= 1e-6 && f.abs() < 1e9 {
                out.push_str(&format!(" {f:12.6}"));
            } else {
                out.push_str(&format!("   0x{u:08x}"));
            }
        }
        out.push('\n');
    }
    out
}

/// Text describing every table bound on `list` (graphics or compute).
pub fn table_srv(list: usize, compute: bool, param: u32, slot: u32) -> Option<(usize, u32, u32)> {
    with(|s| {
        let h = *s.tables.get(&(list, compute))?.get(param as usize)?;
        if h == 0 {
            return None;
        }
        let heap = s.heaps.iter().find(|x| x.gpu != 0 && x.inc != 0 && h >= x.gpu && h < x.gpu + (x.count * x.inc) as u64)?;
        let idx = (h - heap.gpu) as usize / heap.inc + slot as usize;
        if idx >= heap.count {
            return None;
        }
        let cpu = heap.cpu + idx * heap.inc;
        let d = match s.descs.get(&cpu) {
            Some(e) => e.d.clone(),
            None => unsafe { desc_bytes(cpu, heap.inc) }.and_then(|b| s.by_bytes.get(&b).cloned())?,
        };
        match d {
            Desc::Srv { res, desc } => {
                let u = |o: usize| desc.map(|d| u32::from_le_bytes(d[o..o + 4].try_into().unwrap())).unwrap_or(0);
                Some((res, u(4), u(8)))
            }
            _ => None,
        }
    })
}

pub fn table_srv_fmt(list: usize, compute: bool, param: u32, slot: u32) -> Option<(usize, u32, u32, u32)> {
    with(|s| {
        let h = *s.tables.get(&(list, compute))?.get(param as usize)?;
        if h == 0 {
            return None;
        }
        let heap = s.heaps.iter().find(|x| x.gpu != 0 && x.inc != 0 && h >= x.gpu && h < x.gpu + (x.count * x.inc) as u64)?;
        let idx = (h - heap.gpu) as usize / heap.inc + slot as usize;
        if idx >= heap.count {
            return None;
        }
        let cpu = heap.cpu + idx * heap.inc;
        let d = match s.descs.get(&cpu) {
            Some(e) => e.d.clone(),
            None => unsafe { desc_bytes(cpu, heap.inc) }.and_then(|b| s.by_bytes.get(&b).cloned())?,
        };
        match d {
            Desc::Srv { res, desc } => {
                let u = |o: usize| desc.map(|d| u32::from_le_bytes(d[o..o + 4].try_into().unwrap())).unwrap_or(0);
                Some((res, u(0), u(4), u(8)))
            }
            _ => None,
        }
    })
}

pub fn table_uav(list: usize, compute: bool, param: u32, slot: u32) -> Option<(usize, Option<[u8; 40]>)> {
    with(|s| {
        let h = *s.tables.get(&(list, compute))?.get(param as usize)?;
        if h == 0 {
            return None;
        }
        let heap = s.heaps.iter().find(|x| x.gpu != 0 && x.inc != 0 && h >= x.gpu && h < x.gpu + (x.count * x.inc) as u64)?;
        let idx = (h - heap.gpu) as usize / heap.inc + slot as usize;
        if idx >= heap.count {
            return None;
        }
        let cpu = heap.cpu + idx * heap.inc;
        let d = match s.descs.get(&cpu) {
            Some(e) => e.d.clone(),
            None => unsafe { desc_bytes(cpu, heap.inc) }.and_then(|b| s.by_bytes.get(&b).cloned())?,
        };
        match d {
            Desc::Uav { res, desc } => Some((res, desc)),
            _ => None,
        }
    })
}

pub fn uav_view_text(desc: Option<[u8; 40]>) -> String {
    let Some(d) = desc else { return "default view (null desc)".to_string() };
    let u = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let (fmt, dim) = (u(0), u(4));
    match dim {
        4 => format!("fmt={fmt} Texture2D MipSlice={} PlaneSlice={}", u(8), u(12)),
        5 => format!("fmt={fmt} Texture2DArray MipSlice={} First={} Size={} PlaneSlice={}", u(8), u(12), u(16), u(20)),
        3 => format!("fmt={fmt} Texture1DArray MipSlice={} First={} Size={}", u(8), u(12), u(16)),
        2 => format!("fmt={fmt} Texture1D MipSlice={}", u(8)),
        8 => format!("fmt={fmt} Texture3D MipSlice={} FirstW={} WSize={}", u(8), u(12), u(16)),
        1 => format!("fmt={fmt} Buffer first={} n={} stride={}", u64::from_le_bytes(d[8..16].try_into().unwrap()), u(16), u(20)),
        _ => format!("fmt={fmt} ViewDimension={dim}"),
    }
}

pub fn table_count(list: usize, compute: bool) -> usize {
    with(|s| s.tables.get(&(list, compute)).map_or(0, |t| t.len()))
}

pub unsafe fn dump(list: usize, compute: bool, name: &dyn Fn(usize) -> String) -> String {
    // Collect under the lock, read memory after it (Map may take driver locks).
    struct Item {
        param: usize,
        slot: usize,
        /// None: no record written this frame (`age` frames old, or never recorded).
        d: Option<Desc>,
        age: u32,
        why: String,
    }
    let (items, frame) = with(|s| {
        let mut items = Vec::new();
        let Some(tables) = s.tables.get(&(list, compute)) else { return (items, s.frame) };
        for (param, &h) in tables.iter().enumerate() {
            if h == 0 {
                continue;
            }
            let Some(heap) = s.heaps.iter().find(|x| x.gpu != 0 && x.inc != 0 && h >= x.gpu && h < x.gpu + (x.count * x.inc) as u64) else { continue };
            let first = (h - heap.gpu) as usize / heap.inc;
            let hlo = heap.cpu;
            let hhi = heap.cpu + heap.count * heap.inc;
            let in_heap = s.descs.keys().filter(|&&k| k >= hlo && k < hhi).count();
            let mut why_once = true;
            // A table ends where the next table bound on this list begins (the title
            // allocates them back to back), or after 64 slots.
            let next = tables.iter().filter(|&&o| o > h && o < heap.gpu + (heap.count * heap.inc) as u64).min().copied();
            let extent = next.map(|o| (o - h) as usize / heap.inc).unwrap_or(64).min(64);
            for slot in 0..extent {
                if first + slot >= heap.count {
                    break;
                }
                match s.descs.get(&(heap.cpu + (first + slot) * heap.inc)) {
                    Some(e) if s.frame.wrapping_sub(e.frame) <= 1 => items.push(Item { param, slot, d: Some(e.d.clone()), age: 0, why: String::new() }),
                    Some(e) => items.push(Item { param, slot, d: Some(e.d.clone()), age: s.frame.wrapping_sub(e.frame), why: String::new() }),
                    None => {
                        let addr = heap.cpu + (first + slot) * heap.inc;
                        let raw = unsafe { desc_bytes(addr, heap.inc) };
                        let found = raw.as_ref().and_then(|b| s.by_bytes.get(b).cloned());
                        match found {
                            Some(d) => items.push(Item { param, slot, d: Some(d), age: 0, why: String::new() }),
                            None => {
                                let why = if why_once {
                                    why_once = false;
                                    let bytes = match &raw {
                                        Some(v) => format!("bytes {} not among {} recorded byte patterns", v.iter().take(16).map(|x| format!("{x:02x}")).collect::<String>(), s.by_bytes.len()),
                                        None => "bytes unreadable or all zero".to_string(),
                                    };
                                    format!(" [table gpu {h:#x} -> heap cpu {:#x} gpu {:#x} inc {} count {}; slot cpu {addr:#x}; {in_heap} of {} recorded descriptors lie in this heap; {bytes}]", heap.cpu, heap.gpu, heap.inc, heap.count, s.descs.len())
                                } else {
                                    String::new()
                                };
                                items.push(Item { param, slot, d: None, age: u32::MAX, why })
                            }
                        }
                    }
                }
            }
        }
        (items, s.frame)
    });
    let mut out = format!("  bindings ({} list {list:#x}, frame {frame}):\n", if compute { "compute" } else { "graphics" });
    let mut shown: Vec<(u64, u32)> = Vec::new();
    for it in items {
        let Some(d) = it.d.clone() else {
            if it.age == u32::MAX {
                out.push_str(&format!("    param[{}]+{} (no descriptor recorded){}
", it.param, it.slot, it.why));
            }
            continue;
        };
        match d {
            Desc::Cbv { va, size } => {
                out.push_str(&format!("    param[{}]+{} CBV va={va:#x} size={size}", it.param, it.slot));
                if shown.contains(&(va, size)) {
                    out.push_str(" (contents shown above)
");
                    continue;
                }
                shown.push((va, size));
                out.push_str(&cbv_contents(va, size as usize));
            }
            Desc::Srv { res, desc } | Desc::Uav { res, desc } => {
                let is_srv = matches!(it.d, Some(Desc::Srv { .. }));
                let kind = if is_srv { "SRV" } else { "UAV" };
                let view = match desc {
                    Some(d) => {
                        let u = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
                        let q = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
                        if is_srv {
                            // D3D12_SHADER_RESOURCE_VIEW_DESC: Format, ViewDimension, Mapping, union@16
                            match u(4) {
                                1 => format!("fmt={} BUFFER first={} n={} stride={} flags={:#x}", u(0), q(16), u(24), u(28), u(32)),
                                dim => format!("fmt={} dim={dim} map={:#x} mip0={} mips={} a={} b={}", u(0), u(8), u(16), u(20) as i32, u(24), u(28)),
                            }
                        } else {
                            // D3D12_UNORDERED_ACCESS_VIEW_DESC: Format, ViewDimension, union@8
                            match u(4) {
                                1 => format!("fmt={} BUFFER first={} n={} stride={} counterOff={} flags={:#x}", u(0), q(8), u(16), u(20), q(24), u(32)),
                                dim => format!("fmt={} dim={dim} mip={} a={} b={}", u(0), u(8), u(12), u(16)),
                            }
                        }
                    }
                    None => "default view".to_string(),
                };
                let what = if res == 0 {
                    "null resource".to_string()
                } else {
                    let buf = with(|s| s.buffers.values().find(|b| b.res == res).map(|b| (b.size, b.heap_type)));
                    match buf {
                        Some((size, ht)) => format!("buffer {res:#x} size={size} heapType={ht}"),
                        None => name(res),
                    }
                };
                let aged = if it.age > 1 { format!(" (written {} frames ago)", it.age) } else { String::new() };
                let st = crate::rtcap::state_of(res).or_else(|| Some(crate::gpu_va::state(res)).filter(|&x| x != 0)).map(|x| format!(" state={x:#x}")).unwrap_or_default();
                let loc = if res == 0 {
                    String::new()
                } else if let Some(p) = crate::placed::lookup(res as *mut c_void) {
                    format!(" placed title va={:#x} {}x{} dim={}", p.va, p.width(), p.height(), p.dim())
                } else if let Some(v) = crate::gpu_va::texture_va(res) {
                    format!(" texture va={v:#x}")
                } else {
                    " not placed".to_string()
                };
                let fu = if res == 0 { String::new() } else { crate::firstuse::describe(res) };
                out.push_str(&format!("    param[{}]+{} {kind} {what} | {view}{aged}{st}{loc}{fu}\n", it.param, it.slot));
            }
            Desc::Sampler { desc } => {
                let u = |o: usize| u32::from_le_bytes(desc[o..o + 4].try_into().unwrap());
                let f = |o: usize| f32::from_bits(u(o));
                out.push_str(&format!(
                    "    param[{}]+{} SAMPLER filter={:#x} addr={}/{}/{} bias={} aniso={} cmp={} lod=[{},{}]\n",
                    it.param, it.slot, u(0), u(4), u(8), u(12), f(0x10), u(0x14), u(0x18), f(0x2c), f(0x30)));
            }
        }
    }
    let roots: Vec<(u32, u64)> = with(|s| s.root_cbvs.get(&(list, compute)).map(|m| m.iter().map(|(k, v)| (*k, *v)).collect()).unwrap_or_default());
    for (index, va) in roots {
        out.push_str(&format!("    root[{index}] CBV va={va:#x}"));
        if va == 0 {
            out.push_str(" (null)\n");
            continue;
        }
        out.push_str(&cbv_contents(va, 512));
    }
    out
}
