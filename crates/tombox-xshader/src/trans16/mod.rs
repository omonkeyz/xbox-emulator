mod tables;

#[cfg(test)]
mod rdna2_hw;

use std::fmt::Write;
use tables::{EXCEPTIONS, EXP2_T, LN2_Q32, LOG2_T, SIN_C};

pub const OPS: [&str; 7] = ["v_rcp_f16", "v_rsq_f16", "v_sqrt_f16", "v_log_f16", "v_exp_f16", "v_sin_f16", "v_cos_f16"];

pub fn op_index(mnemonic: &str) -> Option<usize> {
    OPS.iter().position(|m| *m == mnemonic)
}

fn r16(s: u32, mag: u64, ex: i32) -> u16 {
    if mag == 0 {
        return (s << 15) as u16;
    }
    let msb = 63 - mag.leading_zeros() as i32;
    let e16 = msb + ex;
    if e16 > 15 {
        return ((s << 15) | 0x7c00) as u16;
    }
    let sh = if e16 >= -14 { msb - 10 } else { -24 - ex };
    let q = if sh <= 0 {
        mag << -sh
    } else if sh >= 64 {
        0
    } else {
        let q = mag >> sh;
        let rem = mag & ((1u64 << sh) - 1);
        let half = 1u64 << (sh - 1);
        if rem > half || (rem == half && q & 1 != 0) { q + 1 } else { q }
    };
    let bits = if e16 >= -14 { (((e16 + 15) as u64) << 10) + q - 1024 } else { q };
    ((s << 15) as u64 | bits) as u16
}

fn split(h: u32) -> (u32, u64, i32) {
    let (s, e, m) = (h >> 15, (h >> 10) & 31, h & 0x3ff);
    if e == 0 && m == 0 {
        (s, 0, 0)
    } else if e == 0 {
        let k = 10 - (31 - m.leading_zeros()) as i32;
        (s, (m as u64) << k, -24 - k)
    } else {
        (s, (m | 0x400) as u64, e as i32 - 25)
    }
}

fn fixed24(h: u32) -> i64 {
    let (s, mm, e) = split(h);
    let x = if e + 24 >= 0 { (mm << (e + 24)) as i64 } else { (mm >> -(e + 24)) as i64 };
    if s != 0 { -x } else { x }
}

fn sinpoly(v: i64) -> i64 {
    let w = (v * v) >> 24;
    let mut acc = SIN_C[6];
    for k in (0..6).rev() {
        acc = SIN_C[k] + ((acc * w) >> 24);
    }
    (acc * v) >> 24
}

fn compute(op: usize, h: u32) -> u16 {
    let (s, e, m) = (h >> 15, (h >> 10) & 31, h & 0x3ff);
    let (nan, inf, zero) = (e == 31 && m != 0, e == 31 && m == 0, e == 0 && m == 0);
    if nan {
        return h as u16;
    }
    match op {
        0 => {
            if zero {
                return ((s << 15) | 0x7c00) as u16;
            }
            if inf {
                return (s << 15) as u16;
            }
            let (_, mm, ee) = split(h);
            let (q, r) = ((1u64 << 24) / mm, (1u64 << 24) % mm);
            r16(s, (q << 1) | (r != 0) as u64, -24 - ee - 1)
        }
        1 => {
            if zero {
                return ((s << 15) | 0x7c00) as u16;
            }
            if s != 0 {
                return 0xfe00;
            }
            if inf {
                return 0;
            }
            let (_, mut mm, mut ee) = split(h);
            if ee & 1 != 0 {
                mm <<= 1;
                ee -= 1;
            }
            let (a, rr) = ((1u64 << 40) / mm, (1u64 << 40) % mm);
            let sq = a.isqrt();
            r16(0, (sq << 1) | (sq * sq != a || rr != 0) as u64, -(ee / 2) - 21)
        }
        2 => {
            if zero {
                return h as u16;
            }
            if s != 0 {
                return 0xfe00;
            }
            if inf {
                return 0x7c00;
            }
            let (_, mut mm, mut ee) = split(h);
            if ee & 1 != 0 {
                mm <<= 1;
                ee -= 1;
            }
            let n = mm << 16;
            let sq = n.isqrt();
            r16(0, (sq << 1) | (sq * sq != n) as u64, (ee - 16) / 2 - 1)
        }
        3 => {
            if zero {
                return 0xfc00;
            }
            if s != 0 {
                return 0xfe00;
            }
            if inf {
                return 0x7c00;
            }
            let (_, mm, ee) = split(h);
            let v = (((ee + 10) as i64) << 32) + LOG2_T[(mm - 1024) as usize] as i64;
            if v < 0 { r16(1, (-v) as u64, -32) } else { r16(0, v as u64, -32) }
        }
        4 => {
            if inf {
                return if s != 0 { 0 } else { 0x7c00 };
            }
            let x = fixed24(h).clamp(-(26i64 << 24), 16i64 << 24);
            let n = x >> 24;
            let f = (x - (n << 24)) as u64;
            let (j, lo) = (f >> 14, f & 0x3fff);
            let p = (1u64 << 32) + EXP2_T[j as usize] as u64;
            let c = (lo * LN2_Q32) >> 24;
            let c2 = (c * c) >> 33;
            r16(0, p + ((p * (c + c2)) >> 32), n as i32 - 32)
        }
        _ => {
            if inf {
                return 0xfe00;
            }
            if zero && op == 5 {
                return h as u16;
            }
            let mut r = (fixed24(h) & 0xff_ffff) as u32;
            if op == 6 {
                r = (r + (1 << 22)) & 0xff_ffff;
            }
            let (q, u) = (r >> 22, r & ((1 << 22) - 1));
            let v = if q & 1 != 0 { (1 << 22) - u } else { u };
            if v == 0 {
                return 0;
            }
            r16((q >= 2) as u32, sinpoly(v as i64) as u64, -32)
        }
    }
}

pub fn eval(op: usize, h: u16) -> u16 {
    let y = compute(op, h as u32);
    EXCEPTIONS.iter().find(|&&(o, x, _)| o as usize == op && x == h).map_or(y, |&(_, _, v)| v)
}

const HLSL_BODY: &str = r#"uint tb_t16_r(uint s, uint64_t mag, int ex) {
    if (mag == 0) return s << 15;
    uint hi = (uint)(mag >> 32);
    int msb = hi != 0u ? 32 + (int)firstbithigh(hi) : (int)firstbithigh((uint)mag);
    int e16 = msb + ex;
    if (e16 > 15) return (s << 15) | 0x7c00u;
    int sh = e16 >= -14 ? msb - 10 : -24 - ex;
    uint64_t q;
    if (sh <= 0) {
        q = mag << (uint)(-sh);
    } else if (sh >= 64) {
        q = 0;
    } else {
        q = mag >> (uint)sh;
        uint64_t rem = mag & ((((uint64_t)1) << (uint)sh) - 1);
        uint64_t half = ((uint64_t)1) << (uint)(sh - 1);
        if (rem > half || (rem == half && (q & 1) != 0)) q += 1;
    }
    uint bits = e16 >= -14 ? (uint)(((uint64_t)(e16 + 15) << 10) + q - 1024) : (uint)q;
    return (s << 15) | bits;
}
void tb_t16_split(uint h, out uint64_t mm, out int ee) {
    uint e = (h >> 10) & 31u, m = h & 0x3ffu;
    if (e == 0u) {
        int k = 10 - (int)firstbithigh(m);
        mm = ((uint64_t)m) << (uint)k;
        ee = -24 - k;
    } else {
        mm = (uint64_t)(m | 0x400u);
        ee = (int)e - 25;
    }
}
int64_t tb_t16_fixed24(uint h) {
    uint64_t mm; int ee;
    tb_t16_split(h, mm, ee);
    int64_t x = ee + 24 >= 0 ? (int64_t)(mm << (uint)(ee + 24)) : (int64_t)(mm >> (uint)(-(ee + 24)));
    return (h >> 15) != 0u ? -x : x;
}
uint64_t tb_t16_isqrt(uint64_t n) {
    uint64_t r = (uint64_t)sqrt((float)n);
    if (r * r > n) r -= 1;
    if (r * r > n) r -= 1;
    if ((r + 1) * (r + 1) <= n) r += 1;
    if ((r + 1) * (r + 1) <= n) r += 1;
    return r;
}
int64_t tb_t16_sinpoly(int64_t v) {
    int64_t w = (v * v) >> 24;
    int64_t acc = TB_T16_SIN[6];
    [unroll] for (int k = 5; k >= 0; k--) acc = TB_T16_SIN[k] + ((acc * w) >> 24);
    return (acc * v) >> 24;
}
uint tb_t16_x(uint op, uint h, uint y) {
    uint key = (op << 16) | h;
    [loop] for (uint i = 0; i < TB_T16_NX; i++) if (TB_T16_XK[i] == key) return TB_T16_XV[i];
    return y;
}
uint tb_t16(uint op, uint h) {
    h &= 0xffffu;
    uint s = h >> 15, e = (h >> 10) & 31u, m = h & 0x3ffu;
    bool nan = e == 31u && m != 0u, inf = e == 31u && m == 0u, zero = e == 0u && m == 0u;
    if (nan) return h;
    uint64_t mm; int ee;
    uint y;
    if (op == 0u) {
        if (zero) return (s << 15) | 0x7c00u;
        if (inf) return s << 15;
        tb_t16_split(h, mm, ee);
        uint q = (1u << 24) / (uint)mm, r = (1u << 24) % (uint)mm;
        y = tb_t16_r(s, ((uint64_t)q << 1) | (r != 0u ? 1 : 0), -24 - ee - 1);
    } else if (op == 1u) {
        if (zero) return (s << 15) | 0x7c00u;
        if (s != 0u) return 0xfe00u;
        if (inf) return 0u;
        tb_t16_split(h, mm, ee);
        if ((ee & 1) != 0) { mm <<= 1; ee -= 1; }
        uint64_t a = (((uint64_t)1) << 40) / mm, rr = (((uint64_t)1) << 40) % mm;
        uint64_t sq = tb_t16_isqrt(a);
        y = tb_t16_r(0u, (sq << 1) | ((sq * sq != a || rr != 0) ? 1 : 0), -(ee / 2) - 21);
    } else if (op == 2u) {
        if (zero) return h;
        if (s != 0u) return 0xfe00u;
        if (inf) return 0x7c00u;
        tb_t16_split(h, mm, ee);
        if ((ee & 1) != 0) { mm <<= 1; ee -= 1; }
        uint64_t n = mm << 16;
        uint64_t sq = tb_t16_isqrt(n);
        y = tb_t16_r(0u, (sq << 1) | (sq * sq != n ? 1 : 0), (ee - 16) / 2 - 1);
    } else if (op == 3u) {
        if (zero) return 0xfc00u;
        if (s != 0u) return 0xfe00u;
        if (inf) return 0x7c00u;
        tb_t16_split(h, mm, ee);
        int64_t v = (((int64_t)(ee + 10)) << 32) + (int64_t)TB_T16_LOG2[(uint)(mm - 1024)];
        y = v < 0 ? tb_t16_r(1u, (uint64_t)(-v), -32) : tb_t16_r(0u, (uint64_t)v, -32);
    } else if (op == 4u) {
        if (inf) return s != 0u ? 0u : 0x7c00u;
        int64_t x = tb_t16_fixed24(h);
        int64_t xlo = -(((int64_t)26) << 24), xhi = ((int64_t)16) << 24;
        x = x < xlo ? xlo : (x > xhi ? xhi : x);
        int64_t n = x >> 24;
        uint64_t f = (uint64_t)(x - (n << 24));
        uint64_t j = f >> 14, lo = f & 0x3fff;
        uint64_t p = (((uint64_t)1) << 32) + (uint64_t)TB_T16_EXP2[(uint)j];
        uint64_t c = (lo * TB_T16_LN2) >> 24;
        uint64_t c2 = (c * c) >> 33;
        y = tb_t16_r(0u, p + ((p * (c + c2)) >> 32), (int)n - 32);
    } else {
        if (inf) return 0xfe00u;
        if (zero && op == 5u) return h;
        uint r = (uint)(tb_t16_fixed24(h) & 0xffffff);
        if (op == 6u) r = (r + (1u << 22)) & 0xffffffu;
        uint q = r >> 22, u = r & ((1u << 22) - 1u);
        uint v = (q & 1u) != 0u ? (1u << 22) - u : u;
        if (v == 0u) return 0u;
        y = tb_t16_r(q >= 2u ? 1u : 0u, (uint64_t)tb_t16_sinpoly((int64_t)v), -32);
    }
    return tb_t16_x(op, h, y);
}
"#;

fn join<T>(xs: impl Iterator<Item = T>, f: impl Fn(T) -> String) -> String {
    xs.map(f).collect::<Vec<_>>().join(", ")
}

pub fn hlsl() -> String {
    let nx = EXCEPTIONS.len().max(1);
    let mut keys: Vec<u32> = EXCEPTIONS.iter().map(|&(o, x, _)| ((o as u32) << 16) | x as u32).collect();
    let mut vals: Vec<u32> = EXCEPTIONS.iter().map(|&(_, _, v)| v as u32).collect();
    if keys.is_empty() {
        keys.push(u32::MAX);
        vals.push(0);
    }
    let mut h = String::new();
    let _ = writeln!(h, "static const uint TB_T16_LOG2[1024] = {{ {} }};", join(LOG2_T.iter(), |x| format!("{x:#x}u")));
    let _ = writeln!(h, "static const uint TB_T16_EXP2[1024] = {{ {} }};", join(EXP2_T.iter(), |x| format!("{x:#x}u")));
    let _ = writeln!(h, "static const int64_t TB_T16_SIN[7] = {{ {} }};", join(SIN_C.iter(), |x| format!("{x}")));
    let _ = writeln!(h, "static const uint64_t TB_T16_LN2 = {LN2_Q32:#x};");
    let _ = writeln!(h, "static const uint TB_T16_NX = {}u;", EXCEPTIONS.len());
    let _ = writeln!(h, "static const uint TB_T16_XK[{nx}] = {{ {} }};", join(keys.iter(), |x| format!("{x:#x}u")));
    let _ = writeln!(h, "static const uint TB_T16_XV[{nx}] = {{ {} }};", join(vals.iter(), |x| format!("{x:#x}u")));
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
    fn f16_transcendentals_match_rdna2_on_every_input() {
        for (i, &(m, want)) in rdna2_hw::DIGESTS.iter().enumerate() {
            assert_eq!(OPS[i], m);
            let got = fnv((0..65536u32).map(|h| eval(i, h as u16) as u32));
            assert_eq!(got, want, "{m}: model digest differs from the {} record", rdna2_hw::SOURCE);
        }
    }

    #[test]
    fn f16_transcendental_specials() {
        let cases: [(usize, u16, u16); 16] = [
            (0, 0x0000, 0x7c00), (0, 0x8000, 0xfc00), (0, 0x7c00, 0x0000), (0, 0x7bff, 0x0100),
            (1, 0x8000, 0xfc00), (1, 0xbc00, 0xfe00), (1, 0xfc00, 0xfe00),
            (2, 0x8000, 0x8000), (2, 0xbc00, 0xfe00),
            (3, 0x8000, 0xfc00), (3, 0x3c00, 0x0000),
            (4, 0xfc00, 0x0000), (4, 0x8000, 0x3c00),
            (5, 0x8000, 0x8000), (5, 0xbc00, 0x0000), (6, 0x7c00, 0xfe00),
        ];
        for (op, x, want) in cases {
            assert_eq!(eval(op, x), want, "{} {x:#06x}", OPS[op]);
        }
        for op in 0..7 {
            assert_eq!(eval(op, 0x7d00), 0x7d00, "{} keeps a signalling NaN", OPS[op]);
        }
    }
}
