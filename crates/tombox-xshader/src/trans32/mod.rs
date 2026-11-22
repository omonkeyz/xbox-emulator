mod tables;

#[cfg(test)]
mod rdna2_hw;

use std::fmt::Write;
use tables::{RCP, RSQ0, RSQ1, SQRT0, SQRT1};

pub const OPS: [&str; 4] = ["v_rcp_f32", "v_rcp_iflag_f32", "v_rsq_f32", "v_sqrt_f32"];

pub fn op_index(mnemonic: &str) -> Option<usize> {
    OPS.iter().position(|m| *m == mnemonic)
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
}

const RCP_UNIT: Unit = Unit { seg_bits: 18, sq_shift: 1, qc: 64, qlo: 32, qhi: 64, switch_bits: 29 };
const RSQ_UNIT: Unit = Unit { seg_bits: 18, sq_shift: 1, qc: 128, qlo: 32, qhi: 64, switch_bits: 29 };
const SQRT_UNIT: Unit = Unit { seg_bits: 19, sq_shift: 2, qc: 64, qlo: 64, qhi: 128, switch_bits: 30 };

fn internal(row: &[i64; 7], u: &Unit, t: i64) -> i64 {
    let [c0, c1, c2, q2, m3, phi, dn] = *row;
    let p = c1 * t;
    let q = if p.abs() < (1i64 << (u.switch_bits + u.seg_bits)) { u.qlo } else { u.qhi };
    let coarse = rne(c0 + rne(p, q << u.seg_bits) * q, u.qc) * u.qc;
    let a = t >> u.sq_shift;
    let s = (a * a) >> 10;
    let h = c2 + q2 * floor_div(m3 * t + phi, dn);
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
            let a = if mn == 0 { 1i64 << 35 } else { internal(&RCP[(mn >> 18) as usize], &RCP_UNIT, (mn & 0x3_ffff) as i64) };
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
            let a = if mn == 0 && par == 0 { 1i64 << 36 } else { internal(&tab[(mn >> 18) as usize], &RSQ_UNIT, (mn & 0x3_ffff) as i64) };
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
            let a = internal(&tab[(mn >> 19) as usize], &SQRT_UNIT, (mn & 0x7_ffff) as i64);
            pack(0, eh, a, 12, if mn >> 19 == 15 { 0 } else { -512 })
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
int64_t tb_t32_internal(uint row, int64_t t, uint sb, uint sqs, int64_t qc, int64_t qlo, int64_t qhi, uint swb) {
    int64_t c0 = TB_T32_TAB[row * 7u + 0u], c1 = TB_T32_TAB[row * 7u + 1u], c2 = TB_T32_TAB[row * 7u + 2u];
    int64_t q2 = TB_T32_TAB[row * 7u + 3u], m3 = TB_T32_TAB[row * 7u + 4u], phi = TB_T32_TAB[row * 7u + 5u], dn = TB_T32_TAB[row * 7u + 6u];
    int64_t p = c1 * t;
    int64_t pm = p < 0 ? -p : p;
    int64_t q = pm < (((int64_t)1) << (swb + sb)) ? qlo : qhi;
    int64_t coarse = tb_t32_rne(c0 + tb_t32_rne(p, q << sb) * q, qc) * qc;
    int64_t a = t >> sqs;
    int64_t s = (a * a) >> 10;
    int64_t hh = c2 + q2 * tb_t32_fdiv(m3 * t + phi, dn);
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
    uint s = x & 0x80000000u, ex = (x >> 23) & 0xffu;
    uint mn = ex == 0u ? 0u : (x & 0x7fffffu);
    if (ex == 255u && mn != 0u) return x;
    bool zero = ex == 0u, inf = ex == 255u;
    int e = (int)ex - 127;
    if (op <= 1u) {
        if (zero) return s | 0x7f800000u;
        if (inf) return s;
        int64_t a = mn == 0u ? (((int64_t)1) << 35) : tb_t32_internal(mn >> 18, (int64_t)(mn & 0x3ffffu), 18u, 1u, 64, 32, 64, 29u);
        return tb_t32_pack(s, -e - 1, a, 11u, (mn >> 18) == 0u ? 0 : -256);
    }
    if (zero) return op == 2u ? (s | 0x7f800000u) : s;
    if (s != 0u) return 0xffc00000u;
    int par = e & 1;
    int eh = (e - par) / 2;
    if (op == 2u) {
        if (inf) return 0u;
        int64_t a = (mn == 0u && par == 0) ? (((int64_t)1) << 36) : tb_t32_internal(32u + 32u * (uint)par + (mn >> 18), (int64_t)(mn & 0x3ffffu), 18u, 1u, 128, 32, 64, 29u);
        return tb_t32_pack(0u, -eh - 1, a, 12u, (mn >> 18) == 0u ? 0 : -512);
    }
    if (inf) return 0x7f800000u;
    int64_t a = tb_t32_internal(96u + 16u * (uint)par + (mn >> 19), (int64_t)(mn & 0x7ffffu), 19u, 2u, 64, 64, 128, 30u);
    return tb_t32_pack(0u, eh, a, 12u, (mn >> 19) == 15u ? 0 : -512);
}
"#;

pub fn hlsl() -> String {
    let rows: Vec<&[i64; 7]> = RCP.iter().chain(RSQ0.iter()).chain(RSQ1.iter()).chain(SQRT0.iter()).chain(SQRT1.iter()).collect();
    let vals: Vec<String> = rows.iter().flat_map(|r| r.iter()).map(|v| format!("{v}")).collect();
    let mut h = String::new();
    let _ = writeln!(h, "static const int64_t TB_T32_TAB[{}] = {{ {} }};", vals.len(), vals.join(", "));
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

    #[test]
    fn f32_rcp_rsq_sqrt_match_rdna2_on_every_mantissa() {
        for (i, &(m, want)) in rdna2_hw::RANGE_DIGESTS.iter().enumerate() {
            assert_eq!(OPS[i], m);
            let n = if i == 1 { 1u32 << 23 } else { 1u32 << 24 };
            let got = fnv((0..n).map(|k| eval(i, 0x3f80_0000 + k)));
            assert_eq!(got, want, "{m}: model digest differs from the {} record", rdna2_hw::SOURCE);
        }
    }

    #[test]
    fn f32_rcp_rsq_sqrt_match_rdna2_across_the_float_range() {
        for (k, &(m, want)) in rdna2_hw::EDGE_DIGESTS.iter().enumerate() {
            let op = k / 2;
            assert!(m.starts_with(OPS[op]));
            let got = fnv((0..1u32 << 20).map(|g| eval(op, g.wrapping_mul(4099))));
            assert_eq!(got, want, "{m}: model digest differs from the {} record", rdna2_hw::SOURCE);
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
