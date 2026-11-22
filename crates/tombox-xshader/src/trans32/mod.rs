mod amd;
mod tables;

#[cfg(test)]
mod rdna2_hw;

use std::fmt::Write;
use tables::{RCP, RSQ0, RSQ1, SQRT0, SQRT1};

pub const OPS: [&str; 8] = ["v_rcp_f32", "v_rcp_iflag_f32", "v_rsq_f32", "v_sqrt_f32", "v_log_f32", "v_exp_f32", "v_sin_f32", "v_cos_f32"];
pub const OPS64: [&str; 3] = ["v_rcp_f64", "v_rsq_f64", "v_sqrt_f64"];

pub fn op_index(mnemonic: &str) -> Option<usize> {
    OPS.iter().position(|m| *m == mnemonic)
}

pub fn op64_index(mnemonic: &str) -> Option<usize> {
    OPS64.iter().position(|m| *m == mnemonic)
}

fn floor_div(v: i64, d: i64) -> i64 {
    let q = v / d;
    if (v % d != 0) && ((v < 0) != (d < 0)) { q - 1 } else { q }
}

fn rne(v: i64, d: i64) -> i64 {
    let q = floor_div(v, d);
    let r = v - q * d;
    let h = d / 2;
    if r > h || (r == h && q & 1 != 0) { q + 1 } else { q }
}

struct Unit {
    seg_bits: u32,
    sq_shift: u32,
    qc: i64,
    qlo: i64,
    qhi: i64,
    switch_bits: u32,
    ext_bits: u32,
    cubic_bits: u32,
}

const RCP_UNIT: Unit = Unit { seg_bits: 18, sq_shift: 1, qc: 64, qlo: 32, qhi: 64, switch_bits: 29, ext_bits: 6, cubic_bits: 2 };
const RSQ_UNIT: Unit = Unit { seg_bits: 18, sq_shift: 1, qc: 128, qlo: 32, qhi: 64, switch_bits: 29, ext_bits: 6, cubic_bits: 2 };
const SQRT_UNIT: Unit = Unit { seg_bits: 19, sq_shift: 2, qc: 64, qlo: 64, qhi: 128, switch_bits: 30, ext_bits: 5, cubic_bits: 1 };

fn internal(row: &[i64; 6], u: &Unit, t: i64, ext: i64, linear_ext: i64) -> i64 {
    let [c0, c1, c2, q2, m3, dn] = *row;
    let k = u.ext_bits;
    let p = c1 * ((t << k) | linear_ext);
    let q = if p.abs() < (1i64 << (u.switch_bits + u.seg_bits + k)) { u.qlo } else { u.qhi };
    let coarse = rne(c0 + rne(p, q << (u.seg_bits + k)) * q, u.qc) * u.qc;
    let a = t >> u.sq_shift;
    let s = (a * a) >> 10;
    let th = ((t << k) | ext) >> (k - u.cubic_bits);
    let h = c2 + q2 * rne(m3 * (th + 1), dn << u.cubic_bits);
    let v = h * s;
    let mut sh = 24;
    while v.abs() >= 1i64 << (sh + 24) {
        sh += 1;
    }
    coarse + (rne(v, 1i64 << sh) << (sh - 24))
}

fn pack(sign: u32, e: i32, a: i64, round_bits: u32, shift: i64) -> u32 {
    if e < -126 {
        let extra = (-126 - e) as u32;
        if extra > 26 {
            return sign;
        }
        let y = rne(a + shift, 1i64 << (round_bits + extra));
        return if y < 0x80_0000 { sign } else { sign | y as u32 };
    }
    let mut y = rne(a + shift, 1i64 << round_bits);
    let mut e = e;
    if y >= 1 << 24 {
        y >>= 1;
        e += 1;
    }
    if e > 127 {
        return sign | 0x7f80_0000;
    }
    sign | (((e + 127) as u32) << 23) | (y as u32 & 0x7f_ffff)
}

pub fn eval(op: usize, x: u32) -> u32 {
    match op {
        4 => return amd::log(x),
        5 => return amd::exp(x),
        6 => return amd::trig(x, false),
        7 => return amd::trig(x, true),
        _ => {}
    }
    let s = x & 0x8000_0000;
    let ex = (x >> 23) & 0xff;
    let mn = if ex == 0 { 0 } else { x & 0x7f_ffff };
    if ex == 255 && mn != 0 {
        return x;
    }
    let (zero, inf) = (ex == 0, ex == 255);
    let e = ex as i32 - 127;
    match op {
        0 | 1 => {
            if zero {
                return s | 0x7f80_0000;
            }
            if inf {
                return s;
            }
            let a = if mn == 0 { 1i64 << 35 } else { internal(&RCP[(mn >> 18) as usize], &RCP_UNIT, (mn & 0x3_ffff) as i64, 0, 0) };
            pack(s, -e - 1, a, 11, if mn >> 18 == 0 { 0 } else { -256 })
        }
        2 => {
            if zero {
                return s | 0x7f80_0000;
            }
            if s != 0 {
                return 0xffc0_0000;
            }
            if inf {
                return 0;
            }
            let par = e & 1;
            let eh = (e - par) / 2;
            let tab = if par == 0 { &RSQ0 } else { &RSQ1 };
            let a = if mn == 0 && par == 0 { 1i64 << 36 } else { internal(&tab[(mn >> 18) as usize], &RSQ_UNIT, (mn & 0x3_ffff) as i64, 0, 0) };
            pack(0, -eh - 1, a, 12, if mn >> 18 == 0 { 0 } else { -512 })
        }
        _ => {
            if zero {
                return s;
            }
            if s != 0 {
                return 0xffc0_0000;
            }
            if inf {
                return 0x7f80_0000;
            }
            let par = e & 1;
            let eh = (e - par) / 2;
            let tab = if par == 0 { &SQRT0 } else { &SQRT1 };
            let a = internal(&tab[(mn >> 19) as usize], &SQRT_UNIT, (mn & 0x7_ffff) as i64, 0, 0);
            pack(0, eh, a, 12, if mn >> 19 == 15 { 0 } else { -512 })
        }
    }
}

const SIGN64: u64 = 1 << 63;
const INF64: u64 = 0x7ff0_0000_0000_0000;
const MANT64: u64 = (1 << 52) - 1;
const NAN64: u64 = 0xfff8_0000_0000_0000;

fn make64(sign: u64, a: i64, p: i32) -> u64 {
    let a = a as u64;
    let lead = 63 - a.leading_zeros() as i32;
    let e = lead + p;
    if e > 1023 {
        return sign | INF64;
    }
    if e >= -1022 {
        return sign | (((e + 1023) as u64) << 52) | ((a << (52 - lead)) & MANT64);
    }
    let sh = -1074 - p;
    if sh >= 64 {
        return sign;
    }
    sign | if sh <= 0 { a << -sh } else { a >> sh }
}

fn nudge(bits: u64, up: bool) -> u64 {
    let mag = bits & !SIGN64;
    if mag == 0 || mag == INF64 {
        return bits;
    }
    (bits & SIGN64) | if up { mag + 1 } else { mag - 1 }
}

pub fn eval64(op: usize, x: u64) -> u64 {
    let s = x & SIGN64;
    let ex = ((x >> 52) & 0x7ff) as i32;
    let mut mf = x & MANT64;
    if ex == 0x7ff && mf != 0 {
        return x;
    }
    let inf = ex == 0x7ff;
    let mut e = ex - 1023;
    if ex == 0 {
        if mf == 0 {
            return if op == 2 { s } else { s | INF64 };
        }
        let lz = mf.leading_zeros() as i32 - 11;
        mf = (mf << lz) & MANT64;
        e = -1022 - lz;
    }
    let m = (mf >> 29) as i64;
    let low_only = mf >> 23 == 0 && mf != 0;
    match op {
        0 => {
            if inf {
                return s;
            }
            if low_only {
                return nudge(make64(s, 1, -e), false);
            }
            if mf == MANT64 {
                return nudge(make64(s, 1, -e - 1), true);
            }
            let ext = ((mf >> 23) & 63) as i64;
            let a = if mf == 0 { 1i64 << 35 } else { internal(&RCP[(m >> 18) as usize], &RCP_UNIT, m & 0x3_ffff, ext, ext) };
            make64(s, a, -35 - e)
        }
        1 => {
            if s != 0 {
                return NAN64;
            }
            if inf {
                return 0;
            }
            let par = e & 1;
            let eh = (e - par) / 2;
            if par == 0 && low_only {
                return nudge(make64(0, 1, -eh), false);
            }
            let seg = (m >> 18) as usize;
            let ext = ((mf >> 23) & 63) as i64;
            let tab = if par == 0 { &RSQ0 } else { &RSQ1 };
            let a = if mf == 0 && par == 0 { 1i64 << 36 } else { internal(&tab[seg], &RSQ_UNIT, m & 0x3_ffff, ext, if seg == 31 { ext & 15 } else { ext }) };
            make64(0, a, -36 - eh)
        }
        _ => {
            if s != 0 {
                return NAN64;
            }
            if inf {
                return INF64;
            }
            let par = e & 1;
            let eh = (e - par) / 2;
            let ext = ((mf >> 24) & 31) as i64;
            let tab = if par == 0 { &SQRT0 } else { &SQRT1 };
            let a = internal(&tab[(m >> 19) as usize], &SQRT_UNIT, m & 0x7_ffff, ext, ext);
            make64(0, a, -35 + eh)
        }
    }
}

const HLSL_BODY: &str = r#"int64_t tb_t32_fdiv(int64_t v, int64_t d) {
    int64_t q = v / d;
    if ((v % d) != 0 && ((v < 0) != (d < 0))) q -= 1;
    return q;
}
int64_t tb_t32_rne(int64_t v, int64_t d) {
    int64_t q = tb_t32_fdiv(v, d);
    int64_t r = v - q * d, h = d / 2;
    return (r > h || (r == h && (q & 1) != 0)) ? q + 1 : q;
}
int64_t tb_t32_internal(uint row, int64_t t, int64_t ext, int64_t lext, uint sb, uint sqs, int64_t qc, int64_t qlo, int64_t qhi, uint swb, uint kb, uint kh) {
    int64_t c0 = TB_T32_TAB[row * 6u + 0u], c1 = TB_T32_TAB[row * 6u + 1u], c2 = TB_T32_TAB[row * 6u + 2u];
    int64_t q2 = TB_T32_TAB[row * 6u + 3u], m3 = TB_T32_TAB[row * 6u + 4u], dn = TB_T32_TAB[row * 6u + 5u];
    int64_t p = c1 * ((t << kb) | lext);
    int64_t pm = p < 0 ? -p : p;
    int64_t q = pm < (((int64_t)1) << (swb + sb + kb)) ? qlo : qhi;
    int64_t coarse = tb_t32_rne(c0 + tb_t32_rne(p, q << (sb + kb)) * q, qc) * qc;
    int64_t a = t >> sqs;
    int64_t s = (a * a) >> 10;
    int64_t th = ((t << kb) | ext) >> (kb - kh);
    int64_t hh = c2 + q2 * tb_t32_rne(m3 * (th + 1), dn << kh);
    int64_t v = hh * s;
    int64_t vm = v < 0 ? -v : v;
    uint sh = 24u;
    [loop] while (vm >= (((int64_t)1) << (sh + 24u))) sh += 1u;
    return coarse + (tb_t32_rne(v, ((int64_t)1) << sh) << (sh - 24u));
}
uint tb_t32_pack(uint s, int e, int64_t a, uint rb, int64_t shift) {
    if (e < -126) {
        uint extra = (uint)(-126 - e);
        if (extra > 26u) return s;
        int64_t yd = tb_t32_rne(a + shift, ((int64_t)1) << (rb + extra));
        return yd < 0x800000 ? s : (s | (uint)yd);
    }
    int64_t y = tb_t32_rne(a + shift, ((int64_t)1) << rb);
    if (y >= (((int64_t)1) << 24)) { y >>= 1; e += 1; }
    if (e > 127) return s | 0x7f800000u;
    return s | ((uint)(e + 127) << 23) | ((uint)y & 0x7fffffu);
}
uint tb_t32(uint op, uint x) {
    if (op >= 4u) return tb_t32_amd(op, x);
    uint s = x & 0x80000000u, ex = (x >> 23) & 0xffu;
    uint mn = ex == 0u ? 0u : (x & 0x7fffffu);
    if (ex == 255u && mn != 0u) return x;
    bool zero = ex == 0u, inf = ex == 255u;
    int e = (int)ex - 127;
    if (op <= 1u) {
        if (zero) return s | 0x7f800000u;
        if (inf) return s;
        int64_t a = mn == 0u ? (((int64_t)1) << 35) : tb_t32_internal(mn >> 18, (int64_t)(mn & 0x3ffffu), 0, 0, 18u, 1u, 64, 32, 64, 29u, 6u, 2u);
        return tb_t32_pack(s, -e - 1, a, 11u, (mn >> 18) == 0u ? 0 : -256);
    }
    if (zero) return op == 2u ? (s | 0x7f800000u) : s;
    if (s != 0u) return 0xffc00000u;
    int par = e & 1;
    int eh = (e - par) / 2;
    if (op == 2u) {
        if (inf) return 0u;
        int64_t a = (mn == 0u && par == 0) ? (((int64_t)1) << 36) : tb_t32_internal(32u + 32u * (uint)par + (mn >> 18), (int64_t)(mn & 0x3ffffu), 0, 0, 18u, 1u, 128, 32, 64, 29u, 6u, 2u);
        return tb_t32_pack(0u, -eh - 1, a, 12u, (mn >> 18) == 0u ? 0 : -512);
    }
    if (inf) return 0x7f800000u;
    int64_t a = tb_t32_internal(96u + 16u * (uint)par + (mn >> 19), (int64_t)(mn & 0x7ffffu), 0, 0, 19u, 2u, 64, 64, 128, 30u, 5u, 1u);
    return tb_t32_pack(0u, eh, a, 12u, (mn >> 19) == 15u ? 0 : -512);
}
uint64_t tb_t64_make(uint64_t s, int64_t a, int p) {
    uint64_t au = (uint64_t)a;
    int lead = (int)tb_a_bw64(au) - 1;
    int e = lead + p;
    if (e > 1023) return s | (((uint64_t)0x7ff00000u) << 32);
    if (e >= -1022) return s | (((uint64_t)(uint)(e + 1023)) << 52) | ((au << (uint)(52 - lead)) & ((((uint64_t)1) << 52) - 1));
    int sh = -1074 - p;
    if (sh >= 64) return s;
    return s | (sh <= 0 ? (au << (uint)(-sh)) : (au >> (uint)sh));
}
uint64_t tb_t64_nudge(uint64_t b, bool up) {
    uint64_t sg = b & (((uint64_t)1) << 63);
    uint64_t mag = b ^ sg;
    if (mag == 0 || mag == (((uint64_t)0x7ff00000u) << 32)) return b;
    return sg | (up ? mag + 1 : mag - 1);
}
uint64_t tb_t64_bits(uint op, uint64_t x) {
    uint64_t INF = ((uint64_t)0x7ff00000u) << 32;
    uint64_t MANT = (((uint64_t)1) << 52) - 1;
    uint64_t s = x & (((uint64_t)1) << 63);
    int ex = (int)((x >> 52) & 0x7ff);
    uint64_t mf = x & MANT;
    if (ex == 0x7ff && mf != 0) return x;
    bool inf = ex == 0x7ff;
    int e = ex - 1023;
    if (ex == 0) {
        if (mf == 0) return op == 2u ? s : (s | INF);
        int lz = 53 - (int)tb_a_bw64(mf);
        mf = (mf << (uint)lz) & MANT;
        e = -1022 - lz;
    }
    int64_t m = (int64_t)(mf >> 29);
    bool low_only = (mf >> 23) == 0 && mf != 0;
    if (op == 0u) {
        if (inf) return s;
        if (low_only) return tb_t64_nudge(tb_t64_make(s, 1, -e), false);
        if (mf == MANT) return tb_t64_nudge(tb_t64_make(s, 1, -e - 1), true);
        int64_t ext = (int64_t)((mf >> 23) & 63);
        int64_t a = mf == 0 ? (((int64_t)1) << 35) : tb_t32_internal((uint)(m >> 18), m & 0x3ffff, ext, ext, 18u, 1u, 64, 32, 64, 29u, 6u, 2u);
        return tb_t64_make(s, a, -35 - e);
    }
    if (s != 0) return ((uint64_t)0xfff80000u) << 32;
    int par = e & 1;
    int eh = (e - par) / 2;
    if (op == 1u) {
        if (inf) return 0;
        if (par == 0 && low_only) return tb_t64_nudge(tb_t64_make(0, 1, -eh), false);
        uint seg = (uint)(m >> 18);
        int64_t ext = (int64_t)((mf >> 23) & 63);
        int64_t a = (mf == 0 && par == 0) ? (((int64_t)1) << 36) : tb_t32_internal(32u + 32u * (uint)par + seg, m & 0x3ffff, ext, seg == 31u ? (ext & 15) : ext, 18u, 1u, 128, 32, 64, 29u, 6u, 2u);
        return tb_t64_make(0, a, -36 - eh);
    }
    if (inf) return INF;
    int64_t ext5 = (int64_t)((mf >> 24) & 31);
    int64_t a = tb_t32_internal(96u + 16u * (uint)par + (uint)(m >> 19), m & 0x7ffff, ext5, ext5, 19u, 2u, 64, 64, 128, 30u, 5u, 1u);
    return tb_t64_make(0, a, -35 + eh);
}
uint2 tb_t64(uint op, uint lo, uint hi) {
    uint64_t r = tb_t64_bits(op, (((uint64_t)hi) << 32) | (uint64_t)lo);
    return uint2((uint)r, (uint)(r >> 32));
}
"#;

pub fn hlsl() -> String {
    let rows: Vec<&[i64; 6]> = RCP.iter().chain(RSQ0.iter()).chain(RSQ1.iter()).chain(SQRT0.iter()).chain(SQRT1.iter()).collect();
    let vals: Vec<String> = rows.iter().flat_map(|r| r.iter()).map(|v| format!("{v}")).collect();
    let mut h = String::new();
    let _ = writeln!(h, "static const int64_t TB_T32_TAB[{}] = {{ {} }};", vals.len(), vals.join(", "));
    h.push_str(&amd::hlsl());
    h.push_str(HLSL_BODY);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv(words: impl Iterator<Item = u32>) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for w in words {
            for b in w.to_le_bytes() {
                h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        h
    }

    fn flush(x: u32) -> u32 {
        if x & 0x7f80_0000 == 0 { x & 0x8000_0000 } else { x }
    }

    #[test]
    fn f32_transcendentals_match_rdna2_on_recorded_ranges() {
        for &(m, base, want) in rdna2_hw::RANGE_DIGESTS.iter() {
            let op = op_index(m).unwrap();
            let got = fnv((0..1u32 << 23).map(|k| eval(op, base.wrapping_add(k))));
            assert_eq!(got, want, "{m} from {base:#010x}: model digest differs from the {} record", rdna2_hw::SOURCE);
        }
    }

    #[test]
    fn f32_transcendentals_match_rdna2_across_the_float_range() {
        for &(m, mode, want) in rdna2_hw::EDGE_DIGESTS.iter() {
            let op = op_index(m).unwrap();
            let run = |x: u32| if mode == 0 { flush(eval(op, flush(x))) } else { eval(op, x) };
            let got = fnv((0..1u32 << 20).map(|g| run(g.wrapping_mul(4099))));
            assert_eq!(got, want, "{m} denormal mode {mode}: model digest differs from the {} record", rdna2_hw::SOURCE);
        }
    }

    fn flush64(x: u64) -> u64 {
        if x & INF64 == 0 { x & SIGN64 } else { x }
    }

    #[test]
    fn f64_rcp_rsq_sqrt_match_rdna2_records() {
        for &(m, kind, a, b, mode, want) in rdna2_hw::F64_DIGESTS.iter() {
            let op = op64_index(m).unwrap();
            let (n, x): (u32, Box<dyn Fn(u32) -> u64>) = match kind {
                0 => (1 << 20, Box::new(move |g: u32| ((g.wrapping_mul(4099) as u64) << 32) | a as u64)),
                1 => (1 << 23, Box::new(move |g: u32| ((a as u64) << 32) | g as u64)),
                2 => (1 << 23, Box::new(move |g: u32| ((a as u64) << 32) | 0u32.wrapping_sub(g) as u64)),
                _ => (1 << 20, Box::new(move |g: u32| ((a.wrapping_add(g) as u64) << 32) | b as u64)),
            };
            let run = |v: u64| if mode == 0 { flush64(eval64(op, flush64(v))) } else { eval64(op, v) };
            let got = fnv((0..n).flat_map(|g| {
                let r = run(x(g));
                [r as u32, (r >> 32) as u32]
            }));
            assert_eq!(got, want, "{m} record kind {kind} ({a:#010x}, {b:#010x}) mode {mode}: model digest differs from the {} record", rdna2_hw::SOURCE);
        }
    }

    #[test]
    fn f64_trans_specials() {
        let cases: [(usize, u64, u64); 12] = [
            (0, 0x3ff0_0000_0000_0000, 0x3ff0_0000_0000_0000),
            (0, 0x3ff0_0000_0000_0001, 0x3fef_ffff_ffff_ffff),
            (0, 0x3fff_ffff_ffff_ffff, 0x3fe0_0000_0000_0001),
            (0, 0x7fe0_0000_0000_0001, 0x0007_ffff_ffff_ffff),
            (0, 0x7fef_ffff_ffff_ffff, 0x0004_0000_0000_0001),
            (0, 0x0004_0000_0000_0001, 0x7ff0_0000_0000_0000),
            (0, 0x0008_0000_0000_0001, 0x7fdf_ffff_ffff_ffff),
            (1, 0x0004_0000_0000_0001, 0x5fef_ffff_ffff_ffff),
            (1, 0x0008_0000_0000_0001, 0x5fe6_a09e_6000_0000),
            (1, 0x8004_0000_0000_0001, 0xfff8_0000_0000_0000),
            (2, 0x8000_0000_0000_0000, 0x8000_0000_0000_0000),
            (2, 0x7ff0_0000_0000_0000, 0x7ff0_0000_0000_0000),
        ];
        for (op, x, want) in cases {
            assert_eq!(eval64(op, x), want, "{} {x:#018x}", OPS64[op]);
        }
        for op in 0..3 {
            assert_eq!(eval64(op, 0x7ff4_0000_0000_0001), 0x7ff4_0000_0000_0001, "{} keeps a signalling NaN", OPS64[op]);
        }
    }

    #[test]
    fn f32_trans_specials() {
        let cases: [(usize, u32, u32); 14] = [
            (0, 0x0000_0000, 0x7f80_0000), (0, 0x8000_0000, 0xff80_0000), (0, 0x7f80_0000, 0), (0, 0x0000_0001, 0x7f80_0000),
            (0, 0x3f80_0000, 0x3f80_0000), (0, 0x7f7f_ffff, 0),
            (2, 0x8000_0000, 0xff80_0000), (2, 0xbf80_0000, 0xffc0_0000), (2, 0x7f80_0000, 0), (2, 0x3f80_0000, 0x3f80_0000),
            (3, 0x8000_0000, 0x8000_0000), (3, 0x8000_0001, 0x8000_0000), (3, 0xbf80_0000, 0xffc0_0000), (3, 0x4080_0000, 0x4000_0000),
        ];
        for (op, x, want) in cases {
            assert_eq!(eval(op, x), want, "{} {x:#010x}", OPS[op]);
        }
        for op in 0..4 {
            assert_eq!(eval(op, 0x7fa0_0001), 0x7fa0_0001, "{} keeps a signalling NaN", OPS[op]);
        }
    }
}
