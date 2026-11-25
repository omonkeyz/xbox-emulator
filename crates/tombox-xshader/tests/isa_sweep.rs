#![cfg(windows)]
#![allow(dead_code)]

#[path = "../../tools/xreplay/src/d3d.rs"]
mod d3d;
#[path = "isa_sweep/gpu.rs"]
mod gpu;
#[path = "isa_sweep/fp.rs"]
pub mod fp;
#[path = "isa_sweep/enc.rs"]
pub mod enc;
#[path = "isa_sweep/ops_v.rs"]
mod ops_v;
#[path = "isa_sweep/ops_s.rs"]
mod ops_s;
#[path = "isa_sweep/ops_c.rs"]
mod ops_c;
#[path = "isa_sweep/ops_x.rs"]
mod ops_x;
#[path = "isa_sweep/rdna2_sweep_hw.rs"]
mod rdna2_sweep_hw;

use enc::*;
use fp::{nan16, nan32, nan64};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use tombox_xshader::meta::{RootParam, RootParamKind, RootRange, RootSig};
use tombox_xshader::{dxc, hlsl::translate, ShaderExtract, Stage};

pub const LANES: usize = 256;
pub const BATCHES: usize = 4;
pub const REC: usize = 128;
pub const SLOTS: usize = 24;
pub const SHIFT: u32 = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    U32,
    Small,
    F32,
    F32Cls,
    Trans,
    Trans16,
    F16,
    F64,
    U64,
    Mixed,
}

#[derive(Clone, Copy, Debug)]
pub enum Cmp {
    X,
    F32(u32),
    F32Tol(u32, f32),
    F16(u32),
    F16Tol(u32, f32),
    F16H(u32),
    Pk16(u32),
    F64(u64),
    F64Hi,
}

pub struct In {
    pub d: [u32; 8],
    pub lane: u32,
}

pub type Ref = Box<dyn Fn(&In) -> Vec<Option<u32>>>;

pub struct T {
    pub m: &'static str,
    pub fam: &'static str,
    pub variant: String,
    pub class: Class,
    pub code: Vec<u32>,
    pub at: usize,
    pub cmp: Vec<Cmp>,
    pub r: Ref,
}

pub struct Suite {
    pub t: Vec<T>,
    pub skipped: Vec<(&'static str, &'static str, &'static str)>,
}

impl Suite {
    pub fn add(&mut self, m: &'static str, fam: &'static str, variant: &str, class: Class, pre: &[u32], op: &[u32], post: &[u32], cmp: &[Cmp], r: impl Fn(&In) -> Vec<Option<u32>> + 'static) {
        let mut code = pre.to_vec();
        let all: Vec<u32> = pre.iter().chain(op).chain(post).copied().collect();
        let isa: Vec<u8> = all.iter().flat_map(|w| w.to_le_bytes()).collect();
        for ins in tombox_xshader::rdna2::disassemble(&isa) {
            for d in &ins.dst {
                let bad = match d {
                    tombox_xshader::rdna2::Opnd::Vgpr(n) => matches!(n, 0 | 1 | 10..=17 | 31),
                    tombox_xshader::rdna2::Opnd::Sgpr(n) => matches!(n, 0..=5 | 10..=17),
                    _ => false,
                };
                assert!(!bad, "{m} [{variant}] writes reserved register {d:?} in {}", ins.mnemonic);
            }
        }
        let at = code.len();
        code.extend_from_slice(op);
        code.extend_from_slice(post);
        self.t.push(T { m, fam, variant: variant.to_string(), class, code, at, cmp: cmp.to_vec(), r: Box::new(r) });
    }
    pub fn skip(&mut self, m: &'static str, fam: &'static str, why: &'static str) {
        self.skipped.push((m, fam, why));
    }
}

pub fn o(x: u32) -> Vec<Option<u32>> {
    vec![Some(x)]
}

pub fn on(x: Option<u32>) -> Vec<Option<u32>> {
    vec![x]
}

pub fn o2(a: u32, b: u32) -> Vec<Option<u32>> {
    vec![Some(a), Some(b)]
}

pub fn o64(x: u64) -> Vec<Option<u32>> {
    vec![Some(x as u32), Some((x >> 32) as u32)]
}

fn xs(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

pub const U32E: [u32; 26] = [
    0, 1, 2, 3, 0x1f, 0x20, 0x21, 0x3f, 0x40, 0x7f, 0x80, 0xff, 0x100, 0x7fff, 0x8000, 0xffff, 0x1_0000, 0x7fff_ffff, 0x8000_0000, 0x8000_0001, 0xffff_fffe, 0xffff_ffff, 0x00ff_ffff,
    0x0080_0000, 0xff80_0000, 0xffff_8000,
];
pub const F32E: [u32; 26] = [
    0, 0x8000_0000, 0x3f80_0000, 0xbf80_0000, 0x3f00_0000, 0x4000_0000, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0x7f80_0001, 0xffc1_2345, 0x0000_0001, 0x807f_ffff, 0x0080_0000,
    0x7f7f_ffff, 0x4b80_0000, 0x3fc0_0000, 0x4020_0000, 0xc020_0000, 0x4f00_0000, 0xcf00_0000, 0x4f80_0000, 0x3eff_ffff, 0x3f7f_ffff, 0xbf00_0000, 0x0040_0000,
];
pub const F16E: [u32; 22] = [
    0, 0x8000, 0x3c00, 0xbc00, 0x3800, 0x4000, 0x7c00, 0xfc00, 0x7e00, 0x7c01, 0xfe55, 0x0001, 0x83ff, 0x0400, 0x7bff, 0x3e00, 0x4100, 0xc100, 0x6400, 0x3bff, 0xb800, 0x0200,
];
pub const F64E: [u64; 22] = [
    0,
    0x8000_0000_0000_0000,
    0x3ff0_0000_0000_0000,
    0xbff0_0000_0000_0000,
    0x3fe0_0000_0000_0000,
    0x7ff0_0000_0000_0000,
    0xfff0_0000_0000_0000,
    0x7ff8_0000_0000_0000,
    0x7ff0_0000_0000_0001,
    0xfff8_0000_0001_2345,
    1,
    0x800f_ffff_ffff_ffff,
    0x0010_0000_0000_0000,
    0x7fef_ffff_ffff_ffff,
    0x4330_0000_0000_0000,
    0x3ff8_0000_0000_0000,
    0x4004_0000_0000_0000,
    0xc004_0000_0000_0000,
    0x41e0_0000_0000_0000,
    0xc1e0_0000_0020_0000,
    0x41f0_0000_0000_0000,
    0x3fdf_ffff_ffff_ffff,
];
pub const U64E: [u64; 14] = [0, 1, 0xffff_ffff, 0x1_0000_0000, 0x7fff_ffff_ffff_ffff, 0x8000_0000_0000_0000, 0xffff_ffff_ffff_ffff, 0x8000_0000, 0xffff_ffff_0000_0000, 0x3f, 0x40, 0x1_0000_0001, 0xffff_ffff_ffff_fffe, 0x7fff_ffff];

fn nice_f32(r: u64) -> u32 {
    let k = (r >> 8) as i32 % 512 - 256;
    let e = (r >> 40) as i32 % 8;
    let v = k as f32 / (1 << e) as f32;
    v.to_bits()
}

fn nice_f16(r: u64) -> u32 {
    let k = (r >> 8) as i32 % 128 - 64;
    let e = (r >> 40) as i32 % 6;
    fp::f2h_rne(k as f32 / (1 << e) as f32)
}

fn gen32(class: Class, r: u64) -> u32 {
    let pick = r & 7;
    match class {
        Class::U32 | Class::Mixed => match pick {
            0 | 1 => U32E[(r >> 8) as usize % U32E.len()],
            2 => (r >> 20) as u32 & 0x3f,
            3 => ((r >> 20) as u32 & 0xffff) | if r & 0x100 != 0 { 0xffff_0000 } else { 0 },
            _ => (r >> 16) as u32,
        },
        Class::Small => match pick {
            0..=3 => (r >> 20) as u32 & 0x3f,
            4 => U32E[(r >> 8) as usize % U32E.len()],
            _ => (r >> 16) as u32,
        },
        Class::F32 | Class::F32Cls | Class::Trans => match pick {
            0 | 1 => F32E[(r >> 8) as usize % F32E.len()],
            2 | 3 => nice_f32(r),
            4 => ((r >> 16) as u32 & 0x807f_ffff) | 0x3f00_0000 ^ (((r >> 50) as u32 & 3) << 23),
            _ => (r >> 16) as u32,
        },
        Class::F16 | Class::Trans16 => {
            let h = |q: u64| match q & 7 {
                0 | 1 => F16E[(q >> 8) as usize % F16E.len()],
                2 | 3 => nice_f16(q),
                _ => (q >> 16) as u32 & 0xffff,
            };
            h(r) | (h(r.rotate_left(23)) << 16)
        }
        Class::F64 | Class::U64 => (r >> 16) as u32,
    }
}

fn gen64(class: Class, r: u64) -> u64 {
    match class {
        Class::F64 => match r & 7 {
            0 | 1 => F64E[(r >> 8) as usize % F64E.len()],
            2 | 3 => {
                let k = (r >> 8) as i64 % 4096 - 2048;
                (k as f64 / (1u64 << ((r >> 40) % 12)) as f64).to_bits()
            }
            4 => ((r >> 3) & 0x800f_ffff_ffff_ffff) | (((0x3fe + ((r >> 56) & 3)) as u64) << 52),
            _ => r.wrapping_mul(0x9e37_79b9_7f4a_7c15),
        },
        _ => match r & 7 {
            0 | 1 => U64E[(r >> 8) as usize % U64E.len()],
            2 => (r >> 20) & 0x7f,
            _ => r.wrapping_mul(0x9e37_79b9_7f4a_7c15),
        },
    }
}

fn edges(class: Class) -> Vec<u64> {
    match class {
        Class::U32 | Class::Mixed | Class::Small => U32E.iter().map(|&x| x as u64).collect(),
        Class::F32 | Class::F32Cls | Class::Trans => F32E.iter().map(|&x| x as u64).collect(),
        Class::F16 | Class::Trans16 => F16E.iter().map(|&x| x as u64).collect(),
        Class::F64 => F64E.to_vec(),
        Class::U64 => U64E.to_vec(),
    }
}

pub fn inputs(class: Class, batch: usize) -> Vec<[u32; 8]> {
    let mut seed = 0x15a5_bee7_0000_0001u64 ^ ((class as u64 + 1) << 40) ^ ((batch as u64 + 1) * 0x0123_4567_89ab_cdef);
    let e = edges(class);
    let n = e.len();
    let wide = matches!(class, Class::F64 | Class::U64);
    let mut out = Vec::with_capacity(LANES);
    for l in 0..LANES {
        let mut d = [0u32; 8];
        for j in 0..8 {
            d[j] = gen32(class, xs(&mut seed));
        }
        if wide {
            for j in 0..4 {
                let v = gen64(class, xs(&mut seed));
                d[2 * j] = v as u32;
                d[2 * j + 1] = (v >> 32) as u32;
            }
        }
        let g = batch * LANES + l;
        if g < n * n {
            let (a, b) = (e[g % n], e[g / n]);
            if wide {
                d[0] = a as u32;
                d[1] = (a >> 32) as u32;
                d[2] = b as u32;
                d[3] = (b >> 32) as u32;
            } else if matches!(class, Class::F16 | Class::Trans16) {
                d[0] = (d[0] & 0xffff_0000) | a as u32;
                d[1] = (d[1] & 0xffff_0000) | b as u32;
                if g % 3 == 0 {
                    d[0] = (d[0] & 0xffff) | ((b as u32) << 16);
                    d[1] = (d[1] & 0xffff) | ((a as u32) << 16);
                }
            } else {
                d[0] = a as u32;
                d[1] = b as u32;
            }
        } else if g < n * n + n * n && !wide && !matches!(class, Class::F16 | Class::Trans16) {
            let h = g - n * n;
            d[1] = e[h % n] as u32;
            d[2] = e[h / n] as u32;
        }
        if class == Class::F32Cls {
            d[1] = (xs(&mut seed) as u32) & 0x3ff;
        }
        out.push(d);
    }
    out
}

pub const IN0: u32 = 10;
pub const OUT: u32 = 20;

fn prologue() -> Vec<u32> {
    let mut w = vec![sop1(3, 126, ic(-1)), 0xf408_0002, 0xfa00_0000, vop2(0x1a, 1, ic(SHIFT as i32), v(0))];
    w.extend_from_slice(&mubuf(0x0e, 0, true, 1, IN0));
    w.extend_from_slice(&mubuf(0x0e, 16, true, 1, IN0 + 4));
    for i in 0..8 {
        w.push(vop1(2, IN0 + i, v(IN0 + i)));
    }
    w.push(vop1(1, 31, ic(1)));
    w
}

fn slot_code(t: &T, k: usize) -> Vec<u32> {
    let mut w = Vec::new();
    for i in 0..4 {
        w.extend_from_slice(&[vop1(1, OUT + i, LIT), 0xabcd_0000 | ((k as u32) << 4) | i]);
    }
    w.extend_from_slice(&[sop1(3, VCC, ic(0)), sop1(3, VCC + 1, ic(0)), sop1(3, M0, ic(0)), sopc(0x06, ic(1), ic(0))]);
    w.extend_from_slice(&t.code);
    w.push(sop1(3, 126, ic(-1)));
    w.extend_from_slice(&mubuf(0x1e, (32 + 16 * k) as u32, true, 1, OUT));
    w
}

fn program(ts: &[&T]) -> Vec<u32> {
    let mut w = prologue();
    for (k, t) in ts.iter().enumerate() {
        w.extend(slot_code(t, k));
    }
    w.push(S_ENDPGM);
    w
}

fn cs_extract(words: &[u32]) -> ShaderExtract {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let root = RootSig {
        version: 2,
        flags: 1,
        params: vec![RootParam { visibility: 5, kind: RootParamKind::Table(vec![RootRange { range_type: 1, num: 8, base_register: 0, space: 0, offset_in_table: u32::MAX }]) }],
        num_static_samplers: 0,
        static_samplers: Vec::new(),
    };
    ShaderExtract {
        stage: Stage::Compute,
        shader_model: (6, 0),
        bitcode_stripped: true,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: isa.clone(),
        isa,
        isa_offset_in_fssm: 0,
        isa_preamble: [0, LANES as u32, 1, 1],
        psv_numthreads: None,
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: true,
        input_sig: vec![],
        output_sig: vec![],
        prim_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        root_sig: Some(root),
    }
}

pub fn hlsl_for(words: &[u32]) -> Result<String, String> {
    let ex = cs_extract(words);
    match std::env::var("TOMBOX_SWEEP_RSRC1").ok().and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()) {
        Some(r) => tombox_xshader::hlsl::translate_with_pgm_rsrc1(&ex, None, r),
        None => translate(&ex),
    }
    .map(|t| t.hlsl)
}

fn compile(ts: &[&T]) -> Result<(Vec<u8>, (u32, u32), String), String> {
    let words = program(ts);
    let rsrc1 = std::env::var("TOMBOX_SWEEP_RSRC1").ok().and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok());
    let ex = cs_extract(&words);
    let t = match rsrc1 {
        Some(r) => tombox_xshader::hlsl::translate_with_pgm_rsrc1(&ex, None, r),
        None => translate(&ex),
    }
    .map_err(|e| format!("translate: {e}"))?;
    let fast: &[&str] = if std::env::var_os("TOMBOX_SWEEP_FAST_FMA").is_some() { &["-D", "TB_FAST_FMA"] } else { &[] };
    let code = dxc::compile_with(&t.hlsl, &dxc::profile_for(t.profile), fast).map_err(|e| format!("dxc: {}", e.lines().take(3).collect::<Vec<_>>().join(" | ")))?;
    Ok((code, gpu::uav_layout(&t.hlsl), t.hlsl))
}

fn run_pack(ts: &[&T], ins: &[Vec<[u32; 8]>]) -> Result<Vec<Vec<[u32; 4]>>, String> {
    let (code, layout, _h) = compile(ts)?;
    let mut res: Vec<Vec<[u32; 4]>> = vec![Vec::new(); ts.len()];
    for batch in ins {
        let mut data = vec![0u32; LANES * REC];
        for (l, d) in batch.iter().enumerate() {
            data[l * REC..l * REC + 8].copy_from_slice(d);
            for k in 8..REC {
                data[l * REC + k] = 0xdead_0000 | k as u32;
            }
        }
        let out = gpu::with_dev(|dv| unsafe { gpu::dispatch(dv, &code, layout, &data) })?;
        for k in 0..ts.len() {
            for l in 0..LANES {
                let b = l * REC + 8 + 4 * k;
                res[k].push([out[b], out[b + 1], out[b + 2], out[b + 3]]);
            }
        }
    }
    Ok(res)
}

fn f32_ord(b: u32) -> i64 {
    if b & 0x8000_0000 != 0 { -((b & 0x7fff_ffff) as i64) } else { b as i64 }
}

fn f16_ord(b: u32) -> i64 {
    let b = b & 0xffff;
    if b & 0x8000 != 0 { -((b & 0x7fff) as i64) } else { b as i64 }
}

fn f64_ord(b: u64) -> i128 {
    if b >> 63 != 0 { -((b & 0x7fff_ffff_ffff_ffff) as i128) } else { b as i128 }
}

fn f16_ok(g: u32, w: u32, ulp: u32, abs: f32) -> bool {
    let (g, w) = (g & 0xffff, w & 0xffff);
    if g != w && g & 0x7fff == 0 && w & 0x7fff == 0 && abs == 0.0 {
        return false;
    }
    if nan16(w) {
        return nan16(g);
    }
    if nan16(g) {
        return false;
    }
    if g == w {
        return true;
    }
    if (f16_ord(g) - f16_ord(w)).unsigned_abs() <= ulp as u64 && (ulp > 0) {
        return true;
    }
    abs > 0.0 && (fp::h2f(g) - fp::h2f(w)).abs() <= abs
}

fn f32_ok(g: u32, w: u32, ulp: u32, abs: f32) -> bool {
    if g != w && g & 0x7fff_ffff == 0 && w & 0x7fff_ffff == 0 && abs == 0.0 {
        return false;
    }
    if nan32(w) {
        return nan32(g);
    }
    if nan32(g) {
        return false;
    }
    if g == w {
        return true;
    }
    if ulp > 0 && (f32_ord(g) - f32_ord(w)).unsigned_abs() <= ulp as u64 {
        return true;
    }
    abs > 0.0 && (f32::from_bits(g) - f32::from_bits(w)).abs() <= abs
}

fn check(cmp: &[Cmp], got: &[u32; 4], want: &[Option<u32>]) -> bool {
    for (k, c) in cmp.iter().enumerate() {
        let Some(w) = want.get(k).copied().flatten() else { continue };
        let g = got[k];
        let ok = match *c {
            Cmp::X => g == w,
            Cmp::F32(u) => f32_ok(g, w, u, 0.0),
            Cmp::F32Tol(u, a) => f32_ok(g, w, u, a),
            Cmp::F16(u) => g >> 16 == w >> 16 && f16_ok(g, w, u, 0.0),
            Cmp::F16Tol(u, a) => g >> 16 == w >> 16 && f16_ok(g, w, u, a),
            Cmp::F16H(u) => g & 0xffff == w & 0xffff && f16_ok(g >> 16, w >> 16, u, 0.0),
            Cmp::Pk16(u) => f16_ok(g, w, u, 0.0) && f16_ok(g >> 16, w >> 16, u, 0.0),
            Cmp::F64(u) => {
                let Some(wh) = want.get(k + 1).copied().flatten() else { continue };
                let (gg, ww) = ((g as u64) | ((got[k + 1] as u64) << 32), (w as u64) | ((wh as u64) << 32));
                if nan64(ww) { nan64(gg) } else if nan64(gg) { false } else { gg == ww || (f64_ord(gg) - f64_ord(ww)).unsigned_abs() <= u as u128 }
            }
            Cmp::F64Hi => true,
        };
        if !ok {
            return false;
        }
    }
    true
}

struct Res {
    m: &'static str,
    fam: &'static str,
    tests: usize,
    lanes: usize,
    undef: usize,
    fail: Option<String>,
}

fn decode_check(t: &T) -> Result<(), String> {
    let isa: Vec<u8> = t.code[t.at..].iter().flat_map(|w| w.to_le_bytes()).collect();
    use tombox_xshader::rdna2::Enc;
    let first = tombox_xshader::rdna2::disassemble(&isa).into_iter().next();
    let got = first.as_ref().map(|i| i.mnemonic.clone()).unwrap_or_default();
    if got != t.m {
        return Err(format!("encoding decodes as {got}"));
    }
    let w = t.code[t.at];
    let want = if w >> 26 == 0b110101 || w >> 26 == 0b110011 {
        Enc::Vop3
    } else if w >> 25 == 0b0111111 {
        Enc::Vop1
    } else if w >> 25 == 0b0111110 {
        Enc::Vopc
    } else if w >> 31 == 0 {
        Enc::Vop2
    } else if w >> 23 == 0b1_0111_1101 {
        Enc::Sop1
    } else if w >> 23 == 0b1_0111_1110 {
        Enc::Sopc
    } else if w >> 28 == 0b1011 {
        Enc::Sopk
    } else if w >> 30 == 0b10 {
        Enc::Sop2
    } else {
        Enc::Unknown
    };
    let enc = first.map(|i| i.enc).unwrap_or(Enc::Unknown);
    if enc != want {
        return Err(format!("encoding decodes as {enc:?}, word says {want:?}"));
    }
    let fam_ok = match t.fam {
        "VOP3" | "VOP3P" => want == Enc::Vop3,
        "VOP1" => matches!(want, Enc::Vop1 | Enc::Vop3),
        "VOP2" => matches!(want, Enc::Vop2 | Enc::Vop3),
        "VOPC" => matches!(want, Enc::Vopc | Enc::Vop3),
        "SOP1" => want == Enc::Sop1,
        "SOP2" => want == Enc::Sop2,
        "SOPC" => want == Enc::Sopc,
        "SOPK" => want == Enc::Sopk,
        _ => true,
    };
    if !fam_ok {
        return Err(format!("{} test encoded as {want:?}", t.fam));
    }
    Ok(())
}

fn eval(t: &T, got: &[[u32; 4]], ins: &[Vec<[u32; 8]>]) -> (usize, usize, Option<String>) {
    let mut lanes = 0;
    let mut undef = 0;
    for (b, batch) in ins.iter().enumerate() {
        for (l, d) in batch.iter().enumerate() {
            let want = (t.r)(&In { d: *d, lane: l as u32 });
            if want.iter().take(t.cmp.len()).any(|w| w.is_none()) {
                undef += 1;
            }
            lanes += 1;
            let g = &got[b * LANES + l];
            if !check(&t.cmp, g, &want) {
                let ws: Vec<String> = want.iter().take(t.cmp.len()).map(|w| w.map_or("--------".into(), |x| format!("{x:08x}"))).collect();
                return (lanes, undef, Some(format!("[{}] in={:08x?} got={:08x?} want=[{}]", t.variant, d, &g[..t.cmp.len()], ws.join(", "))));
            }
        }
    }
    (lanes, undef, None)
}

fn suite() -> Suite {
    let mut s = Suite { t: Vec::new(), skipped: Vec::new() };
    ops_v::add(&mut s);
    ops_x::add(&mut s);
    ops_c::add(&mut s);
    ops_s::add(&mut s);
    s
}

fn filter() -> Option<String> {
    std::env::var("TOMBOX_SWEEP_FILTER").ok().filter(|s| !s.is_empty())
}

#[test]
fn isa_sweep() {
    let t0 = std::time::Instant::now();
    let s = suite();
    let flt = filter();
    let tests: Vec<&T> = s.t.iter().filter(|t| flt.as_ref().map_or(true, |f| t.m.contains(f.as_str()) || t.fam.contains(f.as_str()))).collect();
    let mut by: BTreeMap<&'static str, Res> = BTreeMap::new();
    let mut bad_decode: Vec<&T> = Vec::new();
    let mut runnable: Vec<&T> = Vec::new();
    for t in &tests {
        let e = by.entry(t.m).or_insert(Res { m: t.m, fam: t.fam, tests: 0, lanes: 0, undef: 0, fail: None });
        e.tests += 1;
        match decode_check(t) {
            Ok(()) => runnable.push(t),
            Err(err) => {
                if e.fail.is_none() {
                    e.fail = Some(format!("[{}] {err}", t.variant));
                }
                bad_decode.push(t);
            }
        }
    }
    let mut groups: BTreeMap<Class, Vec<&T>> = BTreeMap::new();
    for t in runnable {
        groups.entry(t.class).or_default().push(t);
    }
    let mut compiles = 0usize;
    for (class, ts) in &groups {
        let ins: Vec<Vec<[u32; 8]>> = (0..BATCHES).map(|b| inputs(*class, b)).collect();
        for chunk in ts.chunks(SLOTS) {
            compiles += 1;
            let outcome = run_pack(chunk, &ins);
            let per: Vec<(usize, Result<Vec<[u32; 4]>, String>)> = match outcome {
                Ok(r) => r.into_iter().enumerate().map(|(i, v)| (i, Ok(v))).collect(),
                Err(_) => chunk
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        compiles += 1;
                        (i, run_pack(&[*t], &ins).map(|mut v| v.remove(0)))
                    })
                    .collect(),
            };
            for (i, r) in per {
                let t = chunk[i];
                let e = by.get_mut(t.m).unwrap();
                match r {
                    Ok(got) => {
                        let (n, u, f) = eval(t, &got, &ins);
                        e.lanes += n;
                        e.undef += u;
                        if e.fail.is_none() {
                            e.fail = f;
                        }
                    }
                    Err(err) => {
                        if e.fail.is_none() {
                            e.fail = Some(format!("[{}] {}", t.variant, err.lines().next().unwrap_or("")));
                        }
                    }
                }
            }
        }
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let dev = gpu::with_dev(|d| Ok(d.name)).unwrap_or("none");
    let mut table = String::new();
    let mut fams: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let _ = writeln!(table, "{:<28} {:<6} {:>5} {:>6} {:>6}  {}", "mnemonic", "family", "tests", "lanes", "undef", "result");
    let mut wfam: BTreeMap<&str, usize> = BTreeMap::new();
    for r in by.values() {
        let f = fams.entry(r.fam).or_default();
        if r.fail.is_none() {
            f.0 += 1
        } else if !gpu::hw() && WARP_KNOWN.contains(&r.m) {
            *wfam.entry(r.fam).or_default() += 1
        } else {
            f.1 += 1
        }
        let _ = writeln!(table, "{:<28} {:<6} {:>5} {:>6} {:>6}  {}", r.m, r.fam, r.tests, r.lanes, r.undef, r.fail.as_deref().map_or("PASS".to_string(), |f| format!("FAIL {f}")));
    }
    for (m, fam, why) in &s.skipped {
        let _ = writeln!(table, "{:<28} {:<6} {:>5} {:>6} {:>6}  SKIP {}", m, fam, 0, 0, 0, why);
    }
    println!("{table}");
    let passed: Vec<&str> = by.values().filter(|r| r.fail.is_none()).map(|r| r.m).collect();
    let warp = !gpu::hw();
    let failed: Vec<&Res> = by.values().filter(|r| r.fail.is_some() && !(warp && WARP_KNOWN.contains(&r.m))).collect();
    let warp_dev: Vec<&Res> = by.values().filter(|r| r.fail.is_some() && warp && WARP_KNOWN.contains(&r.m)).collect();
    for (f, (p, x)) in &fams {
        println!("{f}: {p} pass, {x} fail, {} known WARP deviations", wfam.get(f).copied().unwrap_or(0));
    }
    println!("isa_sweep on {dev}: {} mnemonics, {} pass, {} fail, {} known WARP deviations, {} skipped, {} compiles, {:.1}s", by.len(), passed.len(), failed.len(), warp_dev.len(), s.skipped.len(), compiles, elapsed);
    let esc = |x: &str| x.replace('\\', "\\\\").replace('"', "\\\"");
    let mut j = String::new();
    let _ = write!(j, "{{\n  \"test\": \"tombox-xshader/tests/isa_sweep.rs::isa_sweep\",\n  \"device\": \"{dev}\",\n  \"seconds\": {elapsed:.1},\n  \"compiles\": {compiles},\n  \"filter\": {},\n", flt.as_ref().map_or("null".into(), |f| format!("\"{}\"", esc(f))));
    let _ = write!(j, "  \"families\": {{{}}},\n", fams.iter().map(|(f, (p, x))| format!("\"{f}\": {{\"pass\": {p}, \"fail\": {x}, \"warp_known\": {}}}", wfam.get(f).copied().unwrap_or(0))).collect::<Vec<_>>().join(", "));
    let _ = write!(j, "  \"passed\": [{}],\n", passed.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(", "));
    let _ = write!(j, "  \"failures\": [\n{}\n  ],\n", failed.iter().map(|r| format!("    {{\"mnemonic\": \"{}\", \"family\": \"{}\", \"first\": \"{}\"}}", r.m, r.fam, esc(r.fail.as_deref().unwrap_or("")))).collect::<Vec<_>>().join(",\n"));
    let _ = write!(j, "  \"warp_deviations\": [
{}
  ],
", warp_dev.iter().map(|r| format!("    {{\"mnemonic\": \"{}\", \"family\": \"{}\", \"first\": \"{}\"}}", r.m, r.fam, esc(r.fail.as_deref().unwrap_or("")))).collect::<Vec<_>>().join(",
"));
    let _ = write!(j, "  \"skipped\": [\n{}\n  ]\n}}\n", s.skipped.iter().map(|(m, f, w)| format!("    {{\"mnemonic\": \"{m}\", \"family\": \"{f}\", \"why\": \"{}\"}}", esc(w))).collect::<Vec<_>>().join(",\n"));
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runs/isa_sweep");
    let _ = std::fs::create_dir_all(&dir);
    let name = if gpu::hw() { "summary_hw.json" } else { "summary.json" };
    let name = if flt.is_some() { format!("filtered_{name}") } else { name.to_string() };
    std::fs::write(dir.join(&name), j).expect("write summary");
    let _ = bad_decode;
    assert!(failed.is_empty(), "{} mnemonics failing (see table above)", failed.len());
}

const WARP_KNOWN: &[&str] = &["v_rndne_f64", "v_cvt_f64_i32", "v_cvt_u32_f64", "v_fma_f64", "v_div_fmas_f64"];

#[test]
fn isa_sweep_hlsl_probe() {
    let Ok(m) = std::env::var("TOMBOX_SWEEP_HLSL") else { return };
    let s = suite();
    let tag = std::env::var("TOMBOX_SWEEP_TAG").unwrap_or_default();
    let ts: Vec<&T> = s.t.iter().filter(|t| t.m == m && t.variant.contains(&tag)).collect();
    for t in ts.iter().take(2) {
        println!("{}", hlsl_for(&program(&[*t])).unwrap_or_else(|e| e));
    }
}

const S_WAITCNT_0: u32 = 0xbf8c_0000;
const S_NOP_4: u32 = 0xbf80_0004;

fn hw_prologue() -> Vec<u32> {
    let mut w = vec![0xf404_0100, 0xfa00_0000, sop2(0x1e, 6, 2, ic(8)), S_WAITCNT_0, sop1(3, 126, ic(-1)), 0xf408_0002, 0xfa00_0000, S_WAITCNT_0];
    w.extend_from_slice(&[vop2(0x25, 2, 6, 0), vop2(0x16, 3, ic(5), 2), vop2(0x1a, 1, ic(SHIFT as i32), 3), vop2(0x1b, 4, ic(31), 0), vop2(0x1b, 3, LIT, 3), 0xffff_ffe0, vop2(0x1c, 0, v(3), 4)]);
    w.extend_from_slice(&mubuf(0x0e, 0, true, 1, IN0));
    w.extend_from_slice(&mubuf(0x0e, 16, true, 1, IN0 + 4));
    w.push(S_WAITCNT_0);
    for i in 0..8 {
        w.push(vop1(2, IN0 + i, v(IN0 + i)));
    }
    w.push(vop1(1, 31, ic(1)));
    w.push(S_NOP_4);
    w
}

fn pre_branches(t: &T) -> bool {
    let isa: Vec<u8> = t.code[..t.at].iter().flat_map(|w| w.to_le_bytes()).collect();
    tombox_xshader::rdna2::disassemble(&isa).iter().any(|i| i.enc == tombox_xshader::rdna2::Enc::Sopp && i.mnemonic.contains("branch"))
}

fn hw_slot(t: &T, k: usize) -> Vec<u32> {
    let mut w = Vec::new();
    for i in 0..4 {
        w.extend_from_slice(&[vop1(1, OUT + i, LIT), 0xabcd_0000 | ((k as u32) << 4) | i]);
    }
    w.extend_from_slice(&[sop1(3, VCC, ic(0)), sop1(3, VCC + 1, ic(0)), sop1(3, M0, ic(0)), sopc(0x06, ic(1), ic(0))]);
    let quarter = vop3(0xc5, EXEC, [ic(3), v(30), 0]);
    let mut pre: Vec<u32> = Vec::with_capacity(t.at + 1);
    let mut i = 0;
    while i < t.at {
        if t.code[i..t.at].starts_with(&quarter) {
            pre.extend_from_slice(&vop3(0xc5, 52, [ic(3), v(30), 0]));
            pre.push(sop1(3, EXEC, 52));
            i += quarter.len();
        } else {
            pre.push(t.code[i]);
            i += 1;
        }
    }
    if pre_branches(t) {
        w.extend_from_slice(&pre);
        w.extend_from_slice(&t.code[t.at..]);
    } else {
        w.extend_from_slice(&pre);
        w.push(S_NOP_4);
        w.extend_from_slice(&t.code[t.at..]);
    }
    w.push(S_NOP_4);
    w.extend_from_slice(&[sop1(3, 126, ic(-1)), vop1(2, 9, v(1)), S_NOP_4, sop2(0x20, 9, 9, ic(SHIFT as i32)), sop2(0x0e, 9, 9, ic(31)), sop2(0x1e, 126, ic(1), 9)]);
    w.extend_from_slice(&mubuf(0x1e, (32 + 16 * k) as u32, true, 1, OUT));
    w.push(sop1(3, 126, ic(-1)));
    w
}

fn hw_program(ts: &[&T]) -> Vec<u32> {
    let mut w = hw_prologue();
    for (k, t) in ts.iter().enumerate() {
        w.extend(hw_slot(t, k));
    }
    w.push(S_ENDPGM);
    w
}

pub fn sweep_keys(s: &Suite) -> Vec<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    s.t.iter()
        .map(|t| {
            let base = format!("{}|{}", t.m, t.variant);
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            if *n == 1 { base } else { format!("{base}#{n}") }
        })
        .collect()
}

pub fn fnv64w(h: u64, w: u32) -> u64 {
    w.to_le_bytes().iter().fold(h, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

pub const FNV_SEED: u64 = 0xcbf2_9ce4_8422_2325;

pub fn hw_positions(t: &T, ins: &[Vec<[u32; 8]>]) -> Vec<(usize, usize, u32)> {
    let mut v = Vec::new();
    for (b, batch) in ins.iter().enumerate() {
        for (l, d) in batch.iter().enumerate() {
            let want = (t.r)(&In { d: *d, lane: l as u32 });
            for k in 0..t.cmp.len() {
                if let Some(w) = want.get(k).copied().flatten() {
                    v.push((b * LANES + l, k, w));
                }
            }
        }
    }
    v
}

fn known_reason(m: &str) -> &'static str {
    match m {
        "v_add_f32" | "v_sub_f32" | "v_subrev_f32" | "v_mul_f32" | "v_mul_legacy_f32" | "v_min_f32" | "v_max_f32" => "f32 omod acts only with IEEE 0 and f32 output denormals flushed (issue #386)",
        "v_mul_lo_i32" => "gfx103x has no v_mul_lo_i32 (returns 0); its low 32 bits equal v_mul_lo_u32",
        _ => "UNEXPLAINED",
    }
}

fn le_words(path: &std::path::Path) -> Option<Vec<u32>> {
    std::fs::read(path).ok().map(|b| b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn sentinel_batch(batch: &[[u32; 8]]) -> Vec<u32> {
    let mut data = vec![0u32; LANES * REC];
    for (l, d) in batch.iter().enumerate() {
        data[l * REC..l * REC + 8].copy_from_slice(d);
        for k in 8..REC {
            data[l * REC + k] = 0xdead_0000 | k as u32;
        }
    }
    data
}

#[test]
fn isa_sweep_silicon() {
    let Some(dir) = std::env::var_os("TOMBOX_SWEEP_SILICON").map(std::path::PathBuf::from) else { return };
    let mode = std::env::var("TOMBOX_SWEEP_SILICON_MODE").unwrap_or_else(|_| "export".into());
    let s = suite();
    let flt = filter();
    let absent: BTreeMap<usize, String> = std::fs::read_to_string(dir.join("absent.txt"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once(' ').and_then(|(i, w)| i.parse().ok().map(|i| (i, w.to_string()))))
        .collect();
    let mut groups: BTreeMap<Class, Vec<&T>> = BTreeMap::new();
    let mut not_here: BTreeMap<&'static str, String> = BTreeMap::new();
    for (i, t) in s.t.iter().enumerate().filter(|(_, t)| flt.as_ref().map_or(true, |f| t.m.contains(f.as_str()) || t.fam.contains(f.as_str()))) {
        if decode_check(t).is_err() {
            continue;
        }
        if let Some(why) = absent.get(&i) {
            not_here.entry(t.m).or_insert_with(|| why.clone());
            continue;
        }
        groups.entry(t.class).or_default().push(t);
    }
    let mut packs: Vec<(Class, Vec<&T>)> = Vec::new();
    for (class, ts) in &groups {
        for chunk in ts.chunks(SLOTS) {
            packs.push((*class, chunk.to_vec()));
        }
    }
    if mode == "export" {
        std::fs::create_dir_all(&dir).expect("create export dir");
        let mut ops = String::new();
        for (i, t) in s.t.iter().enumerate() {
            let isa: Vec<u8> = t.code[t.at..].iter().flat_map(|w| w.to_le_bytes()).collect();
            let n = tombox_xshader::rdna2::disassemble(&isa).first().map_or(1, |x| x.raw.len().max(1));
            let words: Vec<String> = t.code[t.at..(t.at + n).min(t.code.len())].iter().map(|w| format!("{w:08x}")).collect();
            let _ = writeln!(ops, "{i} {} {} {}", t.m, t.fam, words.join(","));
        }
        std::fs::write(dir.join("ops.txt"), ops).expect("write ops");
        let mut man = String::new();
        for class in groups.keys() {
            for b in 0..BATCHES {
                let data = sentinel_batch(&inputs(*class, b));
                let bytes: Vec<u8> = data.iter().flat_map(|w| w.to_le_bytes()).collect();
                std::fs::write(dir.join(format!("in_{class:?}_{b}.bin")), bytes).expect("write inputs");
            }
        }
        for (p, (class, ts)) in packs.iter().enumerate() {
            let bytes: Vec<u8> = hw_program(ts).iter().flat_map(|w| w.to_le_bytes()).collect();
            std::fs::write(dir.join(format!("p{p:03}.bin")), bytes).expect("write pack");
            let names: Vec<String> = ts.iter().map(|t| format!("{}:{}", t.m, t.variant.replace(' ', "_"))).collect();
            let _ = writeln!(man, "p{p:03} {class:?} {} {}", ts.len(), names.join("|"));
        }
        std::fs::write(dir.join("manifest.txt"), man).expect("write manifest");
        println!("isa_sweep_silicon: exported {} packs, {} tests, {} classes to {}", packs.len(), packs.iter().map(|p| p.1.len()).sum::<usize>(), groups.len(), dir.display());
        return;
    }
    if mode == "host" {
        let sub = std::env::var("TOMBOX_SWEEP_SILICON_OUT").unwrap_or_else(|_| "out".into());
        let mut first: BTreeMap<&'static str, String> = BTreeMap::new();
        let mut ok_m: std::collections::BTreeSet<&'static str> = Default::default();
        for (p, (class, ts)) in packs.iter().enumerate() {
            let ins: Vec<Vec<[u32; 8]>> = (0..BATCHES).map(|b| inputs(*class, b)).collect();
            let outs: Vec<Option<Vec<u32>>> = (0..BATCHES).map(|b| le_words(&dir.join(&sub).join(format!("p{p:03}_b{b}.bin")))).collect();
            if outs.iter().any(|o| o.is_none()) {
                continue;
            }
            let host = match run_pack(ts, &ins) {
                Ok(h) => h,
                Err(e) => {
                    for t in ts {
                        first.entry(t.m).or_insert_with(|| format!("p{p:03} host run failed: {}", e.lines().next().unwrap_or("")));
                    }
                    continue;
                }
            };
            for (k, t) in ts.iter().enumerate() {
                let mut bad = None;
                'lanes: for (b, o) in outs.iter().flatten().enumerate() {
                    for l in 0..LANES {
                        let at = l * REC + 8 + 4 * k;
                        let sil: Vec<Option<u32>> = (0..4).map(|i| Some(o[at + i])).collect();
                        let g = host[k][b * LANES + l];
                        if !check(&t.cmp, &g, &sil) {
                            bad = Some(format!("p{p:03} [{}] in={:08x?} host={:08x?} silicon={:08x?}", t.variant, &ins[b][l][..3], &g[..t.cmp.len()], &o[at..at + t.cmp.len()]));
                            break 'lanes;
                        }
                    }
                }
                match bad {
                    Some(f) => {
                        first.entry(t.m).or_insert(f);
                    }
                    None => {
                        ok_m.insert(t.m);
                    }
                }
            }
        }
        for (m, f) in &first {
            println!("HOSTDIFF {m:<26} {f}");
        }
        let n = ok_m.iter().filter(|m| !first.contains_key(*m)).count();
        println!("isa_sweep_silicon host: translator (initial MODE from TOMBOX_SWEEP_RSRC1) vs silicon {sub}: {} mnemonics agree, {} differ", n, first.len());
        return;
    }
    let mut by: BTreeMap<&'static str, Res> = BTreeMap::new();
    let mut skipped: BTreeMap<&'static str, String> = BTreeMap::new();
    let keys = sweep_keys(&s);
    let key_of: std::collections::HashMap<*const T, &str> = s.t.iter().zip(keys.iter()).map(|(t, k)| (t as *const T, k.as_str())).collect();
    let mut rec = String::new();
    let mut n_ex = 0usize;
    for (p, (class, ts)) in packs.iter().enumerate() {
        let ins: Vec<Vec<[u32; 8]>> = (0..BATCHES).map(|b| inputs(*class, b)).collect();
        let outs: Vec<Option<Vec<u32>>> = (0..BATCHES).map(|b| le_words(&dir.join("out").join(format!("p{p:03}_b{b}.bin")))).collect();
        if outs.iter().any(|o| o.as_ref().map_or(true, |w| w.len() != LANES * REC)) {
            let why = std::fs::read_to_string(dir.join("out").join(format!("p{p:03}.skip"))).unwrap_or_else(|_| "no silicon output".into());
            for t in ts {
                skipped.entry(t.m).or_insert_with(|| why.trim().to_string());
            }
            continue;
        }
        for (k, t) in ts.iter().enumerate() {
            let mut got: Vec<[u32; 4]> = Vec::with_capacity(BATCHES * LANES);
            for o in outs.iter().flatten() {
                for l in 0..LANES {
                    let b = l * REC + 8 + 4 * k;
                    got.push([o[b], o[b + 1], o[b + 2], o[b + 3]]);
                }
            }
            let e = by.entry(t.m).or_insert(Res { m: t.m, fam: t.fam, tests: 0, lanes: 0, undef: 0, fail: None });
            e.tests += 1;
            let (n, u, f) = eval(t, &got, &ins);
            if mode == "record" {
                let mut h = FNV_SEED;
                let mut ex: Vec<String> = Vec::new();
                let known = if f.is_some() { known_reason(t.m) } else { "" };
                for (lane, k, w) in hw_positions(t, &ins) {
                    let g = got[lane][k];
                    h = fnv64w(h, g);
                    if g != w && known.is_empty() {
                        ex.push(format!("{:#x}", ((lane as u64) << 34) | ((k as u64) << 32) | g as u64));
                    }
                }
                n_ex += ex.len();
                let key = key_of[&(*t as *const T)];
                let _ = writeln!(rec, "    (\"{}\", {h:#018x}, \"{known}\", &[{}]),", key.replace('\\', "\\\\").replace('"', "\\\""), ex.join(", "));
            }
            e.lanes += n;
            e.undef += u;
            if e.fail.is_none() {
                e.fail = f.map(|x| format!("p{p:03} {x}"));
            }
        }
    }
    let mut table = String::new();
    let _ = writeln!(table, "{:<28} {:<6} {:>5} {:>6} {:>6}  {}", "mnemonic", "family", "tests", "lanes", "undef", "result vs gfx1036");
    for r in by.values() {
        let _ = writeln!(table, "{:<28} {:<6} {:>5} {:>6} {:>6}  {}", r.m, r.fam, r.tests, r.lanes, r.undef, r.fail.as_deref().map_or("PASS".to_string(), |f| format!("FAIL {f}")));
    }
    for (m, why) in &not_here {
        if !by.contains_key(m) {
            skipped.entry(*m).or_insert_with(|| format!("not on gfx103x: {why}"));
        }
    }
    for (m, why) in &skipped {
        if !by.contains_key(m) {
            let _ = writeln!(table, "{m:<28} {:<6} {:>5} {:>6} {:>6}  SKIP {why}", "", 0, 0, 0);
        }
    }
    println!("{table}");
    let fails = by.values().filter(|r| r.fail.is_some()).count();
    println!("isa_sweep_silicon: {} mnemonics checked against gfx1036, {} pass, {} fail, {} not run on silicon", by.len(), by.len() - fails, fails, skipped.keys().filter(|m| !by.contains_key(*m)).count());
    std::fs::write(dir.join("check.txt"), table).expect("write check");
    if mode == "record" {
        let n = rec.lines().count();
        let src = format!("pub const SOURCE: &str = \"gfx1036, Ryzen 5 9600X iGPU, ROCm/HIP 5.7.1, 2026-11-25, raw-kernel run of every isa_sweep case\";\n\npub const RECORD: [(&str, u64, &str, &[u64]); {n}] = [\n{rec}];\n");
        std::fs::write(dir.join("rdna2_sweep_hw.rs"), &src).expect("write record");
        println!("isa_sweep_silicon: recorded {n} cases, {n_ex} exception words, {} bytes", src.len());
    }
}

#[test]
fn sweep_models_match_rdna2_silicon() {
    let s = suite();
    let keys = sweep_keys(&s);
    let rec: BTreeMap<&str, (u64, &str, &[u64])> = rdna2_sweep_hw::RECORD.iter().map(|&(k, h, kn, ex)| (k, (h, kn, ex))).collect();
    let mut cache: BTreeMap<Class, Vec<Vec<[u32; 8]>>> = BTreeMap::new();
    let (mut checked, mut unrecorded) = (0usize, 0usize);
    let mut known: BTreeMap<&str, &str> = BTreeMap::new();
    let mut bad: Vec<String> = Vec::new();
    for (t, key) in s.t.iter().zip(&keys) {
        let Some(&(h, kn, ex)) = rec.get(key.as_str()) else {
            unrecorded += 1;
            continue;
        };
        if !kn.is_empty() {
            if kn == "UNEXPLAINED" {
                bad.push(format!("{key}: recorded as an unexplained silicon difference"));
            }
            known.insert(t.m, kn);
            continue;
        }
        let ins = cache.entry(t.class).or_insert_with(|| (0..BATCHES).map(|b| inputs(t.class, b)).collect());
        let pos = hw_positions(t, ins);
        let mut got = vec![[0u32; 4]; BATCHES * LANES];
        for &(lane, k, w) in &pos {
            got[lane][k] = w;
        }
        for &e in ex {
            got[(e >> 34) as usize][((e >> 32) & 3) as usize] = e as u32;
        }
        if pos.iter().fold(FNV_SEED, |a, &(lane, k, _)| fnv64w(a, got[lane][k])) != h {
            bad.push(format!("{key}: the model plus the recorded exceptions no longer reproduce the gfx1036 output digest"));
            continue;
        }
        if let (_, _, Some(f)) = eval(t, &got, ins) {
            bad.push(format!("{key}: {f}"));
            continue;
        }
        checked += 1;
    }
    for (m, why) in &known {
        println!("known silicon difference: {m}: {why}");
    }
    println!("sweep models vs {}: {checked} cases checked against silicon, {} mnemonics with a known difference, {unrecorded} cases not in the record", rdna2_sweep_hw::SOURCE, known.len());
    assert!(bad.is_empty(), "{} sweep cases disagree with the gfx1036 record:\n{}", bad.len(), bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
}
