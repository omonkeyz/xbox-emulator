use crate::dcc::{self, Dcc, OBJ, PTRS, SURF};
use crate::oracle::{oracle, Oracle, SERIAL};

pub(crate) struct Con {
    pub(crate) mem: Vec<u64>,
    pub(crate) h: usize,
}

pub(crate) type F1 = unsafe extern "system" fn(*mut u8) -> u64;
type Ctor = unsafe extern "system" fn(*mut u8) -> *mut u8;
type Init = unsafe extern "system" fn(*mut u8, u32, u32, u32, *const u32) -> i32;

impl Con {
    pub(crate) fn p(&self) -> *mut u8 {
        self.mem.as_ptr() as *mut u8
    }
    pub(crate) fn raw(o: &Oracle, fill: u8) -> Con {
        let mut c = Con { mem: vec![u64::from_ne_bytes([fill; 8]); (OBJ + 0x100) / 8], h: o.h };
        unsafe {
            let f: Ctor = core::mem::transmute(o.h + 0x2cdc0);
            f(c.p());
        }
        let _ = &mut c;
        c
    }
    pub(crate) fn init(&mut self, fmt: u32, sw: u32, dim: u32, p0: u32, p1: u32) -> i32 {
        let prm = [p0, p1, 0, 0];
        unsafe {
            let f: Init = core::mem::transmute(self.h + 0xa600);
            f(self.p(), fmt, sw, dim, prm.as_ptr())
        }
    }
    pub(crate) fn new(o: &Oracle, fmt: u32, sw: u32, dim: u32, p0: u32, p1: u32) -> Option<Con> {
        let mut c = Con::raw(o, 0);
        if c.init(fmt, sw, dim, p0, p1) != 0 {
            return None;
        }
        Some(c)
    }
    pub(crate) fn bytes(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.p(), OBJ) }
    }
    pub(crate) fn buf(&self, k: usize) -> &mut [u8] {
        let m = self.bytes();
        let a = u64::from_le_bytes(m[SURF[k] + 0x20..SURF[k] + 0x28].try_into().unwrap()) as usize;
        unsafe { core::slice::from_raw_parts_mut(a as *mut u8, 256) }
    }
    pub(crate) fn push(&self, d: &Dcc) {
        let m = self.bytes();
        let keep: Vec<(usize, [u8; 8])> = PTRS.iter().map(|&o| (o, m[o..o + 8].try_into().unwrap())).collect();
        m.copy_from_slice(&d.m);
        for (o, v) in keep {
            m[o..o + 8].copy_from_slice(&v);
        }
        let base = self.p() as u64;
        let e = if d.r64(0x13c0) != 0 { base + 0x13c8 } else { 0 };
        m[0x13c0..0x13c8].copy_from_slice(&e.to_le_bytes());
        for k in 0..3 {
            self.buf(k).copy_from_slice(&d.b[k]);
        }
    }
    pub(crate) fn pull(&self) -> Dcc {
        let mut d = Dcc::new();
        d.m.copy_from_slice(self.bytes());
        for &o in PTRS.iter() {
            d.w64(o, 0);
        }
        let e = u64::from_le_bytes(self.bytes()[0x13c0..0x13c8].try_into().unwrap());
        d.w64(0x13c0, (e != 0) as u64);
        for k in 0..3 {
            d.b[k].copy_from_slice(self.buf(k));
        }
        d
    }
    pub(crate) fn call2(&self, rva: usize, a: u32) -> u64 {
        unsafe {
            let f: unsafe extern "system" fn(*mut u8, u32) -> u64 = core::mem::transmute(self.h + rva);
            f(self.p(), a)
        }
    }
    pub(crate) fn call1(&self, rva: usize) -> u64 {
        unsafe {
            let f: F1 = core::mem::transmute(self.h + rva);
            f(self.p())
        }
    }
}

pub(crate) fn holes(o: &Oracle) -> Vec<bool> {
    let a = Con::raw(o, 0xaa);
    let b = Con::raw(o, 0x55);
    let (x, y) = (a.bytes(), b.bytes());
    (0..OBJ).map(|i| x[i] == 0xaa && y[i] == 0x55).collect()
}

pub(crate) fn diff(a: &Dcc, b: &Dcc, holes: &[bool], bufs: bool) -> Option<String> {
    for i in 0..OBJ {
        if holes[i] || PTRS.iter().any(|&p| i >= p && i < p + 8 && p != 0x13c0) {
            continue;
        }
        if a.m[i] != b.m[i] {
            return Some(format!("obj+{:#x}: native {:#04x} console {:#04x}", i, a.m[i], b.m[i]));
        }
    }
    for k in 0..3 {
        if !bufs {
            break;
        }
        if let Some(j) = (0..256).find(|&j| a.b[k][j] != b.b[k][j]) {
            return Some(format!("buf{} +{:#x}: native {:#04x} console {:#04x}", k, j, a.b[k][j], b.b[k][j]));
        }
    }
    None
}

pub(crate) fn safe_formats() -> Vec<u32> {
    (1..=0x78).filter(|&f| dcc::safe_format(f)).collect()
}

pub(crate) struct Rng(pub(crate) u64);
impl Rng {
    pub(crate) fn u(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    pub(crate) fn below(&mut self, n: u32) -> u32 {
        self.u() % n
    }
}

pub(crate) fn configs() -> Vec<(u32, u32, u32, u32, u32)> {
    let mut v = Vec::new();
    for f in safe_formats() {
        for sw in 0..32 {
            for dim in 0..6 {
                for p0 in 0..2 {
                    v.push((f, sw, dim, p0, 0));
                }
            }
        }
    }
    v
}

#[test]
fn oracle_dcc_init() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let hl = holes(&o);
    let nh = hl.iter().filter(|&&x| x).count();
    let mut cases = 0u64;
    let mut bad = 0u64;
    let mut p1s = vec![0u32, 1, 2, 3, 7, 0x20, 0xffff_ffff];
    let p0s = [0u32, 1, 2, 3, 5, 31, 32, 0xffff_ffff];
    for f in 0..=0x80u32 {
        for sw in [0u32, 1, 2, 3, 4, 7, 0x10, 0x1b, 0x1f, 0x20, 0xffff_ffff] {
            for dim in 0..9u32 {
                for &p0 in p0s.iter() {
                    for &p1 in p1s.iter() {
                        if !dcc::args_ok(f, sw) {
                            continue;
                        }
                        let mut n = Dcc::new();
                        let rn = n.init(f, sw, dim, p0, p1);
                        if rn.is_err() {
                            cases += 1;
                            if dcc::args_ok(f, sw) && dcc::safe_format(f) {
                                bad += 1;
                            }
                            continue;
                        }
                        let c = Con::new(&o, f, sw, dim, p0, p1).expect("console init failed");
                        cases += 1;
                        if let Some(m) = diff(&n, &c.pull(), &hl, false) {
                            bad += 1;
                            if bad < 10 {
                                println!("init f={} sw={} dim={} p0={} p1={}: {}", f, sw, dim, p0, p1, m);
                            }
                        }
                    }
                }
            }
        }
        p1s.rotate_left(1);
    }
    println!("dcc init: {} cases, {} mismatches, {} ctor holes", cases, bad, nh);
    assert_eq!(bad, 0);
}

#[test]
fn oracle_dcc_bits() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    type Seg = unsafe extern "system" fn(*const u32, *mut u32) -> *mut u32;
    type Cmp = unsafe extern "system" fn(*const u32, u32) -> u8;
    type Msk = unsafe extern "system" fn(u32) -> u32;
    let seg: Seg = unsafe { core::mem::transmute(o.h + 0x4af40) };
    let cmp: Cmp = unsafe { core::mem::transmute(o.h + 0x4b120) };
    let msk: Msk = unsafe { core::mem::transmute(o.h + 0x230f0) };
    let mut cases = 0u64;
    let mut bad = 0u64;
    for n in 0..=32u32 {
        cases += 1;
        if unsafe { msk(n) } != crate::dcc::bits::mask(n) {
            bad += 1;
        }
    }
    let mut invalid = 0;
    for meta in 0..256u32 {
        let Some(nat) = crate::dcc::bits::seg_info_checked(meta) else {
            invalid += 1;
            continue;
        };
        let mut out = [0u32; 16];
        unsafe { seg(&meta, out.as_mut_ptr()) };
        cases += 1;
        if out[..8] != nat[..] || out[8..].iter().any(|&x| x != 0) {
            bad += 1;
            println!("seg meta={:#x} native {:x?} console {:x?}", meta, nat, out);
        }
        for off in (0..256u32).step_by(1) {
            cases += 1;
            let c = unsafe { cmp(&meta, off) } & 1 != 0;
            if crate::dcc::bits::sizes_differ(meta, off) != Some(c) {
                bad += 1;
                if bad < 20 {
                    println!("cmp meta={:#x} off={:#x} console {}", meta, off, c);
                }
            }
        }
    }
    println!("dcc bits: {} cases, {} mismatches, {} overflow metas", cases, bad, invalid);
    assert_eq!(bad, 0);
}
