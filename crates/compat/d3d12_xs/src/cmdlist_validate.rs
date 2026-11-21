use compat_common::record;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};

const M: &str = "d3d12_xs";
const TAG: &str = "cmdlist.validate";
const KEEP: usize = 48;
const KEEP_REC: usize = 1024;

pub fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| compat_common::live::flag("TOMBOX_CMDLIST_VALIDATE", "cmdlist_validate", false))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Param {
    Table,
    Constants(u32),
    Cbv,
    Srv,
    Uav,
}

#[derive(Default)]
struct ListState {
    log: VecDeque<String>,
    violations: Vec<String>,
    seq: u64,
    gfx_rs: Option<usize>,
    cmp_rs: Option<usize>,
    heaps: Vec<(u32, u32)>,
    events: i32,
    open_queries: HashSet<(usize, u32)>,
    ty: Option<u32>,
    heap_ranges: Vec<(u64, u64)>,
    recs: VecDeque<Rec>,
}

#[derive(Default)]
struct State {
    lists: HashMap<usize, ListState>,
    root_sigs: HashMap<usize, Vec<Param>>,
    query_heaps: HashMap<usize, (u32, u32)>,
}

static S: Mutex<Option<State>> = Mutex::new(None);
static NEXT: [AtomicUsize; 128] = [const { AtomicUsize::new(0) }; 128];
static DEV_RS: AtomicUsize = AtomicUsize::new(0);
static DEV_QH: AtomicUsize = AtomicUsize::new(0);
static REPORTS: AtomicUsize = AtomicUsize::new(0);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = S.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

fn next(slot: usize) -> usize {
    NEXT[slot].load(Ordering::Relaxed)
}

fn list_type(list: *mut c_void) -> Option<u32> {
    let f = crate::cmdlist_wrap::vt_get(8);
    if f == 0 || list.is_null() {
        return None;
    }
    let get: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(f) };
    Some(unsafe { get(list) })
}

const DIRECT_ONLY: &[&str] = &[
    "DrawInstanced", "DrawIndexedInstanced", "ClearRenderTargetView", "ClearDepthStencilView", "OMSetRenderTargets",
    "RSSetViewports", "RSSetScissorRects", "IASetIndexBuffer", "IASetVertexBuffers", "SetGraphicsRoot",
];
const NOT_COPY: &[&str] = &[
    "Dispatch", "SetComputeRoot", "SetPipelineState", "SetDescriptorHeaps", "ClearUnorderedAccessView", "ExecuteIndirect",
];

fn note(list: *mut c_void, cmd: String, mut bad: Vec<String>) {
    let known = with(|s| s.lists.get(&(list as usize)).and_then(|l| l.ty));
    let ty = known.or_else(|| list_type(list));
    if let Some(t) = ty {
        let head = cmd.split(' ').next().unwrap_or("");
        let direct_only = DIRECT_ONLY.iter().any(|p| head.starts_with(p));
        let not_copy = NOT_COPY.iter().any(|p| head.starts_with(p));
        let name = match t { 0 => "DIRECT", 1 => "BUNDLE", 2 => "COMPUTE", 3 => "COPY", _ => "OTHER" };
        if (t == 2 || t == 3) && direct_only {
            bad.push(format!("{head} is not allowed on a {name} command list"));
        } else if t == 3 && not_copy {
            bad.push(format!("{head} is not allowed on a COPY command list"));
        }
    }
    with(|s| {
        let l = s.lists.entry(list as usize).or_default();
        if l.ty.is_none() {
            l.ty = ty;
        }
        l.seq += 1;
        let seq = l.seq;
        for b in &bad {
            if l.violations.len() < 64 {
                l.violations.push(format!("#{seq} {cmd}: {b}"));
            }
        }
        let mark = if bad.is_empty() { String::new() } else { format!("   <-- {}", bad.join("; ")) };
        l.log.push_back(format!("#{seq} {cmd}{mark}"));
        while l.log.len() > KEEP {
            l.log.pop_front();
        }
    });
}

#[derive(Clone, Copy, Default, Debug)]
struct ResDesc {
    dim: u32,
    width: u64,
    height: u32,
    depth_or_array: u16,
    mips: u16,
    format: u32,
}

unsafe fn res_desc(res: *mut c_void) -> Option<ResDesc> {
    if res.is_null() {
        return None;
    }
    let vt = *(res as *const *const usize);
    let get: unsafe extern "system" fn(*mut c_void, *mut u8) -> *mut u8 = core::mem::transmute(*vt.add(10));
    let mut d = [0u8; 56];
    get(res, d.as_mut_ptr());
    let u32_at = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    Some(ResDesc {
        dim: u32_at(0),
        width: u64::from_le_bytes(d[16..24].try_into().unwrap()),
        height: u32_at(24),
        depth_or_array: u16::from_le_bytes(d[28..30].try_into().unwrap()),
        mips: u16::from_le_bytes(d[30..32].try_into().unwrap()).max(1),
        format: u32_at(32),
    })
}

fn planes(format: u32) -> u32 {
    match format {
        19 | 20 | 44 | 45 | 103 | 104 | 105 | 106 => 2,
        _ => 1,
    }
}

fn subresources(d: &ResDesc) -> u32 {
    let arr = if d.dim == 4 { 1 } else { d.depth_or_array.max(1) as u32 };
    if d.dim == 1 {
        return 1;
    }
    d.mips as u32 * arr * planes(d.format)
}

fn sub_extent(d: &ResDesc, sub: u32) -> (u64, u32, u32) {
    let mip = sub % d.mips as u32;
    let w = (d.width >> mip).max(1);
    let h = if d.dim >= 3 { (d.height >> mip).max(1) } else { 1 };
    let z = if d.dim == 4 { ((d.depth_or_array as u32) >> mip).max(1) } else { 1 };
    (w, h, z)
}

const WRITE_STATES: u32 = 0x4 | 0x8 | 0x10 | 0x100 | 0x400 | 0x1000;
const KNOWN_STATES: u32 = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x40 | 0x80 | 0x100 | 0x200 | 0x400 | 0x800 | 0x1000 | 0x2000
    | 0x4000 | 0x40_0000 | 0x80_0000 | 0x100_0000 | 0x1_0000 | 0x2_0000 | 0x4_0000 | 0x8_0000 | 0x10_0000 | 0x20_0000;

fn state_problem(s: u32) -> Option<String> {
    if s & !KNOWN_STATES != 0 {
        return Some(format!("state {s:#x} has bits {:#x} that are not D3D12_RESOURCE_STATES", s & !KNOWN_STATES));
    }
    let w = s & WRITE_STATES;
    if w != 0 && s != w {
        return Some(format!("state {s:#x} combines a write state with other states"));
    }
    if w.count_ones() > 1 {
        return Some(format!("state {s:#x} combines several write states"));
    }
    None
}

unsafe fn check_barriers(n: u32, b: *const u8) -> (String, Vec<String>) {
    let mut bad = Vec::new();
    let mut parts = Vec::new();
    if n > 0 && b.is_null() {
        bad.push("null barrier array".into());
        return (format!("ResourceBarrier n={n}"), bad);
    }
    for i in 0..n as usize {
        let p = b.add(i * 32);
        let ty = *(p as *const u32);
        let flags = *(p.add(4) as *const u32);
        if flags > 3 || flags == 3 {
            bad.push(format!("[{i}] flags {flags:#x} invalid"));
        }
        match ty {
            0 => {
                let res = *(p.add(8) as *const *mut c_void);
                let sub = *(p.add(16) as *const u32);
                let before = *(p.add(20) as *const u32);
                let after = *(p.add(24) as *const u32);
                parts.push(format!("T{{{res:p} sub={sub:#x} {before:#x}->{after:#x}}}"));
                if res.is_null() {
                    bad.push(format!("[{i}] transition with null resource"));
                    continue;
                }
                if before == after {
                    bad.push(format!("[{i}] StateBefore == StateAfter ({before:#x})"));
                }
                for (nm, s) in [("before", before), ("after", after)] {
                    if let Some(e) = state_problem(s) {
                        bad.push(format!("[{i}] {nm}: {e}"));
                    }
                }
                if sub != u32::MAX {
                    if let Some(d) = res_desc(res) {
                        let c = subresources(&d);
                        if sub >= c {
                            bad.push(format!("[{i}] subresource {sub} >= {c} (dim={} mips={} arr={} fmt={})", d.dim, d.mips, d.depth_or_array, d.format));
                        }
                    }
                }
            }
            1 => {
                let a = *(p.add(8) as *const *mut c_void);
                let z = *(p.add(16) as *const *mut c_void);
                parts.push(format!("A{{{a:p}->{z:p}}}"));
                if flags != 0 {
                    bad.push(format!("[{i}] aliasing barrier with split flags {flags:#x}"));
                }
                if !a.is_null() && a == z {
                    bad.push(format!("[{i}] aliasing barrier before == after"));
                }
            }
            2 => {
                let r = *(p.add(8) as *const *mut c_void);
                parts.push(format!("U{{{r:p}}}"));
                if flags != 0 {
                    bad.push(format!("[{i}] UAV barrier with split flags {flags:#x}"));
                }
            }
            t => {
                parts.push(format!("?{t}"));
                bad.push(format!("[{i}] barrier type {t} invalid"));
            }
        }
    }
    let shown = if parts.len() > 6 { format!("{} ... (+{})", parts[..6].join(" "), parts.len() - 6) } else { parts.join(" ") };
    (format!("ResourceBarrier n={n} {shown}"), bad)
}

unsafe fn check_rects(n: u32, rects: *const i32, what: &str, bad: &mut Vec<String>) {
    if n > 0 && rects.is_null() {
        bad.push(format!("{what}: {n} rects but null pointer"));
        return;
    }
    for i in 0..n as usize {
        let r = core::slice::from_raw_parts(rects.add(i * 4), 4);
        if r[0] > r[2] || r[1] > r[3] {
            bad.push(format!("{what}: rect[{i}] {r:?} has left>right or top>bottom"));
        }
    }
}

fn root_check(list: *mut c_void, gfx: bool, idx: u32, want: &[Param], what: &str, extra: Option<(u32, u32)>) -> Vec<String> {
    with(|s| {
        let l = s.lists.entry(list as usize).or_default();
        let rs = if gfx { l.gfx_rs } else { l.cmp_rs };
        let heaps = l.heaps.clone();
        let mut bad = Vec::new();
        let Some(rs) = rs else {
            bad.push(format!("{what} with no {} root signature set", if gfx { "graphics" } else { "compute" }));
            return bad;
        };
        let Some(params) = s.root_sigs.get(&rs) else {
            return bad;
        };
        let Some(p) = params.get(idx as usize) else {
            bad.push(format!("root parameter {idx} >= {} parameters in root signature {rs:#x}", params.len()));
            return bad;
        };
        let ok = want.iter().any(|w| match (w, p) {
            (Param::Constants(_), Param::Constants(_)) => true,
            (a, b) => a == b,
        });
        if !ok {
            bad.push(format!("root parameter {idx} is {p:?}, call needs {want:?}"));
        }
        if let (Param::Constants(n), Some((off, cnt))) = (p, extra) {
            if off + cnt > *n {
                bad.push(format!("constants {off}+{cnt} exceed the parameter's {n} values"));
            }
        }
        if *p == Param::Table && heaps.is_empty() {
            bad.push("descriptor table set before SetDescriptorHeaps".into());
        }
        bad
    })
}

fn table_range(list: *mut c_void, h: u64) -> Vec<String> {
    with(|s| {
        let l = s.lists.entry(list as usize).or_default();
        if l.heap_ranges.is_empty() || l.heap_ranges.iter().any(|&(a, b)| h >= a && h < b) {
            return vec![];
        }
        vec![format!("GPU handle {h:#x} is outside every bound descriptor heap {:x?}", l.heap_ranges)]
    })
}

trait Arg {
    fn arg(self) -> u64;
}
impl Arg for u64 { fn arg(self) -> u64 { self } }
impl Arg for u32 { fn arg(self) -> u64 { self as u64 } }
impl Arg for i32 { fn arg(self) -> u64 { self as u32 as u64 } }
impl Arg for u8 { fn arg(self) -> u64 { self as u64 } }
impl Arg for f32 { fn arg(self) -> u64 { self.to_bits() as u64 } }
impl<T> Arg for *const T { fn arg(self) -> u64 { self as usize as u64 } }
impl<T> Arg for *mut T { fn arg(self) -> u64 { self as usize as u64 } }

struct Rec {
    seq: u64,
    xslot: usize,
    a: [u64; 7],
    bufs: Vec<(usize, Vec<u8>)>,
}

fn buf_spans(xslot: usize, a: &[u64]) -> Vec<(usize, usize)> {
    let n = |i: usize| a.get(i).copied().unwrap_or(0) as usize;
    match xslot {
        16 | 83 => vec![(0, 48), (4, 48), (5, 24)],
        23 => vec![(0, 16)],
        49 => vec![(2, n(1) * 24)],
        21 => vec![(1, n(0) * 24)],
        22 => vec![(1, n(0) * 16)],
        26 => vec![(1, n(0) * 32)],
        32 => vec![(1, n(0) * 8)],
        39 | 40 => vec![(2, n(1) * 4)],
        47 => vec![(0, 16)],
        48 => vec![(2, n(1) * 16)],
        50 => vec![(1, if n(2) as u32 != 0 { 8 } else { n(0) * 8 }), (3, 8)],
        51 => vec![(5, n(4) * 16)],
        52 => vec![(1, 16), (3, n(2) * 16)],
        53 | 54 => vec![(3, 16), (5, n(4) * 16)],
        57 => vec![(1, n(2))],
        _ => vec![],
    }
}

fn rec(list: *mut c_void, xslot: usize, args: &[u64]) {
    if xslot == 9 || xslot == 10 {
        return;
    }
    let mut a = [0u64; 7];
    for (i, v) in args.iter().take(7).enumerate() {
        a[i] = *v;
    }
    let mut bufs = Vec::new();
    for (i, len) in buf_spans(xslot, args) {
        let p = a[i] as *const u8;
        if !p.is_null() && len > 0 && len < 1 << 20 {
            bufs.push((i, unsafe { core::slice::from_raw_parts(p, len) }.to_vec()));
        }
    }
    with(|s| {
        let l = s.lists.entry(list as usize).or_default();
        let seq = l.seq;
        l.recs.push_back(Rec { seq, xslot, a, bufs });
        while l.recs.len() > KEEP_REC {
            l.recs.pop_front();
        }
    });
}

macro_rules! call {
    ($slot:expr, fn($($t:ty),*) $(-> $r:ty)?, $this:expr $(, $a:expr)*) => {{
        rec($this, $slot, &[$(Arg::arg($a)),*]);
        let f: unsafe extern "system" fn($($t),*) $(-> $r)? = core::mem::transmute(next($slot));
        f($this $(, $a)*)
    }};
}

fn pc_slot(x: usize) -> usize {
    match x {
        28..=31 => x + 24,
        32..=55 => x - 4,
        _ => x,
    }
}

const IID_DEVICE: [u8; 16] = guid(0x189819f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
const IID_ALLOC: [u8; 16] = guid(0x6102dee4, 0xaf59, 0x4b09, [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24]);
const IID_LIST: [u8; 16] = guid(0x5b160d0f, 0xac1b, 0x4185, [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55]);

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

unsafe fn vfn(obj: *mut c_void, slot: usize) -> usize {
    *(*(obj as *const *const usize)).add(slot)
}

unsafe fn replay_one(list: *mut c_void, r: &Rec) {
    let mut a = r.a;
    let mut fixed: Vec<Vec<u8>> = Vec::new();
    for (i, b) in &r.bufs {
        a[*i] = b.as_ptr() as u64;
        if r.xslot == 26 && *i == 1 {
            if let Some(v) = crate::xone_barrier::sanitize(r.a[0] as u32, b.as_ptr()) {
                a[0] = (v.len() / 32) as u64;
                fixed.push(v);
                a[1] = fixed.last().unwrap().as_ptr() as u64;
            }
        }
    }
    let f = if r.xslot >= 60 && r.xslot != 68 { next(r.xslot) } else { vfn(list, pc_slot(r.xslot)) };
    if r.xslot == 68 {
        let g: unsafe extern "system" fn(*mut c_void, f32, f32) = core::mem::transmute(vfn(list, 62));
        g(list, f32::from_bits(a[0] as u32), f32::from_bits(a[1] as u32));
    } else if r.xslot == 51 {
        let g: unsafe extern "system" fn(*mut c_void, u64, u32, f32, u8, u32, u64) = core::mem::transmute(f);
        g(list, a[0], a[1] as u32, f32::from_bits(a[2] as u32), a[3] as u8, a[4] as u32, a[5]);
    } else {
        let g: unsafe extern "system" fn(*mut c_void, u64, u64, u64, u64, u64, u64, u64) = core::mem::transmute(f);
        g(list, a[0], a[1], a[2], a[3], a[4], a[5], a[6]);
    }
}

unsafe fn try_alone(dev: *mut c_void, ty: u32, r: &Rec) -> i32 {
    let rel = |o: *mut c_void| {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(vfn(o, 2));
        f(o);
    };
    let ca: unsafe extern "system" fn(*mut c_void, u32, *const u8, *mut *mut c_void) -> i32 = core::mem::transmute(vfn(dev, 9));
    let cl: unsafe extern "system" fn(*mut c_void, u32, u32, *mut c_void, *mut c_void, *const u8, *mut *mut c_void) -> i32 = core::mem::transmute(vfn(dev, 12));
    let mut alloc: *mut c_void = core::ptr::null_mut();
    if ca(dev, ty, IID_ALLOC.as_ptr(), &mut alloc) < 0 || alloc.is_null() {
        return 1;
    }
    let mut scratch: *mut c_void = core::ptr::null_mut();
    if cl(dev, 0, ty, alloc, core::ptr::null_mut(), IID_LIST.as_ptr(), &mut scratch) < 0 || scratch.is_null() {
        rel(alloc);
        return 1;
    }
    replay_one(scratch, r);
    let close: unsafe extern "system" fn(*mut c_void) -> i32 = core::mem::transmute(vfn(scratch, 9));
    let hr = close(scratch);
    rel(scratch);
    rel(alloc);
    hr
}

unsafe fn barrier_pass(list: *mut c_void, ty: u32, recs: &VecDeque<Rec>) -> String {
    let mut dev: *mut c_void = core::ptr::null_mut();
    let gd: unsafe extern "system" fn(*mut c_void, *const u8, *mut *mut c_void) -> i32 = core::mem::transmute(vfn(list, 7));
    if gd(list, IID_DEVICE.as_ptr(), &mut dev) < 0 || dev.is_null() {
        return "barrier check: GetDevice failed".into();
    }
    let mut out = String::new();
    let mut tested = 0;
    for r in recs.iter().filter(|r| r.xslot == 26) {
        tested += 1;
        if try_alone(dev, ty, r) >= 0 {
            continue;
        }
        out.push_str(&format!(" #{} n={} fails alone:", r.seq, r.a[0]));
        if let Some((_, b)) = r.bufs.iter().find(|(i, _)| *i == 1) {
            for (j, one) in b.chunks_exact(32).enumerate() {
                let single = Rec { seq: r.seq, xslot: 26, a: [1, 0, 0, 0, 0, 0, 0], bufs: vec![(1, one.to_vec())] };
                if try_alone(dev, ty, &single) < 0 {
                    let w = |o: usize| u64::from_le_bytes(one[o..o + 8].try_into().unwrap());
                    let ty0 = u32::from_le_bytes(one[0..4].try_into().unwrap());
                    let fl = u32::from_le_bytes(one[4..8].try_into().unwrap());
                    let placed = |p: u64| crate::placed::is_committed_placement(p as *mut c_void);
                    out.push_str(&format!(" [{j}] type={ty0} flags={fl} a={:#x}(placed={}) b={:#x}(placed={}) states={:#x}->{:#x};", w(8), placed(w(8)), w(16), placed(w(16)), u32::from_le_bytes(one[20..24].try_into().unwrap()), u32::from_le_bytes(one[24..28].try_into().unwrap())));
                }
            }
        }
    }
    let rel: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(vfn(dev, 2));
    rel(dev);
    if out.is_empty() {
        format!("barrier check: all {tested} recorded ResourceBarrier calls close fine alone")
    } else {
        format!("barrier check: BAD BARRIER{out}")
    }
}

unsafe fn bisect(list: *mut c_void, ty: u32, recs: &VecDeque<Rec>) -> String {
    let mut dev: *mut c_void = core::ptr::null_mut();
    let gd: unsafe extern "system" fn(*mut c_void, *const u8, *mut *mut c_void) -> i32 = core::mem::transmute(vfn(list, 7));
    if gd(list, IID_DEVICE.as_ptr(), &mut dev) < 0 || dev.is_null() {
        return "replay: GetDevice failed".into();
    }
    let rel = |o: *mut c_void| {
        let r: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(vfn(o, 2));
        r(o);
    };
    let ca: unsafe extern "system" fn(*mut c_void, u32, *const u8, *mut *mut c_void) -> i32 = core::mem::transmute(vfn(dev, 9));
    let cl: unsafe extern "system" fn(*mut c_void, u32, u32, *mut c_void, *mut c_void, *const u8, *mut *mut c_void) -> i32 = core::mem::transmute(vfn(dev, 12));
    let attempt = |cmds: &[&Rec]| -> Result<i32, String> {
        let mut alloc: *mut c_void = core::ptr::null_mut();
        let hr = ca(dev, ty, IID_ALLOC.as_ptr(), &mut alloc);
        if hr < 0 || alloc.is_null() {
            return Err(format!("CreateCommandAllocator hr={hr:#x}"));
        }
        let mut scratch: *mut c_void = core::ptr::null_mut();
        let hr = cl(dev, 0, ty, alloc, core::ptr::null_mut(), IID_LIST.as_ptr(), &mut scratch);
        if hr < 0 || scratch.is_null() {
            rel(alloc);
            return Err(format!("CreateCommandList hr={hr:#x}"));
        }
        for r in cmds {
            replay_one(scratch, r);
        }
        let close: unsafe extern "system" fn(*mut c_void) -> i32 = core::mem::transmute(vfn(scratch, 9));
        let hr = close(scratch);
        rel(scratch);
        rel(alloc);
        Ok(hr)
    };
    let v: Vec<&Rec> = recs.iter().collect();
    let out = match attempt(&v) {
        Err(e) => format!("replay: {e}"),
        Ok(full) if full >= 0 => format!("replay: all {} recorded commands close fine on a fresh list (hr={full:#x}); the failure comes from something not recorded here (initial PSO or a call made below this layer)", v.len()),
        Ok(full) => {
            let mut found = None;
            for k in 1..=v.len() {
                if matches!(attempt(&v[..k]), Ok(h) if h < 0) {
                    found = Some(k);
                    break;
                }
            }
            match found {
                Some(k) => {
                    let r = v[k - 1];
                    let alone = attempt(&[r]).unwrap_or(-1);
                    let mut parts = String::new();
                    if r.xslot == 26 && r.a[0] > 1 {
                        if let Some((_, b)) = r.bufs.iter().find(|(i, _)| *i == 1) {
                            for (j, one) in b.chunks_exact(32).enumerate() {
                                let single = Rec { seq: r.seq, xslot: 26, a: [1, 0, 0, 0, 0, 0, 0], bufs: vec![(1, one.to_vec())] };
                                let h = attempt(&[&single]).unwrap_or(-1);
                                parts.push_str(&format!(" [{j}] type={} alone hr={h:#x};", u32::from_le_bytes(one[0..4].try_into().unwrap())));
                            }
                        }
                    }
                    format!("replay: BAD COMMAND is #{} (Xbox slot {} -> PC slot {}, args {:x?}); a fresh list closes fine with the commands before it and fails once it is added (that command alone: hr={alone:#x}){parts}", r.seq, r.xslot, pc_slot(r.xslot), &r.a[..])
                }
                None => format!("replay: the full replay fails (hr={full:#x}) but no prefix does"),
            }
        }
    };
    rel(dev);
    out
}

unsafe extern "system" fn h_close(this: *mut c_void) -> i32 {
    let open_events = with(|s| s.lists.get(&(this as usize)).map(|l| l.events).unwrap_or(0));
    let open_q = with(|s| s.lists.get(&(this as usize)).map(|l| l.open_queries.len()).unwrap_or(0));
    let mut bad = Vec::new();
    if open_events != 0 {
        bad.push(format!("{open_events} BeginEvent without EndEvent at Close"));
    }
    if open_q != 0 {
        bad.push(format!("{open_q} queries begun but not ended at Close"));
    }
    note(this, "Close".into(), bad);
    let hr = call!(9, fn(*mut c_void) -> i32, this);
    let st = with(|s| s.lists.remove(&(this as usize)));
    if let Some(l) = st {
        if hr < 0 || !l.violations.is_empty() {
            if REPORTS.fetch_add(1, Ordering::Relaxed) < 12 {
                record(M, TAG, &format!("list {this:p} type={:?} Close hr={hr:#010x}: {} violation(s), last {} of {} commands:", l.ty, l.violations.len(), l.log.len(), l.seq));
                for (nm, rs) in [("compute", l.cmp_rs), ("graphics", l.gfx_rs)] {
                    if let Some(rs) = rs {
                        let p = with(|s| (s.root_sigs.get(&rs).cloned(), s.root_sigs.len()));
                        record(M, TAG, &format!("  {nm} root signature {rs:#x}: {:?} ({} registered)", p.0, p.1));
                    }
                }
                for v in &l.violations {
                    record(M, TAG, &format!("  VIOLATION {v}"));
                }
                for c in &l.log {
                    record(M, TAG, &format!("  {c}"));
                }
                if hr < 0 {
                    let complete = l.seq as usize <= KEEP_REC;
                    record(M, TAG, &format!("  {}", barrier_pass(this, l.ty.unwrap_or(0), &l.recs)));
                    if complete {
                        let msg = bisect(this, l.ty.unwrap_or(0), &l.recs);
                        record(M, TAG, &format!("  {msg}"));
                    } else {
                        record(M, TAG, "  replay skipped: the list is longer than the replay record");
                    }
                }
            }
        }
    }
    hr
}

unsafe extern "system" fn h_reset(this: *mut c_void, alloc: *mut c_void, pso: *mut c_void) -> i32 {
    with(|s| {
        s.lists.insert(this as usize, ListState::default());
    });
    note(this, format!("Reset pso={pso:p}"), vec![]);
    call!(10, fn(*mut c_void, *mut c_void, *mut c_void) -> i32, this, alloc, pso)
}

unsafe extern "system" fn h_draw(this: *mut c_void, a: u32, b: u32, c: u32, d: u32) {
    note(this, format!("DrawInstanced {a} {b} {c} {d}"), vec![]);
    call!(12, fn(*mut c_void, u32, u32, u32, u32), this, a, b, c, d)
}

unsafe extern "system" fn h_draw_indexed(this: *mut c_void, a: u32, b: u32, c: u32, d: i32, e: u32) {
    note(this, format!("DrawIndexedInstanced {a} {b} {c} {d} {e}"), vec![]);
    call!(13, fn(*mut c_void, u32, u32, u32, i32, u32), this, a, b, c, d, e)
}

unsafe extern "system" fn h_dispatch(this: *mut c_void, x: u32, y: u32, z: u32) {
    let mut bad = Vec::new();
    if x > 65535 || y > 65535 || z > 65535 {
        bad.push("thread group count above 65535".into());
    }
    note(this, format!("Dispatch {x} {y} {z}"), bad);
    call!(14, fn(*mut c_void, u32, u32, u32), this, x, y, z)
}

unsafe extern "system" fn h_copy_buffer(this: *mut c_void, dst: *mut c_void, doff: u64, src: *mut c_void, soff: u64, n: u64) {
    let mut bad = Vec::new();
    for (nm, r, o) in [("dst", dst, doff), ("src", src, soff)] {
        match res_desc(r) {
            None => bad.push(format!("{nm} is null")),
            Some(d) if d.dim != 1 => bad.push(format!("{nm} is not a buffer (dim {})", d.dim)),
            Some(d) if o.saturating_add(n) > d.width => bad.push(format!("{nm} range {o:#x}+{n:#x} exceeds buffer size {:#x}", d.width)),
            _ => {}
        }
    }
    if !dst.is_null() && dst == src && doff < soff.saturating_add(n) && soff < doff.saturating_add(n) {
        bad.push("overlapping copy within one buffer".into());
    }
    note(this, format!("CopyBufferRegion {dst:p}+{doff:#x} <- {src:p}+{soff:#x} n={n:#x}"), bad);
    call!(15, fn(*mut c_void, *mut c_void, u64, *mut c_void, u64, u64), this, dst, doff, src, soff, n)
}

unsafe fn loc_str(l: *const u8, bad: &mut Vec<String>, nm: &str) -> Option<(ResDesc, Option<u32>, Option<[u32; 4]>)> {
    if l.is_null() {
        bad.push(format!("{nm} location null"));
        return None;
    }
    let res = *(l as *const *mut c_void);
    let ty = *(l.add(8) as *const u32);
    let d = match res_desc(res) {
        Some(d) => d,
        None => {
            bad.push(format!("{nm} resource null"));
            return None;
        }
    };
    if ty == 0 {
        let sub = *(l.add(16) as *const u32);
        if d.dim == 1 {
            bad.push(format!("{nm} SUBRESOURCE_INDEX location on a buffer"));
        } else if sub >= subresources(&d) {
            bad.push(format!("{nm} subresource {sub} >= {}", subresources(&d)));
        }
        Some((d, Some(sub), None))
    } else if ty == 1 {
        let off = *(l.add(16) as *const u64);
        let fmt = *(l.add(24) as *const u32);
        let w = *(l.add(28) as *const u32);
        let h = *(l.add(32) as *const u32);
        let z = *(l.add(36) as *const u32);
        let pitch = *(l.add(40) as *const u32);
        if d.dim != 1 {
            bad.push(format!("{nm} PLACED_FOOTPRINT location on a texture"));
        }
        if off % 512 != 0 {
            bad.push(format!("{nm} footprint offset {off:#x} not 512-aligned"));
        }
        if pitch % 256 != 0 {
            bad.push(format!("{nm} footprint RowPitch {pitch} not 256-aligned"));
        }
        if fmt == 0 {
            bad.push(format!("{nm} footprint format UNKNOWN"));
        }
        if w == 0 || h == 0 || z == 0 {
            bad.push(format!("{nm} footprint has a zero dimension {w}x{h}x{z}"));
        }
        let end = off + pitch as u64 * (h as u64 * z as u64).saturating_sub(1);
        if d.dim == 1 && end > d.width {
            bad.push(format!("{nm} footprint ends at {end:#x} past buffer size {:#x}", d.width));
        }
        Some((d, None, Some([fmt, w, h, z])))
    } else {
        bad.push(format!("{nm} location type {ty} invalid"));
        None
    }
}

unsafe extern "system" fn h_copy_texture(this: *mut c_void, dst: *const u8, x: u32, y: u32, z: u32, src: *const u8, bx: *const u32) {
    let mut bad = Vec::new();
    let dl = loc_str(dst, &mut bad, "dst");
    let sl = loc_str(src, &mut bad, "src");
    let src_ext = match &sl {
        Some((d, Some(sub), _)) => Some(sub_extent(d, *sub)),
        Some((_, None, Some(f))) => Some((f[1] as u64, f[2], f[3])),
        _ => None,
    };
    let (mut cw, mut ch, mut cz) = src_ext.unwrap_or((0, 0, 0));
    if !bx.is_null() {
        let b = core::slice::from_raw_parts(bx, 6);
        if b[0] >= b[3] || b[1] >= b[4] || b[2] >= b[5] {
            bad.push(format!("empty or inverted src box {b:?}"));
        }
        if let Some((w, h, d)) = src_ext {
            if b[3] as u64 > w || b[4] > h || b[5] > d {
                bad.push(format!("src box {b:?} exceeds source extent {w}x{h}x{d}"));
            }
        }
        cw = b[3].saturating_sub(b[0]) as u64;
        ch = b[4].saturating_sub(b[1]);
        cz = b[5].saturating_sub(b[2]);
    }
    let dst_ext = match &dl {
        Some((d, Some(sub), _)) => Some(sub_extent(d, *sub)),
        Some((_, None, Some(f))) => Some((f[1] as u64, f[2], f[3])),
        _ => None,
    };
    if let (Some((w, h, d)), true) = (dst_ext, src_ext.is_some()) {
        if x as u64 + cw > w || y + ch > h || z + cz > d {
            bad.push(format!("copy of {cw}x{ch}x{cz} at ({x},{y},{z}) exceeds destination extent {w}x{h}x{d}"));
        }
    }
    if let (Some((dd, Some(_), _)), Some((sd, Some(_), _))) = (&dl, &sl) {
        if dd.format != sd.format {
            bad.push(format!("texture formats differ: dst {} src {}", dd.format, sd.format));
        }
    }
    note(this, format!("CopyTextureRegion dst={:p} ({x},{y},{z}) src={:p} box={bx:p}", if dst.is_null() { core::ptr::null() } else { *(dst as *const *const c_void) }, if src.is_null() { core::ptr::null() } else { *(src as *const *const c_void) }), bad);
    call!(16, fn(*mut c_void, *const u8, u32, u32, u32, *const u8, *const u32), this, dst, x, y, z, src, bx)
}

unsafe extern "system" fn h_copy_resource(this: *mut c_void, dst: *mut c_void, src: *mut c_void) {
    let mut bad = Vec::new();
    match (res_desc(dst), res_desc(src)) {
        (Some(a), Some(b)) => {
            if a.dim != b.dim || a.width != b.width || a.height != b.height || a.depth_or_array != b.depth_or_array || a.mips != b.mips {
                bad.push(format!("CopyResource shape mismatch dst {a:?} src {b:?}"));
            }
        }
        _ => bad.push("null resource".into()),
    }
    if dst == src {
        bad.push("dst == src".into());
    }
    note(this, format!("CopyResource {dst:p} <- {src:p}"), bad);
    call!(17, fn(*mut c_void, *mut c_void, *mut c_void), this, dst, src)
}

unsafe extern "system" fn h_viewports(this: *mut c_void, n: u32, p: *const f32) {
    let mut bad = Vec::new();
    if n > 16 {
        bad.push(format!("{n} viewports > 16"));
    }
    if n > 0 && p.is_null() {
        bad.push("null viewport array".into());
    }
    note(this, format!("RSSetViewports n={n}"), bad);
    call!(21, fn(*mut c_void, u32, *const f32), this, n, p)
}

unsafe extern "system" fn h_scissors(this: *mut c_void, n: u32, p: *const i32) {
    let mut bad = Vec::new();
    if n > 16 {
        bad.push(format!("{n} scissor rects > 16"));
    }
    check_rects(n, p, "scissor", &mut bad);
    note(this, format!("RSSetScissorRects n={n}"), bad);
    call!(22, fn(*mut c_void, u32, *const i32), this, n, p)
}

unsafe extern "system" fn h_set_pso(this: *mut c_void, pso: *mut c_void) {
    let bad = if pso.is_null() { vec!["null pipeline state".to_string()] } else { vec![] };
    note(this, format!("SetPipelineState {pso:p}"), bad);
    call!(25, fn(*mut c_void, *mut c_void), this, pso)
}

unsafe extern "system" fn h_barrier(this: *mut c_void, n: u32, b: *const u8) {
    let (cmd, bad) = check_barriers(n, b);
    note(this, cmd, bad);
    call!(26, fn(*mut c_void, u32, *const u8), this, n, b)
}

fn query_check(list: *mut c_void, heap: *mut c_void, ty: u32, idx: u32, begin: bool) -> Vec<String> {
    with(|s| {
        let mut bad = Vec::new();
        if let Some(&(hty, count)) = s.query_heaps.get(&(heap as usize)) {
            if idx >= count {
                bad.push(format!("query index {idx} >= heap count {count}"));
            }
            let compatible = match hty {
                0 => ty == 0 || ty == 1,
                1 => ty == 2,
                2 => ty == 3,
                3 => (4..=7).contains(&ty),
                _ => true,
            };
            if !compatible {
                bad.push(format!("query type {ty} does not match heap type {hty}"));
            }
        }
        if begin && ty == 2 {
            bad.push("BeginQuery on a timestamp query".into());
        }
        let l = s.lists.entry(list as usize).or_default();
        let key = (heap as usize, idx);
        if ty != 2 {
            if begin {
                if !l.open_queries.insert(key) {
                    bad.push(format!("query {idx} begun twice without EndQuery"));
                }
                let same_type_open = l.open_queries.len() > 1 && ty <= 1;
                if same_type_open {
                    bad.push("nested occlusion queries".into());
                }
            } else if !l.open_queries.remove(&key) {
                bad.push(format!("EndQuery {idx} without BeginQuery"));
            }
        }
        bad
    })
}

unsafe extern "system" fn h_begin_query(this: *mut c_void, heap: *mut c_void, ty: u32, idx: u32) {
    let bad = query_check(this, heap, ty, idx, true);
    note(this, format!("BeginQuery {heap:p} type={ty} idx={idx}"), bad);
    call!(28, fn(*mut c_void, *mut c_void, u32, u32), this, heap, ty, idx)
}

unsafe extern "system" fn h_end_query(this: *mut c_void, heap: *mut c_void, ty: u32, idx: u32) {
    let bad = query_check(this, heap, ty, idx, false);
    note(this, format!("EndQuery {heap:p} type={ty} idx={idx}"), bad);
    call!(29, fn(*mut c_void, *mut c_void, u32, u32), this, heap, ty, idx)
}

unsafe extern "system" fn h_resolve_query(this: *mut c_void, heap: *mut c_void, ty: u32, start: u32, n: u32, dst: *mut c_void, off: u64) {
    let mut bad = Vec::new();
    if let Some((_, count)) = with(|s| s.query_heaps.get(&(heap as usize)).copied()) {
        if start as u64 + n as u64 > count as u64 {
            bad.push(format!("resolve range {start}+{n} exceeds heap count {count}"));
        }
    }
    if off % 8 != 0 {
        bad.push(format!("destination offset {off:#x} not 8-aligned"));
    }
    let stride: u64 = match ty { 3 => 88, 4..=7 => 32, _ => 8 };
    if let Some(d) = res_desc(dst) {
        if off + stride * n as u64 > d.width {
            bad.push(format!("resolve writes {off:#x}+{} past buffer size {:#x}", stride * n as u64, d.width));
        }
    } else {
        bad.push("null destination buffer".into());
    }
    note(this, format!("ResolveQueryData {heap:p} type={ty} {start}+{n} -> {dst:p}+{off:#x}"), bad);
    call!(30, fn(*mut c_void, *mut c_void, u32, u32, u32, *mut c_void, u64), this, heap, ty, start, n, dst, off)
}

unsafe extern "system" fn h_set_heaps(this: *mut c_void, n: u32, heaps: *const *mut c_void) {
    let mut bad = Vec::new();
    let mut seen = Vec::new();
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    if n > 2 {
        bad.push(format!("{n} descriptor heaps (max 2)"));
    }
    for i in 0..n as usize {
        let h = if heaps.is_null() { core::ptr::null_mut() } else { *heaps.add(i) };
        if h.is_null() {
            bad.push(format!("heap[{i}] null"));
            continue;
        }
        let vt = *(h as *const *const usize);
        let get: unsafe extern "system" fn(*mut c_void, *mut u32) -> *mut u32 = core::mem::transmute(*vt.add(8));
        let mut d = [0u32; 4];
        get(h, d.as_mut_ptr());
        if d[0] > 1 {
            bad.push(format!("heap[{i}] type {} is not CBV_SRV_UAV or SAMPLER", d[0]));
        }
        if d[2] & 1 == 0 {
            bad.push(format!("heap[{i}] is not shader-visible"));
        }
        if seen.iter().any(|&(t, _)| t == d[0]) {
            bad.push(format!("two heaps of type {}", d[0]));
        }
        seen.push((d[0], d[1]));
        let gpu: unsafe extern "system" fn(*mut c_void) -> u64 = core::mem::transmute(*vt.add(10));
        let base = gpu(h);
        let stride = if d[0] == 1 { 32u64 } else { 64u64 };
        ranges.push((base, base + d[1] as u64 * stride));
    }
    with(|s| {
        let l = s.lists.entry(this as usize).or_default();
        l.heaps = seen.clone();
        l.heap_ranges = ranges.clone();
    });
    note(this, format!("SetDescriptorHeaps {seen:?} gpu {:x?}", ranges), bad);
    call!(32, fn(*mut c_void, u32, *const *mut c_void), this, n, heaps)
}

unsafe extern "system" fn h_cmp_rs(this: *mut c_void, rs: *mut c_void) {
    with(|s| s.lists.entry(this as usize).or_default().cmp_rs = (!rs.is_null()).then_some(rs as usize));
    note(this, format!("SetComputeRootSignature {rs:p}"), vec![]);
    call!(33, fn(*mut c_void, *mut c_void), this, rs)
}

unsafe extern "system" fn h_gfx_rs(this: *mut c_void, rs: *mut c_void) {
    with(|s| s.lists.entry(this as usize).or_default().gfx_rs = (!rs.is_null()).then_some(rs as usize));
    note(this, format!("SetGraphicsRootSignature {rs:p}"), vec![]);
    call!(34, fn(*mut c_void, *mut c_void), this, rs)
}

unsafe extern "system" fn h_cmp_table(this: *mut c_void, i: u32, h: u64) {
    let mut bad = root_check(this, false, i, &[Param::Table], "SetComputeRootDescriptorTable", None);
    if h == 0 {
        bad.push("null GPU descriptor handle".into());
    }
    bad.extend(table_range(this, h));
    note(this, format!("SetComputeRootDescriptorTable {i} {h:#x}"), bad);
    call!(35, fn(*mut c_void, u32, u64), this, i, h)
}

unsafe extern "system" fn h_gfx_table(this: *mut c_void, i: u32, h: u64) {
    let mut bad = root_check(this, true, i, &[Param::Table], "SetGraphicsRootDescriptorTable", None);
    if h == 0 {
        bad.push("null GPU descriptor handle".into());
    }
    bad.extend(table_range(this, h));
    note(this, format!("SetGraphicsRootDescriptorTable {i} {h:#x}"), bad);
    call!(36, fn(*mut c_void, u32, u64), this, i, h)
}

unsafe extern "system" fn h_cmp_c1(this: *mut c_void, i: u32, v: u32, off: u32) {
    let bad = root_check(this, false, i, &[Param::Constants(0)], "SetComputeRoot32BitConstant", Some((off, 1)));
    note(this, format!("SetComputeRoot32BitConstant {i} {v:#x} @{off}"), bad);
    call!(37, fn(*mut c_void, u32, u32, u32), this, i, v, off)
}

unsafe extern "system" fn h_gfx_c1(this: *mut c_void, i: u32, v: u32, off: u32) {
    let bad = root_check(this, true, i, &[Param::Constants(0)], "SetGraphicsRoot32BitConstant", Some((off, 1)));
    note(this, format!("SetGraphicsRoot32BitConstant {i} {v:#x} @{off}"), bad);
    call!(38, fn(*mut c_void, u32, u32, u32), this, i, v, off)
}

unsafe extern "system" fn h_cmp_cn(this: *mut c_void, i: u32, n: u32, p: *const c_void, off: u32) {
    let bad = root_check(this, false, i, &[Param::Constants(0)], "SetComputeRoot32BitConstants", Some((off, n)));
    note(this, format!("SetComputeRoot32BitConstants {i} n={n} @{off}"), bad);
    call!(39, fn(*mut c_void, u32, u32, *const c_void, u32), this, i, n, p, off)
}

unsafe extern "system" fn h_gfx_cn(this: *mut c_void, i: u32, n: u32, p: *const c_void, off: u32) {
    let bad = root_check(this, true, i, &[Param::Constants(0)], "SetGraphicsRoot32BitConstants", Some((off, n)));
    note(this, format!("SetGraphicsRoot32BitConstants {i} n={n} @{off}"), bad);
    call!(40, fn(*mut c_void, u32, u32, *const c_void, u32), this, i, n, p, off)
}

macro_rules! root_view {
    ($name:ident, $slot:literal, $gfx:literal, $kind:expr, $label:literal) => {
        unsafe extern "system" fn $name(this: *mut c_void, i: u32, va: u64) {
            let bad = root_check(this, $gfx, i, &[$kind], $label, None);
            note(this, format!(concat!($label, " {} {:#x}"), i, va), bad);
            call!($slot, fn(*mut c_void, u32, u64), this, i, va)
        }
    };
}
root_view!(h_cmp_cbv, 41, false, Param::Cbv, "SetComputeRootConstantBufferView");
root_view!(h_gfx_cbv, 42, true, Param::Cbv, "SetGraphicsRootConstantBufferView");
root_view!(h_cmp_srv, 43, false, Param::Srv, "SetComputeRootShaderResourceView");
root_view!(h_gfx_srv, 44, true, Param::Srv, "SetGraphicsRootShaderResourceView");
root_view!(h_cmp_uav, 45, false, Param::Uav, "SetComputeRootUnorderedAccessView");
root_view!(h_gfx_uav, 46, true, Param::Uav, "SetGraphicsRootUnorderedAccessView");

unsafe extern "system" fn h_ib(this: *mut c_void, v: *const u8) {
    let mut bad = Vec::new();
    if !v.is_null() {
        let fmt = *(v.add(12) as *const u32);
        if fmt != 42 && fmt != 57 && fmt != 0 {
            bad.push(format!("index format {fmt} is not R16_UINT/R32_UINT"));
        }
    }
    note(this, "IASetIndexBuffer".into(), bad);
    call!(47, fn(*mut c_void, *const u8), this, v)
}

unsafe extern "system" fn h_vbs(this: *mut c_void, start: u32, n: u32, v: *const u8) {
    let mut bad = Vec::new();
    if start as u64 + n as u64 > 32 {
        bad.push(format!("vertex buffer slots {start}+{n} exceed 32"));
    }
    note(this, format!("IASetVertexBuffers {start}+{n}"), bad);
    call!(48, fn(*mut c_void, u32, u32, *const u8), this, start, n, v)
}

unsafe extern "system" fn h_targets(this: *mut c_void, n: u32, rtv: *const u64, single: i32, dsv: *const u64) {
    let mut bad = Vec::new();
    if n > 8 {
        bad.push(format!("{n} render targets > 8"));
    }
    if n > 0 && rtv.is_null() {
        bad.push("null render-target handle array".into());
    }
    note(this, format!("OMSetRenderTargets n={n} single={single} dsv={}", !dsv.is_null()), bad);
    call!(50, fn(*mut c_void, u32, *const u64, i32, *const u64), this, n, rtv, single, dsv)
}

unsafe extern "system" fn h_clear_dsv(this: *mut c_void, h: u64, flags: u32, d: f32, st: u8, n: u32, r: *const i32) {
    let mut bad = Vec::new();
    if flags == 0 || flags > 3 {
        bad.push(format!("clear flags {flags:#x} invalid"));
    }
    if !(0.0..=1.0).contains(&d) {
        bad.push(format!("depth clear value {d} outside [0,1]"));
    }
    check_rects(n, r, "ClearDepthStencilView", &mut bad);
    note(this, format!("ClearDepthStencilView {h:#x} flags={flags} d={d} s={st} rects={n}"), bad);
    call!(51, fn(*mut c_void, u64, u32, f32, u8, u32, *const i32), this, h, flags, d, st, n, r)
}

unsafe extern "system" fn h_clear_rtv(this: *mut c_void, h: u64, c: *const f32, n: u32, r: *const i32) {
    let mut bad = Vec::new();
    if c.is_null() {
        bad.push("null clear colour".into());
    }
    check_rects(n, r, "ClearRenderTargetView", &mut bad);
    note(this, format!("ClearRenderTargetView {h:#x} rects={n}"), bad);
    call!(52, fn(*mut c_void, u64, *const f32, u32, *const i32), this, h, c, n, r)
}

unsafe extern "system" fn h_clear_uav_u(this: *mut c_void, g: u64, c: u64, res: *mut c_void, v: *const u32, n: u32, r: *const i32) {
    let mut bad = Vec::new();
    if res.is_null() || g == 0 || c == 0 {
        bad.push("null resource or descriptor handle".into());
    }
    check_rects(n, r, "ClearUnorderedAccessViewUint", &mut bad);
    note(this, format!("ClearUnorderedAccessViewUint gpu={g:#x} cpu={c:#x} {res:p} rects={n}"), bad);
    call!(53, fn(*mut c_void, u64, u64, *mut c_void, *const u32, u32, *const i32), this, g, c, res, v, n, r)
}

unsafe extern "system" fn h_clear_uav_f(this: *mut c_void, g: u64, c: u64, res: *mut c_void, v: *const f32, n: u32, r: *const i32) {
    let mut bad = Vec::new();
    if res.is_null() || g == 0 || c == 0 {
        bad.push("null resource or descriptor handle".into());
    }
    check_rects(n, r, "ClearUnorderedAccessViewFloat", &mut bad);
    note(this, format!("ClearUnorderedAccessViewFloat gpu={g:#x} cpu={c:#x} {res:p} rects={n}"), bad);
    call!(54, fn(*mut c_void, u64, u64, *mut c_void, *const f32, u32, *const i32), this, g, c, res, v, n, r)
}

unsafe extern "system" fn h_begin_event(this: *mut c_void, meta: u32, data: *const c_void, size: u32) {
    with(|s| s.lists.entry(this as usize).or_default().events += 1);
    note(this, format!("BeginEvent meta={meta} size={size}"), vec![]);
    call!(57, fn(*mut c_void, u32, *const c_void, u32), this, meta, data, size)
}

unsafe extern "system" fn h_end_event(this: *mut c_void) {
    let depth = with(|s| {
        let l = s.lists.entry(this as usize).or_default();
        l.events -= 1;
        l.events
    });
    let bad = if depth < 0 { vec!["EndEvent without BeginEvent".to_string()] } else { vec![] };
    note(this, "EndEvent".into(), bad);
    call!(58, fn(*mut c_void), this)
}

unsafe extern "system" fn h_indirect(this: *mut c_void, sig: *mut c_void, max: u32, args: *mut c_void, aoff: u64, cnt: *mut c_void, coff: u64) {
    let mut bad = Vec::new();
    if sig.is_null() || args.is_null() {
        bad.push("null command signature or argument buffer".into());
    }
    if aoff % 4 != 0 || coff % 4 != 0 {
        bad.push("argument/count offset not 4-aligned".into());
    }
    note(this, format!("ExecuteIndirect sig={sig:p} max={max} args={args:p}+{aoff:#x} count={cnt:p}+{coff:#x}"), bad);
    call!(59, fn(*mut c_void, *mut c_void, u32, *mut c_void, u64, *mut c_void, u64), this, sig, max, args, aoff, cnt, coff)
}

unsafe extern "system" fn h_depth_bounds(this: *mut c_void, lo: f32, hi: f32) {
    let mut bad = Vec::new();
    if !(0.0..=1.0).contains(&lo) || !(0.0..=1.0).contains(&hi) {
        bad.push(format!("depth bounds {lo}..{hi} outside [0,1]"));
    }
    if lo > hi {
        bad.push(format!("depth bounds min {lo} > max {hi}"));
    }
    note(this, format!("OMSetDepthBounds {lo} {hi}"), bad);
    call!(68, fn(*mut c_void, f32, f32), this, lo, hi)
}

macro_rules! passthrough {
    ($(($name:ident, $slot:literal, $label:literal)),* $(,)?) => {
        $(
            unsafe extern "system" fn $name(this: *mut c_void, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64) {
                note(this, format!(concat!($label, " {:#x} {:#x} {:#x} {:#x}"), a, b, c, d), vec![]);
                call!($slot, fn(*mut c_void, u64, u64, u64, u64, u64, u64, u64), this, a, b, c, d, e, f, g)
            }
        )*
        fn passthrough_hooks() -> Vec<(usize, usize)> { vec![$(($slot, $name as usize)),*] }
    };
}
passthrough!(
    (p18, 18, "CopyTiles"), (p19, 19, "ResolveSubresource"), (p20, 20, "IASetPrimitiveTopology"),
    (p23, 23, "OMSetBlendFactor"), (p24, 24, "OMSetStencilRef"), (p27, 27, "ExecuteBundle"),
    (p31, 31, "SetPredication"), (p49, 49, "SOSetTargets"), (p55, 55, "DiscardResource"), (p56, 56, "SetMarker"),
    (p67, 67, "FlushTextureCacheX"), (p83, 83, "CopyTextureRegionX"), (p84, 84, "CopyResourceX"),
    (p11, 11, "ClearState"), (p60, 60, "XboxSlot60"), (p61, 61, "XboxSlot61"), (p62, 62, "XboxSlot62"), (p63, 63, "XboxSlot63"), (p64, 64, "XboxSlot64"), (p65, 65, "XboxSlot65"), (p66, 66, "XboxSlot66"), (p69, 69, "XboxSlot69"), (p70, 70, "XboxSlot70"), (p71, 71, "XboxSlot71"), (p72, 72, "XboxSlot72"), (p73, 73, "XboxSlot73"), (p74, 74, "XboxSlot74"), (p75, 75, "XboxSlot75"), (p76, 76, "XboxSlot76"), (p77, 77, "XboxSlot77"), (p78, 78, "XboxSlot78"), (p79, 79, "XboxSlot79"), (p80, 80, "XboxSlot80"), (p81, 81, "XboxSlot81"), (p82, 82, "XboxSlot82"), (p85, 85, "XboxSlot85"), (p86, 86, "XboxSlot86"), (p87, 87, "XboxSlot87"), (p88, 88, "XboxSlot88"), (p89, 89, "XboxSlot89"), (p90, 90, "XboxSlot90"), (p91, 91, "XboxSlot91"), (p92, 92, "XboxSlot92"), (p93, 93, "XboxSlot93"), (p94, 94, "XboxSlot94"), (p95, 95, "XboxSlot95"), (p96, 96, "XboxSlot96"), (p97, 97, "XboxSlot97"), (p98, 98, "XboxSlot98"), (p99, 99, "XboxSlot99"), (p100, 100, "XboxSlot100"), (p101, 101, "XboxSlot101"), (p102, 102, "XboxSlot102"), (p103, 103, "XboxSlot103"), (p104, 104, "XboxSlot104"), (p105, 105, "XboxSlot105"), (p106, 106, "XboxSlot106"), (p107, 107, "XboxSlot107"), (p108, 108, "XboxSlot108"), (p109, 109, "XboxSlot109"), (p110, 110, "XboxSlot110"), (p111, 111, "XboxSlot111"), (p112, 112, "XboxSlot112"), (p113, 113, "XboxSlot113"), (p114, 114, "XboxSlot114"), (p115, 115, "XboxSlot115"), (p116, 116, "XboxSlot116"), (p117, 117, "XboxSlot117"), (p118, 118, "XboxSlot118"), (p119, 119, "XboxSlot119"), (p120, 120, "XboxSlot120"), (p121, 121, "XboxSlot121"), (p122, 122, "XboxSlot122"), (p123, 123, "XboxSlot123"), (p124, 124, "XboxSlot124"), (p125, 125, "XboxSlot125"), (p126, 126, "XboxSlot126"), (p127, 127, "XboxSlot127"),
);

pub unsafe fn install() {
    if !enabled() {
        return;
    }
    let hooks: &[(usize, usize)] = &[
        (9, h_close as usize), (10, h_reset as usize), (12, h_draw as usize), (13, h_draw_indexed as usize),
        (14, h_dispatch as usize), (15, h_copy_buffer as usize), (16, h_copy_texture as usize), (17, h_copy_resource as usize),
        (21, h_viewports as usize), (22, h_scissors as usize), (25, h_set_pso as usize), (26, h_barrier as usize),
        (28, h_begin_query as usize), (29, h_end_query as usize), (30, h_resolve_query as usize),
        (32, h_set_heaps as usize), (33, h_cmp_rs as usize), (34, h_gfx_rs as usize),
        (35, h_cmp_table as usize), (36, h_gfx_table as usize), (37, h_cmp_c1 as usize), (38, h_gfx_c1 as usize),
        (39, h_cmp_cn as usize), (40, h_gfx_cn as usize), (41, h_cmp_cbv as usize), (42, h_gfx_cbv as usize),
        (43, h_cmp_srv as usize), (44, h_gfx_srv as usize), (45, h_cmp_uav as usize), (46, h_gfx_uav as usize),
        (47, h_ib as usize), (48, h_vbs as usize), (50, h_targets as usize), (51, h_clear_dsv as usize),
        (52, h_clear_rtv as usize), (53, h_clear_uav_u as usize), (54, h_clear_uav_f as usize),
        (57, h_begin_event as usize), (58, h_end_event as usize), (59, h_indirect as usize),
    ];
    let mut all = hooks.to_vec();
    all.push((68, h_depth_bounds as usize));
    all.extend(passthrough_hooks());
    let mut n = 0;
    for &(slot, f) in &all {
        let prev = crate::cmdlist_wrap::vt_get(slot);
        if prev == 0 {
            record(M, TAG, &format!("slot {slot} empty, not validated"));
            continue;
        }
        NEXT[slot].store(prev, Ordering::Relaxed);
        crate::cmdlist_wrap::vt_set(slot, f);
        n += 1;
    }
    record(M, TAG, &format!("command-list validation on: {n} commands checked, last {KEEP} per list dumped on a failed Close"));
}

fn parse_root_sig(blob: &[u8]) -> Option<Vec<Param>> {
    let body = if blob.get(0..4) == Some(b"DXBC") {
        let c = tombox_xshader::Dxbc::parse(blob).ok()?;
        let ch = c.chunk(b"RTS0")?;
        blob.get(ch.offset..ch.offset + ch.size)?
    } else {
        blob
    };
    let rs = tombox_xshader::meta::parse_rts0(body).ok()?;
    Some(
        rs.params
            .iter()
            .map(|p| match &p.kind {
                tombox_xshader::meta::RootParamKind::Table(_) => Param::Table,
                tombox_xshader::meta::RootParamKind::Constants { num32, .. } => Param::Constants(*num32),
                tombox_xshader::meta::RootParamKind::Descriptor { kind: 2, .. } => Param::Cbv,
                tombox_xshader::meta::RootParamKind::Descriptor { kind: 3, .. } => Param::Srv,
                tombox_xshader::meta::RootParamKind::Descriptor { .. } => Param::Uav,
            })
            .collect(),
    )
}

unsafe extern "system" fn create_root_signature(this: *mut c_void, mask: u32, blob: *const u8, len: usize, riid: *const c_void, ppv: *mut *mut c_void) -> i32 {
    let f: unsafe extern "system" fn(*mut c_void, u32, *const u8, usize, *const c_void, *mut *mut c_void) -> i32 =
        core::mem::transmute(DEV_RS.load(Ordering::Relaxed));
    let hr = f(this, mask, blob, len, riid, ppv);
    if hr >= 0 && !ppv.is_null() && !(*ppv).is_null() && !blob.is_null() {
        let b = core::slice::from_raw_parts(blob, len);
        match parse_root_sig(b) {
            Some(p) => with(|s| {
                s.root_sigs.insert(*ppv as usize, p);
            }),
            None => record(M, TAG, &format!("root signature {:p}: blob ({len} bytes) not parsed; its parameters are not checked", *ppv)),
        }
    }
    hr
}

unsafe extern "system" fn create_query_heap(this: *mut c_void, desc: *const u32, riid: *const c_void, ppv: *mut *mut c_void) -> i32 {
    let f: unsafe extern "system" fn(*mut c_void, *const u32, *const c_void, *mut *mut c_void) -> i32 =
        core::mem::transmute(DEV_QH.load(Ordering::Relaxed));
    let hr = f(this, desc, riid, ppv);
    if hr >= 0 && !desc.is_null() && !ppv.is_null() && !(*ppv).is_null() {
        let (ty, count) = (*desc, *desc.add(1));
        with(|s| {
            s.query_heaps.insert(*ppv as usize, (ty, count));
        });
    }
    hr
}

pub fn hook_device(v: &mut [usize]) {
    if !enabled() || v.len() <= 32 {
        return;
    }
    DEV_RS.store(v[16], Ordering::Relaxed);
    v[16] = create_root_signature as usize;
    DEV_QH.store(v[32], Ordering::Relaxed);
    v[32] = create_query_heap as usize;
}
