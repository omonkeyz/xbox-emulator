use core::ffi::c_void;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use crate::cap::{kv, num, table_sizes, unhex, Capture, Slot};
use crate::d3d::{self, Gpu, COMMON, COPY_DEST, COPY_SOURCE, GENERIC_READ};
use crate::pso_serial;
use crate::stats::{max_abs_diff, Image};
use crate::vcall;

pub struct Opts {
    pub stats: Vec<u64>,
    pub stats_all: bool,
    pub range: Option<(usize, usize)>,
    pub bmp: bool,
    pub out: PathBuf,
    pub retranslate: bool,
    pub hlsl_override: Option<PathBuf>,
    pub pso_patch: Vec<(Option<u64>, usize, u32)>,
    pub no_predication: bool,
    pub verbose: bool,
}

struct Res {
    obj: *mut c_void,
    desc: [u8; 0x38],
    buffer: bool,
    size: u64,
    subs: u32,
    actual: Vec<u32>,
    logical: Vec<u32>,
}

struct Heap {
    obj: *mut c_void,
    cpu: usize,
    gpu: u64,
    inc: usize,
    mirror: Option<(*mut c_void, usize)>,
    sampler: bool,
    made: HashSet<u64>,
}

struct Pso {
    obj: *mut c_void,
    compute: bool,
    hashes: Vec<(String, u64)>,
    ds_write: bool,
}

#[derive(Default, Clone)]
struct ListState {
    pso: u64,
    groot: u64,
    croot: u64,
    gtables: BTreeMap<u32, u64>,
    ctables: BTreeMap<u32, u64>,
    gviews: BTreeMap<u32, (char, u64)>,
    cviews: BTreeMap<u32, (char, u64)>,
    rtvs: Vec<u64>,
    dsv: u64,
    lines: BTreeMap<String, String>,
}

enum RbKind {
    Buf { size: u64 },
    Tex { fps: Vec<[u8; 32]> },
    Query { heap_type: u32, count: u32 },
}

struct Readback {
    label: String,
    buf: *mut c_void,
    kind: RbKind,
    compare: Option<PathBuf>,
    bmp: Option<PathBuf>,
}

pub struct Engine {
    pub gpu: Gpu,
    pub cap: Capture,
    opts: Opts,
    res: HashMap<u64, Res>,
    missing: HashSet<u64>,
    roots: HashMap<u64, *mut c_void>,
    psos: HashMap<u64, Option<Pso>>,
    heaps: HashMap<usize, Heap>,
    rtv: (*mut c_void, usize, usize),
    dsv: (*mut c_void, usize, usize),
    views: HashMap<u64, usize>,
    sigs: HashMap<u64, *mut c_void>,
    qheaps: HashMap<(u64, u32), *mut c_void>,
    qmax: HashMap<(u64, u32), u32>,
    initial: HashMap<u64, u32>,
    own_q: Option<(*mut c_void, *mut c_void, u32)>,
    keep: Vec<*mut c_void>,
    rbs: Vec<Readback>,
    st: ListState,
    dry: bool,
    pub report: String,
    calls: usize,
    warned: HashSet<String>,
}

fn qtype_heap(t: u32) -> u32 {
    match t {
        0 | 1 => 0,
        2 => 1,
        3 => 2,
        _ => 3,
    }
}

fn planes(fmt: u32) -> u32 {
    if matches!(fmt, 19 | 20 | 21 | 22 | 44 | 45 | 46 | 47) { 2 } else { 1 }
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

pub fn pso_field(name: &str) -> Option<usize> {
    Some(match name {
        "depthenable" => 0x1f0,
        "depthwrite" => 0x1f4,
        "depthfunc" => 0x1f8,
        "stencilenable" => 0x1fc,
        "fill" => 0x1c4,
        "cull" => 0x1c8,
        "frontccw" => 0x1cc,
        "depthbias" => 0x1d0,
        "depthclip" => 0x1dc,
        "rtcount" => 0x240,
        "samplemask" => 0x1c0,
        x if x.starts_with("0x") => usize::from_str_radix(&x[2..], 16).ok()?,
        _ => return None,
    })
}

impl Engine {
    pub fn new(gpu: Gpu, cap: Capture, opts: Opts) -> Engine {
        let mut e = Engine {
            gpu,
            cap,
            opts,
            res: HashMap::new(),
            missing: HashSet::new(),
            roots: HashMap::new(),
            psos: HashMap::new(),
            heaps: HashMap::new(),
            rtv: (core::ptr::null_mut(), 0, 0),
            dsv: (core::ptr::null_mut(), 0, 0),
            views: HashMap::new(),
            sigs: HashMap::new(),
            qheaps: HashMap::new(),
            qmax: HashMap::new(),
            initial: HashMap::new(),
            own_q: None,
            keep: Vec::new(),
            rbs: Vec::new(),
            st: ListState::default(),
            dry: false,
            report: String::new(),
            calls: 0,
            warned: HashSet::new(),
        };
        e.prescan();
        e
    }

    fn warn(&mut self, s: String) {
        if self.warned.insert(s.clone()) {
            let _ = writeln!(self.report, "  warning: {s}");
        }
    }

    fn prescan(&mut self) {
        for ex in &self.cap.executes {
            for l in &ex.lines {
                let w: Vec<&str> = l.text.split_whitespace().collect();
                match w.first() {
                    Some(&"barrier") if w.len() >= 3 => {
                        let b = unhex(w[2]);
                        for c in b.chunks_exact(32) {
                            if le32(c, 0) == 0 {
                                let r = le64(c, 8);
                                self.initial.entry(r).or_insert(le32(c, 20));
                            }
                        }
                    }
                    Some(&"beginquery") | Some(&"endquery") if w.len() >= 4 => {
                        let h = kv(&w, "heap").map(num).unwrap_or(0);
                        let t = qtype_heap(num(w[2]) as u32);
                        let i = num(w[3]) as u32;
                        let m = self.qmax.entry((h, t)).or_insert(0);
                        *m = (*m).max(i + 1);
                    }
                    Some(&"resolvequery") if w.len() >= 5 => {
                        let h = kv(&w, "heap").map(num).unwrap_or(0);
                        let t = qtype_heap(num(w[2]) as u32);
                        let end = (num(w[3]) + num(w[4])) as u32;
                        let m = self.qmax.entry((h, t)).or_insert(0);
                        *m = (*m).max(end);
                    }
                    _ => {}
                }
            }
        }
    }

    unsafe fn list(&self) -> *mut c_void {
        self.gpu.list
    }

    unsafe fn ensure_res(&mut self, game: u64) -> Option<*mut c_void> {
        if game == 0 {
            return None;
        }
        if let Some(r) = self.res.get(&game) {
            return Some(r.obj);
        }
        if self.missing.contains(&game) {
            return None;
        }
        let (desc, data, heap, end_state) = if let Some(r) = self.cap.resources.get(&game) {
            (r.desc, r.data.clone(), 1u32, r.state)
        } else if let Some(b) = self.cap.buffer_by_res(game) {
            let mut d = [0u8; 0x38];
            d[0..4].copy_from_slice(&1u32.to_le_bytes());
            d[0x10..0x18].copy_from_slice(&b.size.to_le_bytes());
            d[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
            d[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
            d[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
            d[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
            d[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
            (d, b.data.clone(), b.heap, b.state)
        } else {
            self.missing.insert(game);
            self.warn(format!("resource {game:#x} is referenced but was not recorded"));
            return None;
        };
        let mut cd = desc;
        let dim = le32(&cd, 0);
        let buffer = dim == 1;
        cd[8..16].fill(0);
        if buffer {
            cd[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
            cd[0x20..0x24].fill(0);
            let f = le32(&cd, 0x30) & 0x4;
            cd[0x30..0x34].copy_from_slice(&f.to_le_bytes());
        } else {
            cd[0x2c..0x30].fill(0);
            let f = le32(&cd, 0x30) & 0x2f;
            cd[0x30..0x34].copy_from_slice(&f.to_le_bytes());
        }
        let obj = match self.gpu.committed(&cd, d3d::default_heap(), COMMON) {
            Ok(o) => o,
            Err(e) => {
                self.missing.insert(game);
                self.warn(format!("resource {game:#x}: {e}"));
                return None;
            }
        };
        let mips = u16::from_le_bytes([cd[0x1e], cd[0x1f]]).max(1) as u32;
        let arr = if dim == 4 || buffer { 1 } else { u16::from_le_bytes([cd[0x1c], cd[0x1d]]).max(1) as u32 };
        let subs = if buffer { 1 } else { mips * arr * planes(le32(&cd, 0x20)) };
        let init = self.initial.get(&game).copied().or(end_state).unwrap_or(if heap == 2 { GENERIC_READ } else if heap == 3 { COPY_DEST } else { COMMON });
        let size = le64(&cd, 0x10);
        self.res.insert(game, Res { obj, desc: cd, buffer, size, subs, actual: vec![COMMON; subs as usize], logical: vec![init; subs as usize] });
        if let Some(p) = data {
            match std::fs::read(&p) {
                Ok(b) => self.upload(game, &b, 0),
                Err(e) => self.warn(format!("{}: {e}", p.display())),
            }
        }
        Some(obj)
    }

    unsafe fn transition(&mut self, game: u64, sub: Option<u32>, to: u32) {
        let list = self.list();
        let Some(r) = self.res.get_mut(&game) else { return };
        match sub {
            Some(s) if (s as usize) < r.actual.len() => {
                if r.actual[s as usize] != to {
                    let b = [0u64, r.obj as u64, s as u64 | ((r.actual[s as usize] as u64) << 32), to as u64];
                    vcall!(list, 26, fn(u32, *const u64) -> (), 1, b.as_ptr());
                    r.actual[s as usize] = to;
                }
            }
            _ => {
                if r.actual.iter().all(|&a| a == r.actual[0]) {
                    if r.actual[0] != to {
                        let b = [0u64, r.obj as u64, 0xffff_ffffu64 | ((r.actual[0] as u64) << 32), to as u64];
                        vcall!(list, 26, fn(u32, *const u64) -> (), 1, b.as_ptr());
                    }
                } else {
                    for (i, a) in r.actual.iter().enumerate() {
                        if *a != to {
                            let b = [0u64, r.obj as u64, i as u64 | ((*a as u64) << 32), to as u64];
                            vcall!(list, 26, fn(u32, *const u64) -> (), 1, b.as_ptr());
                        }
                    }
                }
                r.actual.iter_mut().for_each(|a| *a = to);
            }
        }
    }

    unsafe fn sync(&mut self, game: u64) {
        let Some(r) = self.res.get(&game) else { return };
        if r.actual == r.logical {
            return;
        }
        let logical = r.logical.clone();
        if logical.iter().all(|&l| l == logical[0]) {
            self.transition(game, None, logical[0]);
        } else {
            for (i, l) in logical.into_iter().enumerate() {
                self.transition(game, Some(i as u32), l);
            }
        }
    }

    unsafe fn use_res(&mut self, game: u64) -> Option<*mut c_void> {
        let o = self.ensure_res(game)?;
        self.sync(game);
        Some(o)
    }

    unsafe fn upload(&mut self, game: u64, data: &[u8], off: u64) {
        let Some(r) = self.res.get(&game) else { return };
        let (buffer, obj, desc, size) = (r.buffer, r.obj, r.desc, r.size);
        let (payload, fps): (&[u8], Option<Vec<[u8; 32]>>) = if data.len() >= 8 && &data[0..4] == b"TBTX" {
            let n = le32(data, 4) as usize;
            let head = 8 + n * 32;
            if data.len() < head {
                return;
            }
            let fps = (0..n).map(|i| data[8 + i * 32..8 + i * 32 + 32].try_into().unwrap()).collect();
            (&data[head..], Some(fps))
        } else {
            (data, None)
        };
        if payload.is_empty() {
            return;
        }
        let up = match self.gpu.buffer(payload.len() as u64, 2, GENERIC_READ, false) {
            Ok(u) => u,
            Err(e) => {
                self.warn(format!("upload buffer: {e}"));
                return;
            }
        };
        Gpu::map_write(up, payload);
        self.keep.push(up);
        self.transition(game, None, COPY_DEST);
        let list = self.list();
        if buffer {
            let n = (payload.len() as u64).min(size.saturating_sub(off));
            vcall!(list, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), obj, off, up, 0, n);
        } else if let Some(fps) = fps {
            let (ours, _) = self.gpu.footprints(&desc, fps.len() as u32);
            for (i, fp) in fps.iter().enumerate() {
                if ours.get(i).map(|o| o[8..32] != fp[8..32]).unwrap_or(true) {
                    self.warn(format!("resource {game:#x} subresource {i}: recorded footprint differs from this device's"));
                }
                let mut src = [0u8; 48];
                src[0..8].copy_from_slice(&(up as u64).to_le_bytes());
                src[8..12].copy_from_slice(&1u32.to_le_bytes());
                src[16..48].copy_from_slice(fp);
                let mut dst = [0u8; 48];
                dst[0..8].copy_from_slice(&(obj as u64).to_le_bytes());
                dst[16..20].copy_from_slice(&(i as u32).to_le_bytes());
                let end = le64(fp, 0) + le32(fp, 24) as u64 * le32(fp, 16) as u64 * le32(fp, 20) as u64;
                if end as usize > payload.len() + 4096 {
                    continue;
                }
                vcall!(list, 16, fn(*const u8, u32, u32, u32, *const u8, *const c_void) -> (), dst.as_ptr(), 0, 0, 0, src.as_ptr(), core::ptr::null());
            }
        }
    }

    unsafe fn va(&mut self, va: u64) -> u64 {
        if va == 0 {
            return 0;
        }
        let Some(b) = self.cap.buffer_of(va).cloned() else {
            self.warn(format!("GPU address {va:#x} is not inside a recorded buffer"));
            return 0;
        };
        match self.use_res(b.res) {
            Some(o) => Gpu::gpu_va(o) + (va - b.va),
            None => 0,
        }
    }

    unsafe fn heap(&mut self, i: usize) -> Option<(usize, u64, usize, bool)> {
        if let Some(h) = self.heaps.get(&i) {
            return Some((h.cpu, h.gpu, h.inc, h.sampler));
        }
        let sampler = self.cap.is_sampler_heap(i);
        let count = self.cap.heaps[i].count.min(if sampler { 2048 } else { 1_000_000 }) as u32;
        let ty = if sampler { 1 } else { 0 };
        let (obj, cpu, gpu, inc) = match self.gpu.heap(ty, count, true) {
            Ok(h) => h,
            Err(e) => {
                self.warn(e);
                return None;
            }
        };
        let mirror = if sampler { None } else { self.gpu.heap(0, count, false).ok().map(|m| (m.0, m.1)) };
        self.heaps.insert(i, Heap { obj, cpu, gpu, inc, mirror, sampler, made: HashSet::new() });
        Some((cpu, gpu, inc, sampler))
    }

    unsafe fn make_slot(&mut self, hi: usize, slot: u64) {
        if self.heap(hi).is_none() {
            return;
        }
        if self.heaps[&hi].made.contains(&slot) {
            return;
        }
        self.heaps.get_mut(&hi).unwrap().made.insert(slot);
        let Some(s) = self.cap.heaps[hi].slots.get(&slot).cloned() else { return };
        let (cpu, inc, mirror) = {
            let h = &self.heaps[&hi];
            (h.cpu, h.inc, h.mirror)
        };
        let mut targets = vec![cpu + slot as usize * inc];
        if let Some((_, m)) = mirror {
            targets.push(m + slot as usize * inc);
        }
        let dev = self.gpu.device;
        match s {
            Slot::Cbv { va, size } => {
                let v = self.va(va);
                let d = (v, size);
                let desc: [u64; 2] = [d.0, d.1 as u64];
                for t in targets {
                    vcall!(dev, 17, fn(*const u64, usize) -> (), desc.as_ptr(), t);
                }
            }
            Slot::Srv { res, desc } => {
                let o = self.ensure_res(res).unwrap_or(core::ptr::null_mut());
                if o.is_null() && desc.is_none() {
                    return;
                }
                let dp = desc.as_ref().map(|d| d.as_ptr()).unwrap_or(core::ptr::null());
                for t in targets {
                    vcall!(dev, 18, fn(*mut c_void, *const u8, usize) -> (), o, dp, t);
                }
            }
            Slot::Uav { res, desc } => {
                let o = self.ensure_res(res).unwrap_or(core::ptr::null_mut());
                if o.is_null() && desc.is_none() {
                    return;
                }
                let dp = desc.as_ref().map(|d| d.as_ptr()).unwrap_or(core::ptr::null());
                for t in targets {
                    vcall!(dev, 19, fn(*mut c_void, *mut c_void, *const u8, usize) -> (), o, core::ptr::null_mut(), dp, t);
                }
            }
            Slot::Sampler { desc } => {
                if desc.len() >= 52 {
                    vcall!(dev, 22, fn(*const u8, usize) -> (), desc.as_ptr(), targets[0]);
                }
            }
        }
    }

    unsafe fn gpu_handle(&mut self, game: u64) -> u64 {
        let Some((hi, slot)) = self.cap.heap_of_gpu(game) else {
            self.warn(format!("descriptor handle {game:#x} is not in a recorded heap"));
            return 0;
        };
        match self.heap(hi) {
            Some((_, gpu, inc, _)) => gpu + slot * inc as u64,
            None => 0,
        }
    }

    unsafe fn view(&mut self, handle: u64) -> usize {
        if handle == 0 {
            return 0;
        }
        if let Some(&c) = self.views.get(&handle) {
            return c;
        }
        let Some(v) = self.cap.views.get(&handle).cloned() else {
            self.warn(format!("render-target/depth view {handle:#x} was not recorded"));
            return 0;
        };
        let dsv = v.kind == 'd';
        if (if dsv { self.dsv.0 } else { self.rtv.0 }).is_null() {
            let n = self.cap.views.values().filter(|x| (x.kind == 'd') == dsv).count() as u32;
            match self.gpu.heap(if dsv { 3 } else { 2 }, n, false) {
                Ok((o, cpu, _, inc)) => {
                    if dsv {
                        self.dsv = (o, cpu, inc);
                    } else {
                        self.rtv = (o, cpu, inc);
                    }
                }
                Err(e) => {
                    self.warn(e);
                    return 0;
                }
            }
        }
        let used = self.views.keys().filter(|k| self.cap.views.get(k).map(|x| (x.kind == 'd') == dsv).unwrap_or(false)).count();
        let (_, base, inc) = if dsv { self.dsv } else { self.rtv };
        let cpu = base + used * inc;
        let o = self.ensure_res(v.res).unwrap_or(core::ptr::null_mut());
        let dp = v.desc.as_ref().map(|d| d.as_ptr()).unwrap_or(core::ptr::null());
        let dev = self.gpu.device;
        if !o.is_null() || !dp.is_null() {
            vcall!(dev, if dsv { 21 } else { 20 }, fn(*mut c_void, *const u8, usize) -> (), o, dp, cpu);
        }
        self.views.insert(handle, cpu);
        cpu
    }

    unsafe fn root(&mut self, game: u64) -> *mut c_void {
        if game == 0 {
            return core::ptr::null_mut();
        }
        if let Some(&r) = self.roots.get(&game) {
            return r;
        }
        let Some(blob) = self.cap.roots.get(&game).cloned() else {
            self.warn(format!("root signature {game:#x} was not recorded"));
            self.roots.insert(game, core::ptr::null_mut());
            return core::ptr::null_mut();
        };
        let mut r: *mut c_void = core::ptr::null_mut();
        let hr = vcall!(self.gpu.device, 16, fn(u32, *const u8, usize, *const d3d::Guid, *mut *mut c_void) -> i32, 0, blob.as_ptr(), blob.len(), &d3d::IID_ROOTSIG, &mut r);
        if hr < 0 {
            let m = self.gpu.messages();
            self.warn(format!("CreateRootSignature {game:#x} hr={:#010x}{m}", hr as u32));
        }
        self.roots.insert(game, r);
        r
    }

    fn translate(&mut self, pso: u64, stage: &str, hash: u64, rtfmt: [u8; 8], link: &tombox_xshader::hlsl::InputInterp) -> Option<Vec<u8>> {
        let blob = std::fs::read(self.cap.dir.join("pso").join(format!("{pso:x}_{stage}.xbox"))).ok()?;
        let r = (|| -> Result<Vec<u8>, String> {
            let ex = tombox_xshader::extract(&blob)?;
            let mut t = tombox_xshader::hlsl::translate_linked(&ex, Some(link))?;
            if let Some(dir) = &self.opts.hlsl_override {
                if let Ok(h) = std::fs::read_to_string(dir.join(format!("{hash:016x}.hlsl"))) {
                    t.hlsl = h;
                }
            }
            if rtfmt.iter().any(|&k| k != 0) {
                let defs: String = rtfmt.iter().enumerate().filter(|(_, k)| **k != 0).map(|(i, k)| format!("#define TB_RTFMT{i} {k}u\n")).collect();
                t.hlsl = defs + &t.hlsl;
            }
            let _ = std::fs::create_dir_all(self.opts.out.join("hlsl"));
            let _ = std::fs::write(self.opts.out.join("hlsl").join(format!("{hash:016x}.hlsl")), &t.hlsl);
            tombox_xshader::dxc::compile(&t.hlsl, &tombox_xshader::dxc::profile_for(t.profile))
        })();
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.warn(format!("retranslate {hash:016x} ({stage}): {e}"));
                None
            }
        }
    }

    unsafe fn pso(&mut self, game: u64) -> Option<(*mut c_void, bool)> {
        if game == 0 {
            return None;
        }
        if let Some(p) = self.psos.get(&game) {
            return p.as_ref().map(|p| (p.obj, p.compute));
        }
        let made = self.make_pso(game);
        if let Err(e) = &made {
            self.warn(format!("pso {game:#x}: {e}"));
        }
        let p = made.ok();
        let r = p.as_ref().map(|p| (p.obj, p.compute));
        self.psos.insert(game, p);
        r
    }

    unsafe fn make_pso(&mut self, game: u64) -> Result<Pso, String> {
        let rec = self.cap.psos.get(&game).cloned().ok_or("not in objects.txt")?;
        if !rec.registered {
            return Err("UNREGISTERED in the recording".into());
        }
        let raw = std::fs::read(self.cap.dir.join("pso").join(format!("{game:x}.tbpl"))).map_err(|e| e.to_string())?;
        if raw.len() < 8 || &raw[0..4] != b"TBPL" {
            return Err("bad .tbpl".into());
        }
        let n = le32(&raw, 4) as usize;
        let mut parts: Vec<&[u8]> = Vec::new();
        let mut o = 8;
        for _ in 0..n {
            let l = le32(&raw, o) as usize;
            parts.push(&raw[o + 4..o + 4 + l]);
            o += 4 + l;
        }
        let root = self.root(rec.root);
        let hashes: Vec<(String, u64)> = rec.stages.iter().filter_map(|(s, h)| h.map(|h| (s.clone(), h))).collect();
        let hash_of = |s: &str| hashes.iter().find(|e| e.0 == s).map(|e| e.1);
        let mut out: *mut c_void = core::ptr::null_mut();
        if rec.compute {
            let (mut desc, mut code) = pso_serial::decode_compute(&parts)?;
            if self.opts.retranslate {
                if let Some(h) = hash_of("cs") {
                    if let Some(c) = self.translate(game, "cs", h, [0; 8], &Default::default()) {
                        code = c;
                    }
                }
            }
            desc[0..8].copy_from_slice(&(root as u64).to_le_bytes());
            desc[0x08..0x10].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
            desc[0x10..0x18].copy_from_slice(&(code.len() as u64).to_le_bytes());
            let hr = vcall!(self.gpu.device, 11, fn(*const u8, *const d3d::Guid, *mut *mut c_void) -> i32, desc.as_ptr(), &d3d::IID_PSO, &mut out);
            if hr < 0 || out.is_null() {
                return Err(format!("CreateComputePipelineState hr={:#010x}{}", hr as u32, self.gpu.messages()));
            }
            return Ok(Pso { obj: out, compute: true, hashes, ds_write: false });
        }
        let plan = pso_serial::decode_gfx(&parts)?;
        let mut desc = plan.desc.clone();
        let mut code = plan.code.clone();
        if self.opts.retranslate {
            let nrt = le32(&desc, 0x240).min(8) as usize;
            let mut rtfmt = [0u8; 8];
            for i in 0..nrt {
                rtfmt[i] = match le32(&desc, 0x244 + i * 4) {
                    10 | 34 | 54 => 1,
                    26 => 2,
                    _ => 0,
                };
            }
            // As in d3d12_xs: the stage feeding the pixel shader (GS, else DS, else VS)
            // repeats its interpolation modifiers so both signatures pack alike.
            let stages = ["vs", "ps", "ds", "hs", "gs"];
            let feeder = [4usize, 2, 0].into_iter().find(|&k| code[k].is_some()).map(|k| stages[k]);
            let link = std::fs::read(self.cap.dir.join("pso").join(format!("{game:x}_ps.xbox")))
                .ok()
                .and_then(|b| tombox_xshader::extract(&b).ok())
                .map(|ex| tombox_xshader::hlsl::ps_input_interp(&ex))
                .unwrap_or_default();
            let none = tombox_xshader::hlsl::InputInterp::default();
            for (k, st) in stages.iter().enumerate() {
                if code[k].is_none() {
                    continue;
                }
                if let Some(h) = hash_of(st) {
                    let l = if Some(*st) == feeder { &link } else { &none };
                    if let Some(c) = self.translate(game, st, h, if *st == "ps" { rtfmt } else { [0; 8] }, l) {
                        code[k] = Some(c);
                    }
                }
            }
        }
        for (want, off, v) in self.opts.pso_patch.clone() {
            if want.map(|h| hashes.iter().any(|e| e.1 == h)).unwrap_or(true) && off + 4 <= desc.len() {
                desc[off..off + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        desc[0..8].copy_from_slice(&(root as u64).to_le_bytes());
        for (k, &o) in pso_serial::GFX_STAGES.iter().enumerate() {
            let (p, l) = code[k].as_ref().map(|c| (c.as_ptr() as u64, c.len() as u64)).unwrap_or((0, 0));
            desc[o..o + 8].copy_from_slice(&p.to_le_bytes());
            desc[o + 8..o + 16].copy_from_slice(&l.to_le_bytes());
        }
        let names: Vec<Vec<u8>> = plan.input.elems.iter().map(|e| [e.name.as_slice(), b"\0"].concat()).collect();
        let mut elems: Vec<[u8; 32]> = Vec::new();
        for (e, n) in plan.input.elems.iter().zip(&names) {
            let mut b = [0u8; 32];
            b[0..8].copy_from_slice(&(n.as_ptr() as u64).to_le_bytes());
            for (i, x) in [e.index, e.format, e.slot, e.offset, e.class, e.step].iter().enumerate() {
                b[8 + i * 4..12 + i * 4].copy_from_slice(&x.to_le_bytes());
            }
            elems.push(b);
        }
        desc[0x228..0x230].copy_from_slice(&(if elems.is_empty() { 0 } else { elems.as_ptr() as u64 }).to_le_bytes());
        desc[0x230..0x234].copy_from_slice(&(elems.len() as u32).to_le_bytes());
        let so_names: Vec<Vec<u8>> = plan.input.so.iter().map(|e| [e.name.as_slice(), b"\0"].concat()).collect();
        let mut so: Vec<[u8; 24]> = Vec::new();
        for (e, n) in plan.input.so.iter().zip(&so_names) {
            let mut b = [0u8; 24];
            b[0..4].copy_from_slice(&e.stream.to_le_bytes());
            b[8..16].copy_from_slice(&(n.as_ptr() as u64).to_le_bytes());
            b[16..20].copy_from_slice(&e.index.to_le_bytes());
            b[20] = e.start;
            b[21] = e.count;
            b[22] = e.slot;
            so.push(b);
        }
        desc[0x58..0x60].copy_from_slice(&(if so.is_empty() { 0 } else { so.as_ptr() as u64 }).to_le_bytes());
        desc[0x60..0x64].copy_from_slice(&(so.len() as u32).to_le_bytes());
        desc[0x68..0x70].copy_from_slice(&(if plan.input.strides.is_empty() { 0 } else { plan.input.strides.as_ptr() as u64 }).to_le_bytes());
        desc[0x70..0x74].copy_from_slice(&(plan.input.strides.len() as u32).to_le_bytes());
        desc[0x278..0x288].fill(0);
        let hr = vcall!(self.gpu.device, 10, fn(*const u8, *const d3d::Guid, *mut *mut c_void) -> i32, desc.as_ptr(), &d3d::IID_PSO, &mut out);
        if hr < 0 || out.is_null() {
            return Err(format!("CreateGraphicsPipelineState hr={:#010x}{}", hr as u32, self.gpu.messages()));
        }
        let ds_write = (le32(&desc, 0x1f0) != 0 && le32(&desc, 0x1f4) != 0) || (le32(&desc, 0x1fc) != 0 && desc[0x201] != 0);
        Ok(Pso { obj: out, compute: false, hashes, ds_write })
    }

    unsafe fn qheap(&mut self, game: u64, ty: u32) -> *mut c_void {
        let ht = qtype_heap(ty);
        if let Some(&h) = self.qheaps.get(&(game, ht)) {
            return h;
        }
        let count = self.qmax.get(&(game, ht)).copied().unwrap_or(1).max(1);
        let d: [u32; 3] = [ht, count, 0];
        let mut h: *mut c_void = core::ptr::null_mut();
        if vcall!(self.gpu.device, 39, fn(*const u32, *const d3d::Guid, *mut *mut c_void) -> i32, d.as_ptr(), &d3d::IID_QUERYHEAP, &mut h) < 0 {
            self.warn(format!("CreateQueryHeap type {ht} count {count} failed"));
        }
        self.qheaps.insert((game, ht), h);
        h
    }

    unsafe fn sig(&mut self, game: u64) -> *mut c_void {
        if let Some(&s) = self.sigs.get(&game) {
            return s;
        }
        let Some(rec) = self.cap.sigs.get(&game).cloned() else {
            self.warn(format!("command signature {game:#x} was not recorded"));
            self.sigs.insert(game, core::ptr::null_mut());
            return core::ptr::null_mut();
        };
        let needs_root = rec.args.iter().any(|a| (5..=8).contains(&a[0]));
        let root = if needs_root { self.root(rec.root) } else { core::ptr::null_mut() };
        let mut d = [0u8; 24];
        d[0..4].copy_from_slice(&rec.stride.to_le_bytes());
        d[4..8].copy_from_slice(&(rec.args.len() as u32).to_le_bytes());
        d[8..16].copy_from_slice(&(rec.args.as_ptr() as u64).to_le_bytes());
        let mut s: *mut c_void = core::ptr::null_mut();
        if vcall!(self.gpu.device, 41, fn(*const u8, *mut c_void, *const d3d::Guid, *mut *mut c_void) -> i32, d.as_ptr(), root, &d3d::IID_CMDSIG, &mut s) < 0 {
            self.warn(format!("CreateCommandSignature {game:#x} failed{}", self.gpu.messages()));
        }
        self.sigs.insert(game, s);
        s
    }

    fn table_range(&self, root: u64, idx: u32) -> usize {
        self.cap.roots.get(&root).map(|b| table_sizes(b)).and_then(|m| m.get(&idx).map(|e| e.1)).filter(|&n| n > 0).unwrap_or(64)
    }

    unsafe fn bind_tables(&mut self, compute: bool) -> Vec<(u64, bool)> {
        let (root, tables, views) = if compute { (self.st.croot, self.st.ctables.clone(), self.st.cviews.clone()) } else { (self.st.groot, self.st.gtables.clone(), self.st.gviews.clone()) };
        let mut used = Vec::new();
        for (idx, gpu) in tables {
            let Some((hi, slot)) = self.cap.heap_of_gpu(gpu) else { continue };
            let n = self.table_range(root, idx) as u64;
            for s in slot..(slot + n).min(self.cap.heaps[hi].count) {
                self.make_slot(hi, s);
                match self.cap.heaps[hi].slots.get(&s) {
                    Some(Slot::Srv { res, .. }) => used.push((*res, false)),
                    Some(Slot::Uav { res, .. }) => used.push((*res, true)),
                    Some(Slot::Cbv { va, .. }) => {
                        if let Some(b) = self.cap.buffer_of(*va) {
                            used.push((b.res, false));
                        }
                    }
                    _ => {}
                }
            }
        }
        for (_, (k, va)) in views {
            if let Some(b) = self.cap.buffer_of(va) {
                used.push((b.res, k == 'u'));
            }
        }
        if !compute {
            for h in self.st.rtvs.clone() {
                if let Some(v) = self.cap.views.get(&h) {
                    used.push((v.res, true));
                }
            }
            if self.st.dsv != 0 {
                let w = self.psos.get(&self.st.pso).and_then(|p| p.as_ref()).map(|p| p.ds_write).unwrap_or(true);
                if let Some(v) = self.cap.views.get(&self.st.dsv) {
                    used.push((v.res, w));
                }
            }
        }
        let mut seen = HashSet::new();
        used.retain(|e| e.0 != 0 && seen.insert(*e));
        for &(r, _) in &used {
            self.use_res(r);
        }
        used
    }

    fn pso_hashes(&self, pso: u64) -> Vec<(String, u64)> {
        self.cap.psos.get(&pso).map(|p| p.stages.iter().filter_map(|(s, h)| h.map(|h| (s.clone(), h))).collect()).unwrap_or_default()
    }

    fn wants_stats(&self, index: usize) -> bool {
        if self.opts.stats_all {
            return true;
        }
        if let Some((a, b)) = self.opts.range {
            if index >= a && index <= b {
                return true;
            }
        }
        let hs = self.pso_hashes(self.st.pso);
        hs.iter().any(|(_, h)| self.opts.stats.contains(h))
    }

    unsafe fn own_query(&mut self) -> Option<(*mut c_void, *mut c_void, u32)> {
        if self.own_q.is_none() {
            let mk = |ht: u32| -> *mut c_void {
                let d: [u32; 3] = [ht, 4096, 0];
                let mut h: *mut c_void = core::ptr::null_mut();
                vcall!(self.gpu.device, 39, fn(*const u32, *const d3d::Guid, *mut *mut c_void) -> i32, d.as_ptr(), &d3d::IID_QUERYHEAP, &mut h);
                h
            };
            let (o, p) = (mk(0), mk(2));
            self.own_q = Some((o, p, 0));
        }
        self.own_q
    }

    unsafe fn readback_res(&mut self, game: u64, label: String, compare: Option<PathBuf>, bmp: Option<PathBuf>) {
        let Some(r) = self.res.get(&game) else { return };
        let (buffer, desc, size, obj, subs) = (r.buffer, r.desc, r.size, r.obj, r.subs);
        if le32(&desc, 0x24) > 1 {
            let _ = writeln!(self.report, "    {label}: multisampled, not read back");
            return;
        }
        let prev = r.actual.clone();
        let list = self.list();
        if buffer {
            let Ok(buf) = self.gpu.buffer(size, 3, COPY_DEST, false) else { return };
            self.transition(game, None, COPY_SOURCE);
            vcall!(list, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), buf, 0, obj, 0, size);
            self.rbs.push(Readback { label, buf, kind: RbKind::Buf { size }, compare, bmp: None });
        } else {
            let p = planes(le32(&desc, 0x20));
            let n = subs / p;
            let (fps, total) = self.gpu.footprints(&desc, n);
            if total == 0 || total == u64::MAX || total > (1 << 30) {
                return;
            }
            let Ok(buf) = self.gpu.buffer(total, 3, COPY_DEST, false) else { return };
            self.transition(game, None, COPY_SOURCE);
            for (i, fp) in fps.iter().enumerate() {
                let mut dst = [0u8; 48];
                dst[0..8].copy_from_slice(&(buf as u64).to_le_bytes());
                dst[8..12].copy_from_slice(&1u32.to_le_bytes());
                dst[16..48].copy_from_slice(fp);
                let mut src = [0u8; 48];
                src[0..8].copy_from_slice(&(obj as u64).to_le_bytes());
                src[16..20].copy_from_slice(&(i as u32).to_le_bytes());
                vcall!(list, 16, fn(*const u8, u32, u32, u32, *const u8, *const c_void) -> (), dst.as_ptr(), 0, 0, 0, src.as_ptr(), core::ptr::null());
            }
            self.rbs.push(Readback { label, buf, kind: RbKind::Tex { fps }, compare, bmp });
        }
        if prev.iter().all(|&a| a == prev[0]) {
            self.transition(game, None, prev[0]);
        } else {
            for (i, a) in prev.into_iter().enumerate() {
                self.transition(game, Some(i as u32), a);
            }
        }
    }

    unsafe fn finish_readbacks(&mut self) {
        let rbs = std::mem::take(&mut self.rbs);
        for rb in rbs {
            match &rb.kind {
                RbKind::Buf { size } => {
                    if let Some(d) = Gpu::map_read(rb.buf, *size as usize) {
                        let n = d.len() / 4;
                        let (mut nan, mut inf) = (0u64, 0u64);
                        for c in d.chunks_exact(4) {
                            let f = f32::from_le_bytes(c.try_into().unwrap());
                            nan += f.is_nan() as u64;
                            inf += f.is_infinite() as u64;
                        }
                        let mut s = format!("    {}: buffer {} bytes; as f32: nan={nan} inf={inf} of {n}", rb.label, d.len());
                        if let Some(c) = &rb.compare {
                            match std::fs::read(c) {
                                Ok(want) => {
                                    let m = want.len().min(d.len());
                                    let diff = (0..m).filter(|&i| want[i] != d[i]).count();
                                    let _ = write!(s, "; vs recorded post-snapshot: {diff} of {m} bytes differ");
                                }
                                Err(_) => {
                                    let _ = write!(s, "; no post-snapshot file");
                                }
                            }
                        }
                        let _ = writeln!(self.report, "{s}");
                    }
                }
                RbKind::Tex { fps } => {
                    let total = fps.last().map(|f| le64(f, 0) + le32(f, 24) as u64 * le32(f, 16) as u64 * le32(f, 20) as u64).unwrap_or(0);
                    let Some(d) = Gpu::map_read(rb.buf, total as usize) else { continue };
                    let want = rb.compare.as_ref().and_then(|c| std::fs::read(c).ok());
                    for (i, fp) in fps.iter().enumerate().take(16) {
                        let (off, fmt, w, h, depth, pitch) = (le64(fp, 0) as usize, le32(fp, 8), le32(fp, 12), le32(fp, 16), le32(fp, 20), le32(fp, 24) as usize);
                        let len = pitch * h as usize * depth.max(1) as usize;
                        if off + len > d.len() {
                            continue;
                        }
                        let img = Image { fmt, width: w, height: h, pitch, data: d[off..off + len].to_vec() };
                        let st = img.stats();
                        let mut s = format!("    {} sub{i} fmt={fmt} {w}x{h}: {}", rb.label, st.line());
                        if let Some(wb) = &want {
                            if wb.len() > 8 && &wb[0..4] == b"TBTX" {
                                let n = le32(wb, 4) as usize;
                                let head = 8 + n * 32;
                                if i < n && wb.len() >= head {
                                    let wfp = &wb[8 + i * 32..8 + i * 32 + 32];
                                    let (wo, wp, wh) = (le64(wfp, 0) as usize, le32(wfp, 24) as usize, le32(wfp, 16) as usize);
                                    let wl = wp * wh;
                                    if head + wo + wl <= wb.len() {
                                        let wimg = Image { fmt, width: le32(wfp, 12), height: wh as u32, pitch: wp, data: wb[head + wo..head + wo + wl].to_vec() };
                                        let ws = wimg.stats();
                                        let (m, diff, nn) = max_abs_diff(&img, &wimg);
                                        let _ = write!(s, "\n      recorded post: {} | replay vs recorded: {diff}/{nn} texels differ, max abs diff {m}", ws.line());
                                    }
                                }
                            }
                        }
                        let _ = writeln!(self.report, "{s}");
                        if let Some(b) = &rb.bmp {
                            let p = if fps.len() > 1 { b.with_file_name(format!("{}_sub{i}.bmp", b.file_stem().unwrap().to_string_lossy())) } else { b.clone() };
                            let _ = img.write_bmp(&p);
                        }
                    }
                }
                RbKind::Query { heap_type, count } => {
                    let per = if *heap_type == 2 { 88 } else { 8 };
                    if let Some(d) = Gpu::map_read(rb.buf, per * *count as usize) {
                        let mut s = format!("    {}:", rb.label);
                        for i in 0..*count as usize {
                            if *heap_type == 2 {
                                let q: Vec<u64> = (0..11).map(|k| le64(&d, i * 88 + k * 8)).collect();
                                let _ = write!(s, " [IAVerts={} IAPrims={} VS={} GS={} GSPrims={} CInv={} CPrims={} PS={} HS={} DS={} CS={}]", q[0], q[1], q[2], q[3], q[4], q[5], q[6], q[7], q[8], q[9], q[10]);
                            } else {
                                let _ = write!(s, " {}", le64(&d, i * 8));
                            }
                        }
                        let _ = writeln!(self.report, "{s}");
                    }
                }
            }
            d3d::release(rb.buf);
        }
        for k in std::mem::take(&mut self.keep) {
            d3d::release(k);
        }
    }

    unsafe fn resolve_list_queries(&mut self, used: &BTreeMap<(u64, u32), (u32, u32)>) {
        for (&(heap, ht), &(lo, hi)) in used {
            let q = self.qheap(heap, if ht == 2 { 3 } else if ht == 1 { 2 } else { 0 });
            if q.is_null() || hi <= lo {
                continue;
            }
            let per = if ht == 2 { 88u64 } else { 8 };
            let n = hi - lo;
            let Ok(buf) = self.gpu.buffer(per * n as u64, 3, COPY_DEST, false) else { continue };
            let qt = match ht {
                2 => 3,
                1 => 2,
                _ => 0,
            };
            vcall!(self.list(), 54, fn(*mut c_void, u32, u32, u32, *mut c_void, u64) -> (), q, qt, lo, n, buf, 0);
            self.rbs.push(Readback { label: format!("game query heap {heap:#x} type {qt} indices {lo}..{hi}"), buf, kind: RbKind::Query { heap_type: ht, count: n }, compare: None, bmp: None });
        }
    }

    fn state_key(name: &str, w: &[&str]) -> Option<String> {
        Some(match name {
            "topology" | "viewports" | "scissors" | "blendfactor" | "stencilref" | "ib" | "targets" | "depthbounds" | "predication" | "heaps" | "pso" => name.to_string(),
            "vbs" => format!("vbs {}", w.get(1).unwrap_or(&"0")),
            "gtable" | "ctable" | "gcbv" | "ccbv" | "gsrv" | "csrv" | "guav" | "cuav" => format!("{}{} {}", &name[..1], "bind", w.get(1).unwrap_or(&"0")),
            "gconst" | "cconst" => format!("{name} {} {}", w.get(1).unwrap_or(&"0"), w.get(2).unwrap_or(&"0")),
            "gconsts" | "cconsts" => format!("{name} {} {}", w.get(1).unwrap_or(&"0"), w.get(2).unwrap_or(&"0")),
            _ => return None,
        })
    }

    pub unsafe fn exec_line(&mut self, text: &str, index: usize, qused: &mut BTreeMap<(u64, u32), (u32, u32)>) -> Result<(), String> {
        let w: Vec<&str> = text.split_whitespace().collect();
        let Some(&name) = w.first() else { return Ok(()) };
        if name == "groot" || name == "croot" {
            let c = name == "croot";
            self.st.lines.retain(|k, _| !k.starts_with(if c { "cbind" } else { "gbind" }) && !k.starts_with(if c { "cconst" } else { "gconst" }));
            self.st.lines.insert(name.to_string(), text.to_string());
        } else if let Some(k) = Self::state_key(name, &w) {
            self.st.lines.insert(k, text.to_string());
        }
        let list = self.list();
        let dry = self.dry;
        match name {
            "reset" => {
                self.st = ListState::default();
                let p = kv(&w, "pso").map(num).unwrap_or(0);
                if p != 0 {
                    self.st.pso = p;
                    self.st.lines.insert("pso".into(), format!("pso {p:#x}"));
                    if !dry {
                        if let Some((o, _)) = self.pso(p) {
                            vcall!(list, 25, fn(*mut c_void) -> (), o);
                        }
                    }
                }
            }
            "close" => {}
            "pso" => {
                let p = num(w[1]);
                self.st.pso = p;
                if let Some((o, _)) = self.pso(p) {
                    if !dry {
                        vcall!(list, 25, fn(*mut c_void) -> (), o);
                    }
                }
            }
            "topology" => {
                if !dry {
                    vcall!(list, 20, fn(u32) -> (), num(w[1]) as u32);
                }
            }
            "viewports" | "scissors" => {
                let n = num(w[1]) as u32;
                let b = unhex(w.get(2).unwrap_or(&""));
                if !dry {
                    vcall!(list, if name == "viewports" { 21 } else { 22 }, fn(u32, *const u8) -> (), n, b.as_ptr());
                }
            }
            "blendfactor" => {
                let b = unhex(w[1]);
                if !dry && b.len() >= 16 {
                    vcall!(list, 23, fn(*const u8) -> (), b.as_ptr());
                }
            }
            "stencilref" => {
                if !dry {
                    vcall!(list, 24, fn(u32) -> (), num(w[1]) as u32);
                }
            }
            "depthbounds" => {
                if !dry {
                    let lo = f32::from_bits(u32::from_str_radix(w[1], 16).unwrap_or(0));
                    let hi = f32::from_bits(u32::from_str_radix(w[2], 16).unwrap_or(0x3f80_0000));
                    vcall!(list, 62, fn(f32, f32) -> (), lo, hi);
                }
            }
            "barrier" => {
                let b = unhex(w.get(2).unwrap_or(&""));
                for c in b.chunks_exact(32) {
                    let ty = le32(c, 0);
                    let flags = le32(c, 4);
                    let r = le64(c, 8);
                    match ty {
                        0 => {
                            if flags & 1 != 0 {
                                continue;
                            }
                            let sub = le32(c, 16);
                            let after = le32(c, 24);
                            if self.ensure_res(r).is_none() {
                                continue;
                            }
                            if !dry {
                                self.sync(r);
                                self.transition(r, if sub == u32::MAX { None } else { Some(sub) }, after);
                            }
                            if let Some(x) = self.res.get_mut(&r) {
                                if sub == u32::MAX {
                                    x.logical.iter_mut().for_each(|l| *l = after);
                                } else if (sub as usize) < x.logical.len() {
                                    x.logical[sub as usize] = after;
                                }
                            }
                        }
                        2 => {
                            if !dry {
                                let o = if r == 0 { core::ptr::null_mut() } else { self.ensure_res(r).unwrap_or(core::ptr::null_mut()) };
                                let bb = [2u64, o as u64, 0, 0];
                                vcall!(list, 26, fn(u32, *const u64) -> (), 1, bb.as_ptr());
                            }
                        }
                        _ => {}
                    }
                }
            }
            "heaps" => {
                let mut hs: Vec<*mut c_void> = Vec::new();
                for e in &w[2..] {
                    let gpu = e.split(':').find_map(|p| p.strip_prefix("gpu=")).map(num).unwrap_or(0);
                    if let Some((hi, _)) = self.cap.heap_of_gpu(gpu) {
                        if self.heap(hi).is_some() {
                            hs.push(self.heaps[&hi].obj);
                        }
                    }
                }
                if !dry && !hs.is_empty() {
                    vcall!(list, 28, fn(u32, *const *mut c_void) -> (), hs.len() as u32, hs.as_ptr());
                }
            }
            "groot" | "croot" => {
                let g = num(w[1]);
                let c = name == "croot";
                if c {
                    self.st.croot = g;
                    self.st.ctables.clear();
                    self.st.cviews.clear();
                } else {
                    self.st.groot = g;
                    self.st.gtables.clear();
                    self.st.gviews.clear();
                }
                let r = self.root(g);
                if !dry && !r.is_null() {
                    vcall!(list, if c { 29 } else { 30 }, fn(*mut c_void) -> (), r);
                }
            }
            "gtable" | "ctable" => {
                let (i, h) = (num(w[1]) as u32, num(w[2]));
                if name == "ctable" {
                    self.st.ctables.insert(i, h);
                } else {
                    self.st.gtables.insert(i, h);
                }
                let g = self.gpu_handle(h);
                if !dry && g != 0 {
                    vcall!(list, if name == "ctable" { 31 } else { 32 }, fn(u32, u64) -> (), i, g);
                }
            }
            "cconst" | "gconst" => {
                if !dry {
                    vcall!(list, if name == "cconst" { 33 } else { 34 }, fn(u32, u32, u32) -> (), num(w[1]) as u32, u32::from_str_radix(w[3], 16).unwrap_or(0), num(w[2]) as u32);
                }
            }
            "cconsts" | "gconsts" => {
                let b = unhex(w.get(3).unwrap_or(&""));
                if !dry {
                    vcall!(list, if name == "cconsts" { 35 } else { 36 }, fn(u32, u32, *const u8, u32) -> (), num(w[1]) as u32, (b.len() / 4) as u32, b.as_ptr(), num(w[2]) as u32);
                }
            }
            "ccbv" | "gcbv" | "csrv" | "gsrv" | "cuav" | "guav" => {
                let (i, va) = (num(w[1]) as u32, num(w[2]));
                let k = name.as_bytes()[1] as char;
                let k = if k == 'c' { 'c' } else if k == 's' { 's' } else { 'u' };
                if name.starts_with('c') {
                    self.st.cviews.insert(i, (k, va));
                } else {
                    self.st.gviews.insert(i, (k, va));
                }
                let v = self.va(va);
                let slot = match name {
                    "ccbv" => 37,
                    "gcbv" => 38,
                    "csrv" => 39,
                    "gsrv" => 40,
                    "cuav" => 41,
                    _ => 42,
                };
                if !dry {
                    vcall!(list, slot, fn(u32, u64) -> (), i, v);
                }
            }
            "ib" => {
                if w.get(1) == Some(&"null") {
                    if !dry {
                        vcall!(list, 43, fn(*const u8) -> (), core::ptr::null());
                    }
                } else {
                    let v = self.va(num(w[1]));
                    let mut b = [0u8; 16];
                    b[0..8].copy_from_slice(&v.to_le_bytes());
                    b[8..12].copy_from_slice(&(num(w[2]) as u32).to_le_bytes());
                    b[12..16].copy_from_slice(&(num(w[3]) as u32).to_le_bytes());
                    if !dry {
                        vcall!(list, 43, fn(*const u8) -> (), b.as_ptr());
                    }
                }
            }
            "vbs" => {
                let start = num(w[1]) as u32;
                let mut v: Vec<[u8; 16]> = Vec::new();
                for e in &w[3..] {
                    let p: Vec<&str> = e.split(':').collect();
                    if p.len() < 3 {
                        continue;
                    }
                    let a = self.va(num(p[0]));
                    let mut b = [0u8; 16];
                    b[0..8].copy_from_slice(&a.to_le_bytes());
                    b[8..12].copy_from_slice(&(num(p[1]) as u32).to_le_bytes());
                    b[12..16].copy_from_slice(&(num(p[2]) as u32).to_le_bytes());
                    v.push(b);
                }
                if !dry {
                    vcall!(list, 44, fn(u32, u32, *const [u8; 16]) -> (), start, v.len() as u32, if v.is_empty() { core::ptr::null() } else { v.as_ptr() });
                }
            }
            "targets" => {
                let n = num(w[1]) as usize;
                let single = kv(&w, "single").map(num).unwrap_or(0) != 0;
                let hs: Vec<u64> = w.iter().skip(3).filter(|s| !s.starts_with("dsv=")).map(|s| num(s)).collect();
                let dsv = kv(&w, "dsv").map(num).unwrap_or(0);
                let mut handles = Vec::new();
                for i in 0..n.min(8) {
                    let gh = if single {
                        let base = hs.first().copied().unwrap_or(0);
                        if i == 0 { base } else { 0 }
                    } else {
                        hs.get(i).copied().unwrap_or(0)
                    };
                    if single && i > 0 {
                        handles.push(handles[0] + i * self.rtv.2);
                    } else {
                        handles.push(self.view(gh));
                    }
                }
                self.st.rtvs = if single { hs.first().copied().into_iter().collect() } else { hs.clone() };
                self.st.dsv = dsv;
                let d = self.view(dsv);
                if !dry {
                    vcall!(list, 46, fn(u32, *const usize, i32, *const usize) -> (), handles.len() as u32, if handles.is_empty() { core::ptr::null() } else { handles.as_ptr() }, 0, if d == 0 { core::ptr::null() } else { &d });
                }
            }
            "cleardsv" => {
                let h = self.view(num(w[1]));
                if let Some(v) = self.cap.views.get(&num(w[1])).cloned() {
                    self.use_res(v.res);
                }
                let rects = unhex(w.get(6).unwrap_or(&""));
                if !dry && h != 0 {
                    vcall!(list, 47, fn(usize, u32, f32, u8, u32, *const u8) -> (), h, num(w[2]) as u32, f32::from_bits(u32::from_str_radix(w[3], 16).unwrap_or(0)), num(w[4]) as u8, (rects.len() / 16) as u32, if rects.is_empty() { core::ptr::null() } else { rects.as_ptr() });
                }
            }
            "clearrtv" => {
                let h = self.view(num(w[1]));
                if let Some(v) = self.cap.views.get(&num(w[1])).cloned() {
                    self.use_res(v.res);
                }
                let color = unhex(w[2]);
                let rects = unhex(w.get(4).unwrap_or(&""));
                if !dry && h != 0 && color.len() >= 16 {
                    vcall!(list, 48, fn(usize, *const u8, u32, *const u8) -> (), h, color.as_ptr(), (rects.len() / 16) as u32, if rects.is_empty() { core::ptr::null() } else { rects.as_ptr() });
                }
            }
            "clearuavu" | "clearuavf" => {
                let (g, c, r) = (num(w[1]), num(w[2]), num(w[3]));
                let vals = unhex(w[4]);
                let rects = unhex(w.get(6).unwrap_or(&""));
                let gh = self.gpu_handle(g);
                let o = self.use_res(r);
                let cpu = match self.cap.heap_of_gpu(g) {
                    Some((hi, slot)) => {
                        self.make_slot(hi, slot);
                        self.heaps.get(&hi).and_then(|h| h.mirror.map(|m| m.1 + slot as usize * h.inc)).unwrap_or(0)
                    }
                    None => 0,
                };
                let _ = c;
                if !dry && gh != 0 && cpu != 0 && o.is_some() && vals.len() >= 16 {
                    vcall!(list, if name == "clearuavu" { 49 } else { 50 }, fn(u64, usize, *mut c_void, *const u8, u32, *const u8) -> (), gh, cpu, o.unwrap(), vals.as_ptr(), (rects.len() / 16) as u32, if rects.is_empty() { core::ptr::null() } else { rects.as_ptr() });
                }
            }
            "copybuf" => {
                let (d, doff, s, soff) = (kv(&w, "dst").map(num).unwrap_or(0), num(w[2]), kv(&w, "src").map(num).unwrap_or(0), num(w[4]));
                let n = kv(&w, "n").map(num).unwrap_or(0);
                let (od, os) = (self.use_res(d), self.use_res(s));
                if let (false, Some(od), Some(os)) = (dry, od, os) {
                    vcall!(list, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), od, doff, os, soff, n);
                }
            }
            "copytex" => {
                let mut dst = unhex(kv(&w, "dst").unwrap_or(""));
                let mut src = unhex(kv(&w, "src").unwrap_or(""));
                let bx = unhex(kv(&w, "box").unwrap_or(""));
                if dst.len() < 48 || src.len() < 48 {
                    return Ok(());
                }
                let (gd, gs) = (le64(&dst, 0), le64(&src, 0));
                let (od, os) = (self.use_res(gd), self.use_res(gs));
                let (Some(od), Some(os)) = (od, os) else { return Ok(()) };
                dst[0..8].copy_from_slice(&(od as u64).to_le_bytes());
                src[0..8].copy_from_slice(&(os as u64).to_le_bytes());
                let (x, y, z) = (num(w[2]) as u32, num(w[3]) as u32, num(w[4]) as u32);
                if !dry {
                    vcall!(list, 16, fn(*const u8, u32, u32, u32, *const u8, *const u8) -> (), dst.as_ptr(), x, y, z, src.as_ptr(), if bx.len() >= 24 { bx.as_ptr() } else { core::ptr::null() });
                }
            }
            "copyres" => {
                let (d, s) = (kv(&w, "dst").map(num).unwrap_or(0), kv(&w, "src").map(num).unwrap_or(0));
                let (od, os) = (self.use_res(d), self.use_res(s));
                if let (false, Some(od), Some(os)) = (dry, od, os) {
                    vcall!(list, 17, fn(*mut c_void, *mut c_void) -> (), od, os);
                }
            }
            "fill" => {
                let (va, size, value) = (num(w[1]), num(w[2]), u32::from_str_radix(w[3], 16).unwrap_or(0));
                if let Some(b) = self.cap.buffer_of(va).cloned() {
                    if let Some(o) = self.ensure_res(b.res) {
                        if !dry && size > 0 && size < (256 << 20) {
                            let pat: Vec<u8> = (0..size as usize / 4).flat_map(|_| value.to_le_bytes()).collect();
                            if let Ok(up) = self.gpu.buffer(pat.len() as u64, 2, GENERIC_READ, false) {
                                Gpu::map_write(up, &pat);
                                self.keep.push(up);
                                let prev = self.res[&b.res].actual[0];
                                self.transition(b.res, None, COPY_DEST);
                                vcall!(list, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), o, va - b.va, up, 0, pat.len() as u64);
                                self.transition(b.res, None, prev);
                            }
                        }
                    }
                }
            }
            "discard" => {}
            "beginquery" | "endquery" => {
                let h = kv(&w, "heap").map(num).unwrap_or(0);
                let (t, i) = (num(w[2]) as u32, num(w[3]) as u32);
                let q = self.qheap(h, t);
                let e = qused.entry((h, qtype_heap(t))).or_insert((u32::MAX, 0));
                e.0 = e.0.min(i);
                e.1 = e.1.max(i + 1);
                if !dry && !q.is_null() {
                    vcall!(list, if name == "beginquery" { 52 } else { 53 }, fn(*mut c_void, u32, u32) -> (), q, t, i);
                }
            }
            "resolvequery" => {
                let h = kv(&w, "heap").map(num).unwrap_or(0);
                let (t, s, n) = (num(w[2]) as u32, num(w[3]) as u32, num(w[4]) as u32);
                let d = kv(&w, "dst").map(num).unwrap_or(0);
                let off = num(w[6]);
                let q = self.qheap(h, t);
                if self.ensure_res(d).is_some() && !dry && !q.is_null() {
                    self.transition(d, None, COPY_DEST);
                    if let Some(x) = self.res.get_mut(&d) {
                        x.logical.iter_mut().for_each(|l| *l = COPY_DEST);
                    }
                    vcall!(list, 54, fn(*mut c_void, u32, u32, u32, *mut c_void, u64) -> (), q, t, s, n, self.res[&d].obj, off);
                }
            }
            "predication" => {
                let b = kv(&w, "buf").map(num).unwrap_or(0);
                if self.opts.no_predication || dry {
                    return Ok(());
                }
                if b == 0 {
                    vcall!(list, 55, fn(*mut c_void, u64, u32) -> (), core::ptr::null_mut(), 0, 0);
                } else if self.ensure_res(b).is_some() {
                    self.transition(b, None, 0x200);
                    if let Some(x) = self.res.get_mut(&b) {
                        x.logical.iter_mut().for_each(|l| *l = 0x200);
                    }
                    vcall!(list, 55, fn(*mut c_void, u64, u32) -> (), self.res[&b].obj, num(w[2]), num(w[3]) as u32);
                }
            }
            "draw" | "drawidx" | "dispatch" | "indirect" => {
                self.calls += 1;
                let compute = name == "dispatch" || self.psos.get(&self.st.pso).and_then(|p| p.as_ref()).map(|p| p.compute).unwrap_or(false) && name == "indirect";
                let used = self.bind_tables(compute);
                let stats = !dry && self.wants_stats(index);
                let mut own = None;
                if stats && name != "dispatch" {
                    if let Some((_, p, k)) = self.own_query() {
                        if !p.is_null() && k < 4096 {
                            vcall!(list, 52, fn(*mut c_void, u32, u32) -> (), p, 3, k);
                            own = Some(k);
                        }
                    }
                }
                if !dry {
                    let a: Vec<u64> = w[1..].iter().take(5).map(|s| num(s)).collect();
                    match name {
                        "draw" => vcall!(list, 12, fn(u32, u32, u32, u32) -> (), a[0] as u32, a[1] as u32, a[2] as u32, a[3] as u32),
                        "drawidx" => vcall!(list, 13, fn(u32, u32, u32, i32, u32) -> (), a[0] as u32, a[1] as u32, a[2] as u32, a[3] as i32, a[4] as u32),
                        "dispatch" => vcall!(list, 14, fn(u32, u32, u32) -> (), a[0] as u32, a[1] as u32, a[2] as u32),
                        _ => {
                            let s = self.sig(kv(&w, "sig").map(num).unwrap_or(0));
                            let ab = self.use_res(kv(&w, "buf").map(num).unwrap_or(0));
                            let cb = self.use_res(kv(&w, "count").map(num).unwrap_or(0)).unwrap_or(core::ptr::null_mut());
                            let n = num(w[2]) as u32;
                            let (aoff, coff) = (num(w[4]), num(w[6]));
                            if let (false, Some(ab)) = (s.is_null(), ab) {
                                vcall!(list, 59, fn(*mut c_void, u32, *mut c_void, u64, *mut c_void, u64) -> (), s, n, ab, aoff, cb, coff);
                            }
                        }
                    }
                }
                if let Some(k) = own {
                    let (_, p, _) = self.own_q.unwrap();
                    vcall!(list, 53, fn(*mut c_void, u32, u32) -> (), p, 3, k);
                    if let Ok(buf) = self.gpu.buffer(88, 3, COPY_DEST, false) {
                        vcall!(list, 54, fn(*mut c_void, u32, u32, u32, *mut c_void, u64) -> (), p, 3, k, 1, buf, 0);
                        self.rbs.push(Readback { label: format!("line {index} pipeline stats"), buf, kind: RbKind::Query { heap_type: 2, count: 1 }, compare: None, bmp: None });
                    }
                    self.own_q.as_mut().unwrap().2 += 1;
                }
                if stats {
                    let hs: Vec<String> = self.pso_hashes(self.st.pso).iter().map(|(s, h)| format!("{s}={h:016x}")).collect();
                    let _ = writeln!(self.report, "  line {index}: {text}  pso={:#x} {}", self.st.pso, hs.join(" "));
                    for (r, wr) in used {
                        if wr {
                            let bmp = if self.opts.bmp { Some(self.opts.out.join(format!("line{index:05}_{r:x}.bmp"))) } else { None };
                            self.readback_res(r, format!("line {index} out {r:#x}"), None, bmp);
                        }
                    }
                }
            }
            "snapcall" | "snap" | "snapskip" => {}
            _ => {
                if self.opts.verbose {
                    self.warn(format!("unhandled command '{name}'"));
                }
            }
        }
        Ok(())
    }

    pub unsafe fn run_full(&mut self) -> Result<(), String> {
        self.dry = false;
        let exs = self.cap.executes.clone();
        for ex in exs {
            let _ = writeln!(self.report, "execute #{} list={:#x} lines={}{}", ex.n, ex.list, ex.lines.len(), if ex.complete { "" } else { " (recorded mid-list: state before the first line is missing)" });
            self.gpu.begin();
            self.st = ListState::default();
            let mut qused = BTreeMap::new();
            for l in &ex.lines {
                self.exec_line(&l.text, l.index, &mut qused)?;
            }
            self.resolve_list_queries(&qused);
            if let Err(e) = self.gpu.submit() {
                let _ = writeln!(self.report, "  SUBMIT FAILED: {e}");
                return Err(e);
            }
            self.finish_readbacks();
        }
        let _ = writeln!(self.report, "replayed {} executes, {} draw/dispatch calls", self.cap.executes.len(), self.calls);
        Ok(())
    }

    pub unsafe fn run_isolate(&mut self) -> Result<(), String> {
        let exs = self.cap.executes.clone();
        let mut done = 0usize;
        for ex in exs {
            self.st = ListState::default();
            let mut qused = BTreeMap::new();
            let mut i = 0;
            while i < ex.lines.len() {
                let l = &ex.lines[i];
                if !l.text.starts_with("snapcall pre") {
                    self.dry = true;
                    self.exec_line(&l.text, l.index, &mut qused)?;
                    i += 1;
                    continue;
                }
                let head = l.text.clone();
                let mut pre = Vec::new();
                let mut j = i + 1;
                while j < ex.lines.len() && (ex.lines[j].text.starts_with("snap pre") || ex.lines[j].text.starts_with("snapskip pre")) {
                    pre.push(ex.lines[j].text.clone());
                    j += 1;
                }
                let Some(call) = ex.lines.get(j).cloned() else { break };
                j += 1;
                let mut post = Vec::new();
                if j < ex.lines.len() && ex.lines[j].text.starts_with("snapcall post") {
                    j += 1;
                    while j < ex.lines.len() && (ex.lines[j].text.starts_with("snap post") || ex.lines[j].text.starts_with("snapskip post")) {
                        post.push(ex.lines[j].text.clone());
                        j += 1;
                    }
                }
                self.isolate_one(&head, &pre, &call.text, call.index, &post)?;
                self.dry = true;
                let mut q2 = BTreeMap::new();
                self.exec_line(&call.text, call.index, &mut q2)?;
                done += 1;
                i = j;
            }
        }
        let _ = writeln!(self.report, "isolated {done} snapshotted calls");
        Ok(())
    }

    fn snap_file(&self, line: &str, refs: &HashMap<u64, String>) -> Option<PathBuf> {
        let w: Vec<&str> = line.split_whitespace().collect();
        if let Some(f) = kv(&w, "file") {
            return Some(self.cap.dir.join(f));
        }
        if let Some(r) = kv(&w, "ref") {
            let _ = refs;
            return Some(self.cap.dir.join(format!("data/snap_{r}.bin")));
        }
        None
    }

    unsafe fn isolate_one(&mut self, head: &str, pre: &[String], call: &str, index: usize, post: &[String]) -> Result<(), String> {
        let _ = writeln!(self.report, "isolate line {index}: {call}\n  {head}");
        self.dry = false;
        self.gpu.begin();
        let lines: Vec<String> = {
            let l = &self.st.lines;
            let mut v = Vec::new();
            for k in ["heaps", "groot", "croot"] {
                if let Some(x) = l.get(k) {
                    v.push(x.clone());
                }
            }
            for (k, x) in l {
                if !["heaps", "groot", "croot", "predication"].contains(&k.as_str()) {
                    v.push(x.clone());
                }
            }
            v
        };
        let saved = self.st.clone();
        let mut q = BTreeMap::new();
        for l in &lines {
            self.exec_line(l, index, &mut q)?;
        }
        self.st = ListState { lines: saved.lines.clone(), ..self.st.clone() };
        let refs = HashMap::new();
        for p in pre {
            let w: Vec<&str> = p.split_whitespace().collect();
            let r = kv(&w, "res").map(num).unwrap_or(0);
            if p.starts_with("snapskip") {
                let _ = writeln!(self.report, "  input {r:#x} {}: no snapshot ({}) - using end-of-frame contents", kv(&w, "use").unwrap_or(""), kv(&w, "reason").unwrap_or(""));
                continue;
            }
            let Some(f) = self.snap_file(p, &refs) else { continue };
            let off = kv(&w, "off").map(num).unwrap_or(0);
            if self.ensure_res(r).is_none() {
                continue;
            }
            match std::fs::read(&f) {
                Ok(b) => {
                    let logical = self.res[&r].logical.clone();
                    self.upload(r, &b, off);
                    if let Some(x) = self.res.get_mut(&r) {
                        x.logical = logical;
                    }
                }
                Err(e) => {
                    let _ = writeln!(self.report, "  input {r:#x}: {} unreadable ({e})", f.display());
                }
            }
        }
        let used = self.bind_tables(call.starts_with("dispatch"));
        for &(r, _) in &used {
            self.sync(r);
        }
        let gfx = !call.starts_with("dispatch");
        let qs = self.own_query();
        let k = qs.map(|q| q.2).unwrap_or(0);
        let list = self.list();
        if let Some((o, p, _)) = qs {
            if gfx && !o.is_null() {
                vcall!(list, 52, fn(*mut c_void, u32, u32) -> (), o, 0, k);
            }
            if !p.is_null() {
                vcall!(list, 52, fn(*mut c_void, u32, u32) -> (), p, 3, k);
            }
        }
        let save_stats = (self.opts.stats_all, self.opts.stats.clone(), self.opts.range);
        self.opts.stats_all = false;
        self.opts.stats.clear();
        self.opts.range = None;
        let mut q2 = BTreeMap::new();
        self.exec_line(call, index, &mut q2)?;
        (self.opts.stats_all, self.opts.stats, self.opts.range) = save_stats;
        if let Some((o, p, _)) = qs {
            for (h, ht, qt) in [(o, 0u32, 0u32), (p, 2, 3)] {
                if h.is_null() || (qt == 0 && !gfx) {
                    continue;
                }
                vcall!(list, 53, fn(*mut c_void, u32, u32) -> (), h, qt, k);
                if let Ok(buf) = self.gpu.buffer(88, 3, COPY_DEST, false) {
                    vcall!(list, 54, fn(*mut c_void, u32, u32, u32, *mut c_void, u64) -> (), h, qt, k, 1, buf, 0);
                    self.rbs.push(Readback { label: if qt == 0 { "occlusion samples".into() } else { "pipeline stats".into() }, buf, kind: RbKind::Query { heap_type: ht, count: 1 }, compare: None, bmp: None });
                }
            }
            self.own_q.as_mut().unwrap().2 += 1;
        }
        let mut outs: Vec<(u64, Option<PathBuf>, String)> = Vec::new();
        for p in post {
            let w: Vec<&str> = p.split_whitespace().collect();
            let r = kv(&w, "res").map(num).unwrap_or(0);
            let why = kv(&w, "use").unwrap_or("").to_string();
            outs.push((r, if p.starts_with("snapskip") { None } else { self.snap_file(p, &refs) }, why));
        }
        if outs.is_empty() {
            for (r, wr) in &used {
                if *wr {
                    outs.push((*r, None, "bound-writable".into()));
                }
            }
        }
        for (r, cmp, why) in outs {
            let bmp = if self.opts.bmp { Some(self.opts.out.join(format!("iso{index:05}_{r:x}.bmp"))) } else { None };
            self.readback_res(r, format!("out {r:#x} ({why})"), cmp, bmp);
        }
        if let Err(e) = self.gpu.submit() {
            let _ = writeln!(self.report, "  SUBMIT FAILED: {e}");
            return Err(e);
        }
        self.finish_readbacks();
        self.gpu.begin();
        self.st = saved;
        Ok(())
    }

    pub unsafe fn dump_targets(&mut self) {
        self.gpu.begin();
        let ids: Vec<u64> = self.res.iter().filter(|(_, r)| !r.buffer && le32(&r.desc, 0x30) & 0x7 != 0).map(|(k, _)| *k).collect();
        let _ = writeln!(self.report, "end-of-replay targets ({}):", ids.len());
        for r in ids {
            let bmp = if self.opts.bmp { Some(self.opts.out.join(format!("final_{r:x}.bmp"))) } else { None };
            self.readback_res(r, format!("final {r:#x}"), None, bmp);
        }
        if self.gpu.submit().is_ok() {
            self.finish_readbacks();
        }
    }
}
