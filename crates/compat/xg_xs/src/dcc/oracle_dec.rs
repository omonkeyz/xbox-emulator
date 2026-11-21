use super::bits::valid_meta;
use super::{dec, px, Dcc};
use crate::dcc_oracle::{configs, diff, holes, Con, Rng};
use crate::layout::E_INVALIDARG;
use crate::oracle::{oracle, Oracle, SERIAL};

pub(super) type Lay = unsafe extern "system" fn(*mut u8, *mut u8, *const u8, u32);
pub(super) type Conv = unsafe extern "system" fn(*mut u8, *mut u8, *const u8, *const u32, *const u32);
type Wd = unsafe extern "system" fn(*const u8, *const u8) -> u64;
type Clr = unsafe extern "system" fn(*const u8, *const u8, u8, u8) -> u64;
pub(super) type Enc = unsafe extern "system" fn(u32, *const u8, *mut u8, *mut u8, *mut u32, u32, u32, u32, *const u32) -> i32;
pub(super) type DecF = unsafe extern "system" fn(u32, *const u8, *const u8, *mut u8, u32, u32, u32, *const u32) -> i32;

pub(super) const P1S: [u32; 5] = [0, 1, 2, 3, 0x8000_0000];

pub(super) fn copy(d: &Dcc) -> Dcc {
    let mut e = Dcc::new();
    e.m.copy_from_slice(&d.m);
    e.b = d.b;
    e
}

pub(super) struct Tally {
    names: Vec<&'static str>,
    cases: Vec<u64>,
    bad: Vec<u64>,
    errs: Vec<u64>,
}

impl Tally {
    pub(super) fn new() -> Tally {
        Tally { names: Vec::new(), cases: Vec::new(), bad: Vec::new(), errs: Vec::new() }
    }
    pub(super) fn idx(&mut self, n: &'static str) -> usize {
        if let Some(i) = self.names.iter().position(|&x| x == n) {
            return i;
        }
        self.names.push(n);
        self.cases.push(0);
        self.bad.push(0);
        self.errs.push(0);
        self.names.len() - 1
    }
    pub(super) fn hit(&mut self, n: &'static str, m: Option<String>, ctx: &str) {
        let i = self.idx(n);
        self.cases[i] += 1;
        if let Some(m) = m {
            self.bad[i] += 1;
            if self.bad[i] <= 5 {
                println!("{} {}: {}", n, ctx, m);
            }
        }
    }
    pub(super) fn err(&mut self, n: &'static str) {
        let i = self.idx(n);
        self.errs[i] += 1;
    }
    pub(super) fn total_bad(&self) -> u64 {
        self.bad.iter().sum()
    }
    pub(super) fn print(&self, title: &str) {
        for i in 0..self.names.len() {
            println!("{} {}: {} cases, {} mismatches, {} native E_INVALIDARG (console not run)", title, self.names[i], self.cases[i], self.bad[i], self.errs[i]);
        }
    }
}

struct Ctx<'a> {
    o: &'a Oracle,
    f: Con,
    hl: Vec<bool>,
    u: [[bool; 256]; 3],
    masked: u64,
}

impl<'a> Ctx<'a> {
    fn new(o: &'a Oracle, f: Con, hl: Vec<bool>) -> Ctx<'a> {
        Ctx { o, f, hl, u: [[false; 256]; 3], masked: 0 }
    }
    fn cmp(&mut self, t: &mut Tally, name: &'static str, n: &Dcc, post: &Dcc, ctx: &str) {
        let mut c = copy(post);
        for k in 0..3 {
            for p in 0..256 {
                if self.u[k][p] {
                    c.b[k][p] = n.b[k][p];
                    self.masked += 1;
                }
            }
        }
        t.hit(name, diff(n, &c, &self.hl, true), ctx);
    }
    fn track(&mut self, s: &Dcc, dst: usize, src: usize, maps: &[Option<(u32, u32)>; 4]) {
        let su = self.u[src];
        let mut tu = [true; 64];
        for k in 0..4u32 {
            if (s.r32(0x5c) >> k) & 1 == 0 {
                continue;
            }
            let o = (k * 64) as usize;
            let Some((from, to)) = maps[k as usize] else {
                for p in 0..64 {
                    self.u[dst][o + p] = su[o + p];
                }
                continue;
            };
            for a in o..o + 64 {
                let c = px::get(s, a as u32, from).unwrap();
                let b = (px::set(s, &c, to).unwrap() & 63) as usize;
                tu[b] = su[a];
            }
            for p in 0..64 {
                self.u[dst][o + p] = tu[p];
            }
        }
    }
    fn stage(&mut self, t: &mut Tally, s: &mut Dcc, name: &'static str, rva: usize, nf: fn(&mut Dcc) -> Result<(), i32>, ctx: &str) {
        let mut n = copy(s);
        nf(&mut n).expect("native stage failed after native driver succeeded");
        if rva == 0x23560 {
            for k in 0..4 {
                let (mut a, mut b) = (copy(s), copy(s));
                a.b[2][k * 64..k * 64 + 64].fill(0xaa);
                b.b[2][k * 64..k * 64 + 64].fill(0x55);
                let _ = dec::unpack(&mut a);
                let _ = dec::unpack(&mut b);
                let kept = (k * 64..k * 64 + 64).any(|p| a.b[2][p] == 0xaa && b.b[2][p] == 0x55);
                if !kept {
                    self.u[2][k * 64..k * 64 + 64].fill(false);
                }
            }
        }
        self.f.push(s);
        self.f.call1(rva);
        let post = self.f.pull();
        self.cmp(t, name, &n, &post, ctx);
        *s = post;
        s.m[0x6a] = n.m[0x6a];
        s.m[0x6b] = n.m[0x6b];
    }
    fn remap(&mut self, t: &mut Tally, s: &mut Dcc, name: &'static str, dst: usize, src: usize, gm: Option<u32>, mode: u32, ctx: &str) {
        let mut n = copy(s);
        let from = s.r32(0xf0);
        match gm {
            None => dec::relay(&mut n, dst, src, mode).expect("native relayout failed after native driver succeeded"),
            Some(_) => dec::remap(&mut n, dst, src, &[None; 4]).expect("native convert failed"),
        }
        self.f.push(s);
        let pre = copy(s);
        if gm.is_none() {
            let lay: Lay = unsafe { core::mem::transmute(self.o.h + 0x4ac90) };
            unsafe { lay(self.f.p(), self.f.buf(dst).as_mut_ptr(), self.f.buf(src).as_ptr(), mode) };
            self.track(&pre, dst, src, &[Some((from, mode)); 4]);
        } else {
            let conv: Conv = unsafe { core::mem::transmute(self.o.h + 0x4adc0) };
            let g = [3u32; 4];
            unsafe { conv(self.f.p(), self.f.buf(dst).as_mut_ptr(), self.f.buf(src).as_ptr(), g.as_ptr(), g.as_ptr()) };
            self.track(&pre, dst, src, &[None; 4]);
        }
        let post = self.f.pull();
        self.cmp(t, name, &n, &post, ctx);
        *s = post;
        s.m[0x6a] = n.m[0x6a];
        s.m[0x6b] = n.m[0x6b];
    }
    fn block(&mut self, t: &mut Tally, s: &mut Dcc, input: &[u8; 256], meta: u8, ctx: &str) -> bool {
        s.b[1] = *input;
        s.w32(0x58, meta as u32);
        let start = copy(s);
        let mut nat = copy(s);
        if dec::run(&mut nat).is_err() {
            let c = cause(s);
            t.err(c);
            if c == "color 0x24ec0" {
                t.err(if s.r32(0x4c) >= 2 { "color div0 with p1>=2" } else { "color div0 with p1<2" });
            }
            return false;
        }
        s.w64(0x13c0, 0);
        self.stage(t, s, "unpack 0x23560", 0x23560, dec::unpack, ctx);
        if s.r32(0x58) & 0xf != 0 {
            let al = s.r8(0x12c0) as u32;
            let mut maps = [None; 4];
            for k in 0..4 {
                let g = match (al >> (2 * k)) & 3 {
                    1 => 8,
                    2 => 10,
                    3 => 9,
                    _ => 4,
                };
                if g != 3 {
                    maps[k] = Some((g, 3));
                }
            }
            let mut tr = copy(s);
            if tr.r32(4) == 16 {
                tr.w32(4, 8);
            }
            self.track(&tr, 2, 2, &maps);
            self.stage(t, s, "tid swizzle 0x244a0", 0x244a0, dec::tid_swizzle, ctx);
        } else {
            self.stage(t, s, "tid swizzle 0x244a0", 0x244a0, dec::tid_swizzle, ctx);
        }
        let m = s.r32(0x58) & 0xf != 0;
        if m {
            self.remap(t, s, "debase convert 0x4adc0", 2, 2, Some(3), 3, ctx);
        }
        s.w32(0xf0, 3);
        self.stage(t, s, "dedelta transform 0x247a0", 0x247a0, dec::dedelta_transform, ctx);
        self.stage(t, s, "dedelta calc 0x249d0", 0x249d0, dec::dedelta_calc, ctx);
        if !m {
            s.w32(0xf0, 2);
        } else {
            self.remap(t, s, "relayout mode2 0x4ac90", 2, 2, None, 2, ctx);
        }
        self.stage(t, s, "color 0x24ec0", 0x24ec0, dec::color, ctx);
        if !m {
            s.w32(0xf0, 1);
        } else {
            self.remap(t, s, "relayout mode1 0x4ac90", 2, 2, None, 1, ctx);
        }
        if m {
            self.stage(t, s, "signfix 0x27d10", 0x27d10, dec::signfix, ctx);
        }
        self.stage(t, s, "channel 0x25260", 0x25260, dec::channel, ctx);
        self.stage(t, s, "constant 0x25a80", 0x25a80, dec::constant, ctx);
        if !m {
            s.w32(0xf0, 0);
        } else {
            let saved = s.r32(0x5c);
            let meta = s.r32(0x58);
            let mut mk = saved;
            for k in 0..4u32 {
                let on = (saved >> k) & 1 != 0 && super::bits::sizes_differ(meta, k * 64) == Some(true);
                if on {
                    mk |= 1 << k;
                } else {
                    mk &= !(1 << k);
                }
            }
            s.w32(0x5c, mk);
            self.remap(t, s, "final relayout mode0 0x4ac90", 0, 2, None, 0, ctx);
            s.w32(0x5c, saved);
        }
        let leak = self.u[0].iter().any(|&x| x);
        t.hit("output bytes from console stack", leak.then(|| "b0 holds console-undefined bytes".to_string()), ctx);
        self.cmp(t, "driver 0x23210 vs stage chain", &nat, s, ctx);
        self.f.push(&start);
        self.f.call1(0x23210);
        let post = self.f.pull();
        self.cmp(t, "driver 0x23210", &nat, &post, ctx);
        *s = post;
        s.m[0x6a] = nat.m[0x6a];
        s.m[0x6b] = nat.m[0x6b];
        true
    }
}

fn cause(s: &Dcc) -> &'static str {
    let mut n = copy(s);
    if n.r8(0x6b) & 0x80 == 0 {
        n.w8(0x6b, 0x8f);
        n.w8(0x6a, n.r8(0x6a) | 0xf);
    }
    n.w64(0x13c0, 0);
    let steps: [(&'static str, fn(&mut Dcc) -> Result<(), i32>); 4] = [
        ("unpack 0x23560", dec::unpack),
        ("tid swizzle 0x244a0", dec::tid_swizzle),
        ("dedelta transform 0x247a0", dec::dedelta_transform),
        ("dedelta calc 0x249d0", dec::dedelta_calc),
    ];
    let m = n.r32(0x58) & 0xf != 0;
    for (i, (name, f)) in steps.iter().enumerate() {
        if i == 2 {
            n.w32(0xf0, 3);
        }
        if f(&mut n).is_err() {
            return name;
        }
    }
    if m && dec::relay(&mut n, 2, 2, 2).is_err() {
        return "relayout mode2 0x4ac90";
    }
    if dec::color(&mut n).is_err() {
        return "color 0x24ec0";
    }
    if m && dec::relay(&mut n, 2, 2, 1).is_err() {
        return "relayout mode1 0x4ac90";
    }
    if m && dec::signfix(&mut n).is_err() {
        return "signfix 0x27d10";
    }
    if dec::channel(&mut n).is_err() {
        return "channel 0x25260";
    }
    if dec::constant(&mut n).is_err() {
        return "constant 0x25a80";
    }
    if m {
        let saved = n.r32(0x5c);
        let meta = n.r32(0x58);
        let mut mk = 0u32;
        for k in 0..4u32 {
            if (saved >> k) & 1 != 0 && super::bits::sizes_differ(meta, k * 64) == Some(true) {
                mk |= 1 << k;
            }
        }
        n.w32(0x5c, mk);
        if dec::relay(&mut n, 0, 2, 0).is_err() {
            return "final relayout mode0 0x4ac90";
        }
    }
    "b0 segment never written (console stack bytes)"
}
pub(super) fn patterns(r: &mut Rng, bpe: usize) -> Vec<[u8; 256]> {
    let bpe = bpe.max(1).min(16);
    let mut v = Vec::new();
    let mut el = [0u8; 16];
    for b in el.iter_mut() {
        *b = r.u() as u8;
    }
    let mut c = [0u8; 256];
    for i in 0..256 {
        c[i] = el[i % bpe];
    }
    v.push(c);
    v.push([0u8; 256]);
    let mut g = [0u8; 256];
    let step = (r.below(7) + 1) as u8;
    for i in 0..256 {
        g[i] = (i as u8).wrapping_mul(step).wrapping_add(el[i % bpe] & 0x0f);
    }
    v.push(g);
    let mut n = [0u8; 256];
    for x in n.iter_mut() {
        *x = r.u() as u8;
    }
    v.push(n);
    let mut p = c;
    for i in 0..256 {
        if (i / 64) % 2 == 1 || r.below(8) == 0 {
            p[i] = r.u() as u8;
        }
    }
    v.push(p);
    let mut s = [0u8; 256];
    for i in 0..256 {
        s[i] = el[i % bpe].wrapping_add(((i / bpe) as u8) & 3);
    }
    v.push(s);
    v
}

pub(super) fn valid_metas() -> Vec<u8> {
    (0..256u32).filter(|&m| valid_meta(m)).map(|m| m as u8).collect()
}

pub(super) fn states() -> Vec<((u32, u32, u32, u32, u32), usize)> {
    let mut seen: std::collections::HashMap<Vec<u8>, usize> = std::collections::HashMap::new();
    let mut out: Vec<((u32, u32, u32, u32, u32), usize)> = Vec::new();
    for (f, sw, dim, p0, _) in configs() {
        for p1 in P1S {
            let mut d = Dcc::new();
            if d.init(f, sw, dim, p0, p1).is_err() {
                continue;
            }
            let key = d.m.clone();
            match seen.get(&key) {
                Some(&i) => out[i].1 += 1,
                None => {
                    seen.insert(key, out.len());
                    out.push(((f, sw, dim, p0, p1), 1));
                }
            }
        }
    }
    out
}

pub(super) fn encode(o: &Oracle, inp: &[u8; 256], f: u32, sw: u32, dim: u32, p0: u32, p1: u32, bpe: u32) -> Option<([u8; 256], u8)> {
    let enc: Enc = unsafe { core::mem::transmute(o.f("XGEncodeDCC")) };
    let p1 = if bpe < 4 && p1 >= 2 { 0 } else { p1 };
    let prm = [p0, p1, 0, 0];
    let mut out = [0u8; 256];
    let mut meta = 0u8;
    let mut sz = 0u32;
    let r = unsafe { enc(1, inp.as_ptr(), out.as_mut_ptr(), &mut meta, &mut sz, f, sw, dim, prm.as_ptr()) };
    if r < 0 {
        return None;
    }
    Some((out, meta))
}

pub(super) fn defined(mut s: Dcc) -> Dcc {
    s.m[0x6a] = 0;
    s.m[0x6b] = 0x80;
    s
}

#[inline(never)]
pub(super) fn smear(v: u8) -> u8 {
    let mut a = [0u8; 0x4000];
    for x in a.iter_mut() {
        unsafe { core::ptr::write_volatile(x, v) };
    }
    unsafe { core::ptr::read_volatile(&a[0x1234]) }
}

#[test]
fn oracle_dcc_relayout_stack_residue() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let lay: Lay = unsafe { core::mem::transmute(o.h + 0x4ac90) };
    let c = Con::new(&o, 1, 1, 0, 0, 0).unwrap();
    let mut d = c.pull();
    for j in 0..256 {
        d.b[2][j] = 0x11;
        d.b[0][j] = 0x22;
    }
    d.w32(0x5c, 1);
    d.w32(0xf0, 3);
    let mut outs = Vec::new();
    for v in [0xa5u8, 0x5a] {
        c.push(&d);
        let f: unsafe extern "system" fn(*mut u8, *mut u8, *const u8, u32) = lay;
        let p = c.p();
        let dst = c.buf(0).as_mut_ptr();
        let src = c.buf(2).as_ptr();
        let _ = smear(v);
        unsafe { f(p, dst, src, 2) };
        let out = c.pull();
        println!("probe fill {:#x}: b0[0..64] = {:02x?}", v, &out.b[0][..64]);
        outs.push(out.b[0]);
    }
    assert_ne!(outs[0], outs[1]);
    let mut n = copy(&d);
    assert_eq!(dec::relay(&mut n, 0, 2, 2), Err(crate::layout::E_INVALIDARG));
}

#[test]
fn oracle_dcc_dec_helpers() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let wd: Wd = unsafe { core::mem::transmute(o.h + 0x31410) };
    let lo: Clr = unsafe { core::mem::transmute(o.h + 0x31770) };
    let hi: Clr = unsafe { core::mem::transmute(o.h + 0x31820) };
    let mut t = Tally::new();
    let mut d = Dcc::new();
    let ws: Vec<u8> = (0..=0x41u8).chain([0x60, 0x7f, 0x80, 0xc0, 0xff]).collect();
    for v48 in (0..18u32).chain([0xffff_ffff, 0x80]) {
        for bpe in [1u32, 2, 3, 4, 8, 16, 32] {
            for c8 in 0..6u32 {
                for cc in 0..9u32 {
                    for flags in 0..8u8 {
                        d.w32(0x48, v48);
                        d.w32(4, bpe);
                        d.w32(8, c8);
                        d.w32(0xc, cc);
                        d.w8(0x10, [0u8, 1, 2, 0][(flags & 3) as usize]);
                        d.w8(0x11, (flags >> 2) & 1);
                        d.w8(0x12, (flags >> 1) & 1);
                        let p = d.m.as_ptr();
                        let a = unsafe { wd(p, p.add(0x20)) };
                        let ctx = format!("v48={} bpe={} comps={} c={} flags={}", v48, bpe, c8, cc, flags);
                        let b = dec::widths(&d);
                        t.hit("widths 0x31410", (a != b).then(|| format!("console {:#x} native {:#x}", a, b)), &ctx);
                        for &w in &ws {
                            for h in [false, true] {
                                let a = unsafe { lo(p, p.add(0x20), w, h as u8) };
                                let b = dec::clear_lo(&d, w, h);
                                t.hit("clear lo 0x31770", (a != b).then(|| format!("w={} hi={} console {:#x} native {:#x}", w, h, a, b)), &ctx);
                                let a = unsafe { hi(p, p.add(0x20), w, h as u8) };
                                let b = dec::clear_hi(&d, w, h);
                                t.hit("clear hi 0x31820", (a != b).then(|| format!("w={} hi={} console {:#x} native {:#x}", w, h, a, b)), &ctx);
                            }
                        }
                    }
                }
            }
        }
    }
    t.print("dcc dec helpers");
    assert_eq!(t.total_bad(), 0);
}

#[test]
fn oracle_dcc_dec_stages() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let hl = holes(&o);
    let metas = valid_metas();
    let st = states();
    let covered: usize = st.iter().map(|x| x.1).sum();
    let mut r = Rng(0x5eed_dec0_1234);
    let mut t = Tally::new();
    let mut blocks = 0u64;
    let mut aborted = 0u64;
    let mut flagged = 0u64;
    let mut cx_masked = 0u64;
    for &((f, sw, dim, p0, p1), _) in &st {
        let Some(c) = Con::new(&o, f, sw, dim, p0, p1) else { continue };
        let Some(fresh) = Con::new(&o, f, sw, dim, p0, p1) else { continue };
        let mut cx = Ctx::new(&o, fresh, hl.clone());
        let mut s = defined(c.pull());
        let bpe = s.r32(4);
        let mut work: Vec<([u8; 256], u8, &'static str)> = Vec::new();
        for p in patterns(&mut r, bpe as usize) {
            if let Some((comp, meta)) = encode(&o, &p, f, sw, dim, p0, p1, bpe) {
                work.push((comp, meta, "encoded"));
            }
        }
        for _ in 0..6 {
            let mut b = [0u8; 256];
            for x in b.iter_mut() {
                *x = r.u() as u8;
            }
            if r.below(3) == 0 {
                for x in b.iter_mut().skip(r.below(256) as usize) {
                    *x = 0;
                }
            }
            work.push((b, metas[r.below(metas.len() as u32) as usize], "random"));
        }
        let mut cb = [0u8; 256];
        for x in cb.iter_mut() {
            *x = r.u() as u8;
        }
        for m in [0x00u8, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0, 0xf0] {
            if r.below(3) == 0 {
                work.push((cb, m, "const-meta"));
            }
        }
        for (b, meta, kind) in work {
            blocks += 1;
            let ctx = format!("f={} sw={} dim={} p0={} p1={:#x} meta={:#04x} {}", f, sw, dim, p0, p1, meta, kind);
            if !cx.block(&mut t, &mut s, &b, meta, &ctx) {
                aborted += 1;
                s = defined(c.pull());
                cx.u = [[false; 256]; 3];
                continue;
            }
            if s.r64(0x13c0) != 0 {
                flagged += 1;
            }
        }
        cx_masked += cx.masked;
    }
    t.print("dcc dec stage");
    println!("dcc dec stages: {} unique states covering {} configs, {} blocks, {} aborted by native E_INVALIDARG, {} with console error flag, {} console-undefined bytes masked", st.len(), covered, blocks, aborted, flagged, cx_masked);
    assert_eq!(t.total_bad(), 0);
}

#[test]
fn oracle_dcc_dec_e2e() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let cdec: DecF = unsafe { core::mem::transmute(o.f("XGDecodeDCC")) };
    let metas = valid_metas();
    let bad_metas: Vec<u8> = (0..256u32).filter(|&m| !valid_meta(m)).map(|m| m as u8).collect();
    let st = states();
    let mut r = Rng(0xe2e_dcc_77);
    let (mut calls, mut blocks, mut bad, mut nerr, mut inval) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut causes: std::collections::BTreeMap<&'static str, u64> = std::collections::BTreeMap::new();
    let mut extra: Vec<(u32, u32, u32, u32, u32)> = configs();
    extra.retain(|_| r.below(40) == 0);
    let mut cfgs: Vec<(u32, u32, u32, u32, u32)> = st.iter().map(|x| x.0).collect();
    for (f, sw, dim, p0, _) in extra {
        cfgs.push((f, sw, dim, p0, P1S[r.below(5) as usize]));
    }
    for (f, sw, dim, p0, p1) in cfgs {
        let mut d = Dcc::new();
        if d.init(f, sw, dim, p0, p1).is_err() {
            continue;
        }
        let bpe = d.r32(4);
        let pats = patterns(&mut r, bpe as usize);
        let prm = [p0, p1, 0, 0];
        for count in [1usize, 3, 8] {
            let mut src = vec![0u8; 256 * count];
            let mut meta = vec![0u8; count];
            for i in 0..count {
                let kind = r.below(3);
                let mut blk = [0u8; 256];
                let mut m = metas[r.below(metas.len() as u32) as usize];
                if kind == 0 {
                    let p = pats[r.below(pats.len() as u32) as usize];
                    if let Some((c, mm)) = encode(&o, &p, f, sw, dim, p0, p1, bpe) {
                        blk = c;
                        m = mm;
                    }
                } else {
                    for x in blk.iter_mut() {
                        *x = r.u() as u8;
                    }
                    if kind == 2 {
                        m &= 0xf0;
                    }
                }
                src[i * 256..i * 256 + 256].copy_from_slice(&blk);
                meta[i] = m;
            }
            let mut nd = vec![0u8; 256 * count];
            let nr = crate::dcc::decode(count as u32, &src, &meta, &mut nd, f, sw, dim, p0, p1);
            if nr != 0 {
                nerr += 1;
                assert_eq!(nr, E_INVALIDARG);
                let mut d = Dcc::new();
                d.init(f, sw, dim, p0, p1).unwrap();
                for i in 0..count {
                    d.b[1].copy_from_slice(&src[i * 256..i * 256 + 256]);
                    d.w32(0x58, meta[i] as u32);
                    let pre = copy(&d);
                    if dec::run(&mut d).is_err() {
                        let c = cause(&pre);
                        let c = if c == "color 0x24ec0" { if p1 >= 2 { "color div0 with p1>=2" } else { "color div0 with p1<2" } } else { c };
                        *causes.entry(c).or_insert(0u64) += 1;
                        break;
                    }
                }
                continue;
            }
            let mut cd = vec![0u8; 256 * count];
            let cr = unsafe { cdec(count as u32, src.as_ptr(), meta.as_ptr(), cd.as_mut_ptr(), f, sw, dim, prm.as_ptr()) };
            calls += 1;
            blocks += count as u64;
            if cr != 0 || cd != nd {
                bad += 1;
                if bad <= 10 {
                    let j = (0..cd.len()).find(|&j| cd[j] != nd[j]);
                    println!("e2e f={} sw={} dim={} p0={} p1={:#x} count={} metas={:02x?}: console hr={:#x} first diff {:?}", f, sw, dim, p0, p1, count, meta, cr, j.map(|j| (j, nd[j], cd[j])));
                }
            }
        }
        for _ in 0..2 {
            let m = bad_metas[r.below(bad_metas.len() as u32) as usize];
            let src = [0u8; 256];
            let mut nd = [0u8; 256];
            let nr = crate::dcc::decode(1, &src, &[m], &mut nd, f, sw, dim, p0, p1);
            inval += 1;
            assert_eq!(nr, E_INVALIDARG, "invalid meta {:#x} accepted", m);
        }
    }
    println!("dcc dec e2e: {} XGDecodeDCC calls, {} blocks, {} mismatches, {} native E_INVALIDARG skips, {} invalid-meta calls all E_INVALIDARG", calls, blocks, bad, nerr, inval);
    for (c, n) in &causes {
        println!("dcc dec e2e native E_INVALIDARG cause {}: {}", c, n);
    }
    assert_eq!(bad, 0);
}

fn color_faults(d: &Dcc) -> bool {
    let meta = d.r32(0x58);
    if meta & 0xf == 0 {
        return false;
    }
    let mut r15 = d.r32(0x4c);
    if r15 == 0 {
        if d.r32(8).wrapping_sub(1) <= 1 {
            return false;
        }
        let v = d.i32(0x48);
        if v > 0 && v <= 0xb {
            r15 = 3;
        } else if v > 0 && v <= 0xf {
            return false;
        } else {
            r15 = if d.r8(0x10) != 0 { 2 } else { 3 };
        }
    }
    if r15 == 1 || d.r32(4) >> 2 != 0 {
        return false;
    }
    let Some(segs) = super::bits::seg_info_checked(meta) else { return false };
    let old = d.r32(0x5c);
    let mut m5c = 0u32;
    for i in 0..4 {
        if (old >> i) & 1 != 0 {
            m5c |= segs[i];
        }
    }
    (0..8u32).any(|k| (m5c >> (k >> 1)) & 1 != 0 && super::bits::sizes_differ(meta, k * 32) == Some(true))
}

#[inline(never)]
fn only_tid(c: &Con) {
    c.call1(0x244a0);
}

fn tid_holes(s: &Dcc) -> Vec<usize> {
    let mut t = copy(s);
    if t.r32(4) == 16 {
        t.w32(4, 8);
    }
    let al = t.r8(0x12c0) as u32;
    let mut v = Vec::new();
    for k in 0..4u32 {
        if (t.r32(0x5c) >> k) & 1 == 0 || super::bits::sizes_differ(t.r32(0x58), k * 64) != Some(true) {
            continue;
        }
        let g = match (al >> (2 * k)) & 3 {
            1 => 8,
            2 => 10,
            3 => 9,
            _ => 4,
        };
        let mut ever = [false; 64];
        for a in k * 64..k * 64 + 64 {
            let c = px::get(&t, a, g).unwrap();
            ever[(px::set(&t, &c, 3).unwrap() & 63) as usize] = true;
        }
        for b in 0..64 {
            if !ever[b] {
                v.push((k * 64) as usize + b);
            }
        }
    }
    v
}

#[inline(never)]
fn drive(c: &Con, d: &Dcc, v: u8) -> Dcc {
    let mut s = copy(d);
    s.b[0] = [v; 256];
    s.b[2] = [v; 256];
    c.push(&s);
    let _ = smear(v);
    c.call1(0x23210);
    c.pull()
}

#[test]
fn oracle_dcc_dec_undefined_classes() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let metas = valid_metas();
    let st = states();
    let mut r = Rng(0x0dd_c1a5);
    let mut t: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let mut add = |k: String| *t.entry(k).or_insert(0) += 1;
    let mut unknown = 0u64;
    let mut not_undefined = 0u64;
    for &((f, sw, dim, p0, p1), _) in &st {
        let Some(con) = Con::new(&o, f, sw, dim, p0, p1) else { continue };
        let bpe = con.pull().r32(4);
        let mut work: Vec<([u8; 256], u8, &'static str)> = Vec::new();
        for p in patterns(&mut r, bpe as usize) {
            if let Some((comp, meta)) = encode(&o, &p, f, sw, dim, p0, p1, bpe) {
                work.push((comp, meta, "encoded"));
            }
        }
        for _ in 0..4 {
            let mut b = [0u8; 256];
            for x in b.iter_mut() {
                *x = r.u() as u8;
            }
            work.push((b, metas[r.below(metas.len() as u32) as usize], "random"));
        }
        for (b, m, kind) in work {
            let mut nd = [0u8; 256];
            if crate::dcc::decode(1, &b, &[m], &mut nd, f, sw, dim, p0, p1) == 0 {
                add(format!("accepted [{}]", kind));
                continue;
            }
            let mut d = Dcc::new();
            d.init(f, sw, dim, p0, p1).unwrap();
            d.b[1] = b;
            d.w32(0x58, m as u32);
            if color_faults(&d) {
                add(format!("console #DE in color, not run [{}] bpe={}", kind, bpe));
                continue;
            }
            let c = cause(&d);
            if c == "tid swizzle 0x244a0" || c.starts_with("b0 segment") {
                let mut s = copy(&d);
                s.w8(0x6a, 0);
                s.w8(0x6b, 0x80);
                con.push(&s);
                con.call1(0x23560);
                let s1 = con.pull();
                let h = tid_holes(&s1);
                let sub;
                let proven;
                if c == "tid swizzle 0x244a0" && !h.is_empty() {
                    sub = "tid swizzle convert leaves scratch holes, filled from caller stack";
                    let mut ok = true;
                    for v in [0x11u8, 0x22] {
                        con.push(&s1);
                        let _ = smear(v);
                        only_tid(&con);
                        let a = con.pull();
                        ok &= h.iter().all(|&i| a.b[2][i] == v);
                    }
                    proven = ok;
                } else {
                    sub = if c.starts_with("b0") { "output segment never written, keeps prior buffer bytes" } else { "tid swizzle consumes b2 segment never written this call" };
                    let a = drive(&con, &d, 0x11);
                    let z = drive(&con, &d, 0x22);
                    proven = a.b[0] != z.b[0];
                }
                if !proven {
                    not_undefined += 1;
                    if not_undefined <= 6 {
                        println!("unproven {} f={} sw={} dim={} p0={} p1={:#x} meta={:#04x} bpe={} holes={}", sub, f, sw, dim, p0, p1, m, bpe, h.len());
                    }
                }
                add(format!("{} [{}] bpe={}{}", sub, kind, bpe, if proven { "" } else { " UNPROVEN" }));
                continue;
            }            unknown += 1;
            add(format!("UNCLASSIFIED {} [{}]", c, kind));
        }
    }
    for (k, n) in &t {
        println!("dcc dec class {}: {}", k, n);
    }
    println!("dcc dec undefined classes: {} unclassified, {} rejected blocks whose console output did not depend on uninitialised stack/buffer bytes", unknown, not_undefined);
    assert_eq!(unknown, 0);
    assert_eq!(not_undefined, 0);
}
#[test]
fn oracle_dcc_decode_export_shim() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let cdec: DecF = unsafe { core::mem::transmute(o.f("XGDecodeDCC")) };
    let mut r = Rng(0x5141_dec);
    let mut cases = 0u64;
    let src = [0u8; 256 * 2];
    let meta = [0x10u8, 0x00];
    let prm = [0u32, 0, 0, 0];
    let mut a = [0u8; 512];
    let mut b = [0u8; 512];
    let nul = core::ptr::null::<u8>();
    let call = |n: bool, cnt: u32, s: *const u8, m: *const u8, d: *mut u8, f: u32, sw: u32, dim: u32, p: *const u32| -> i32 {
        unsafe { if n { crate::XGDecodeDCC(cnt, s, m, d, f, sw, dim, p) } else { cdec(cnt, s, m, d, f, sw, dim, p) } }
    };
    for (s, m, pp) in [(nul, meta.as_ptr(), prm.as_ptr()), (src.as_ptr(), nul, prm.as_ptr()), (src.as_ptr(), meta.as_ptr(), core::ptr::null())] {
        cases += 1;
        assert_eq!(call(true, 1, s, m, a.as_mut_ptr(), 28, 1, 0, pp), call(false, 1, s, m, b.as_mut_ptr(), 28, 1, 0, pp));
    }
    cases += 1;
    assert_eq!(call(true, 1, src.as_ptr(), meta.as_ptr(), core::ptr::null_mut(), 28, 1, 0, prm.as_ptr()), call(false, 1, src.as_ptr(), meta.as_ptr(), core::ptr::null_mut(), 28, 1, 0, prm.as_ptr()));
    for f in [0u32, 0x79, 0x7a, 0x100, 0x7fff_ffff] {
        for sw in [0u32, 1, 0x1f, 0x20, 0x7fff_ffff] {
            cases += 1;
            let x = call(true, 1, src.as_ptr(), meta.as_ptr(), a.as_mut_ptr(), f, sw, 0, prm.as_ptr());
            let y = call(false, 1, src.as_ptr(), meta.as_ptr(), b.as_mut_ptr(), f, sw, 0, prm.as_ptr());
            assert_eq!(x, y, "f={:#x} sw={:#x}", f, sw);
        }
    }
    let mut asserts = 0u64;
    for f in 1..=0x78u32 {
        if !crate::dcc::safe_format(f) {
            for cnt in [0u32, 1] {
                asserts += 1;
                assert_eq!(call(true, cnt, src.as_ptr(), meta.as_ptr(), a.as_mut_ptr(), f, 1, 0, prm.as_ptr()), E_INVALIDARG);
            }
            continue;
        }
        for sw in [0u32, 1, 2, 3, 0x10, 0x1f, 0xffff_fffe, 0x8000_0000] {
            for dim in [0u32, 3, 4, 9] {
                let p = [r.below(3), [0u32, 1, 3][r.below(3) as usize], 0, 0];
                for cnt in [0u32, 1, 2] {
                    let mut s = [0u8; 512];
                    for x in s.iter_mut() {
                        *x = r.u() as u8;
                    }
                    let m = [[0x00u8, 0x10, 0x20, 0x30, 0x50][r.below(5) as usize], 0x10];
                    a = [0x5a; 512];
                    b = [0x5a; 512];
                    let x = call(true, cnt, s.as_ptr(), m.as_ptr(), a.as_mut_ptr(), f, sw, dim, p.as_ptr());
                    let y = call(false, cnt, s.as_ptr(), m.as_ptr(), b.as_mut_ptr(), f, sw, dim, p.as_ptr());
                    cases += 1;
                    assert_eq!(x, y, "hr f={} sw={:#x} dim={} p={:?} cnt={}", f, sw, dim, p, cnt);
                    if x == 0 {
                        assert_eq!(a[..], b[..], "bytes f={} sw={:#x} dim={} p={:?} cnt={} meta={:02x?}", f, sw, dim, p, cnt, m);
                    }
                }
            }
        }
    }
    println!("dcc decode export shim: {} cases match console (hr and bytes), {} unsupported-format calls return E_INVALIDARG where the console hits int3 at 0xa894 then returns E_INVALIDARG", cases, asserts);
}

fn de_cases(n: usize) -> Vec<((u32, u32, u32, u32, u32), [u8; 256], u8)> {
    let metas = valid_metas();
    let mut r = Rng(0xde_0094);
    let mut v = Vec::new();
    for &((f, sw, dim, p0, p1), _) in &states() {
        let mut d = Dcc::new();
        if d.init(f, sw, dim, p0, p1).is_err() {
            continue;
        }
        let mut b = [0u8; 256];
        for x in b.iter_mut() {
            *x = r.u() as u8;
        }
        let m = metas[r.below(metas.len() as u32) as usize];
        d.b[1] = b;
        d.w32(0x58, m as u32);
        if color_faults(&d) && cause(&d) == "color 0x24ec0" {
            v.push(((f, sw, dim, p0, p1), b, m));
            if v.len() >= n {
                break;
            }
        }
    }
    v
}

#[test]
fn oracle_dcc_dec_color_fault_child() {
    if let Ok(i) = std::env::var("DCC_DE_CASE") {
        let Some(o) = oracle() else { std::process::exit(3) };
        let cdec: DecF = unsafe { core::mem::transmute(o.f("XGDecodeDCC")) };
        let ((f, sw, dim, p0, p1), b, m) = de_cases(64)[i.parse::<usize>().unwrap()];
        let prm = [p0, p1, 0, 0];
        let mut cd = [0u8; 256];
        unsafe { cdec(1, b.as_ptr(), [m].as_ptr(), cd.as_mut_ptr(), f, sw, dim, prm.as_ptr()) };
        std::process::exit(0);
    }
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(_o) = oracle() else { return };
    let cases = de_cases(24);
    assert!(cases.len() >= 20);
    let exe = std::env::current_exe().unwrap();
    let mut faults = 0;
    for i in 0..cases.len() {
        let st = std::process::Command::new(&exe)
            .args(["dcc::oracle_dec::oracle_dcc_dec_color_fault_child", "--exact", "--test-threads=1"])
            .env("DCC_DE_CASE", i.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        let code = st.code().unwrap_or(0) as u32;
        assert_eq!(code, 0xc000_0094, "case {} {:?} exit {:#x}", i, cases[i].0, code);
        faults += 1;
    }
    println!("dcc dec color #DE: {} predicted cases, console XGDecodeDCC raised STATUS_INTEGER_DIVIDE_BY_ZERO in {} child processes", cases.len(), faults);
}