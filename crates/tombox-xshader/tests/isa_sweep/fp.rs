pub fn f(b: u32) -> f32 {
    f32::from_bits(b)
}

pub fn b(x: f32) -> u32 {
    x.to_bits()
}

pub fn d(lo: u32, hi: u32) -> f64 {
    f64::from_bits(lo as u64 | ((hi as u64) << 32))
}

pub fn nan32(x: u32) -> bool {
    x & 0x7fff_ffff > 0x7f80_0000
}

pub fn snan32(x: u32) -> bool {
    nan32(x) && x & 0x0040_0000 == 0
}

pub fn den32(x: u32) -> bool {
    x & 0x7f80_0000 == 0 && x & 0x007f_ffff != 0
}

pub fn nan16(x: u32) -> bool {
    x & 0x7fff > 0x7c00
}

pub fn snan16(x: u32) -> bool {
    nan16(x) && x & 0x0200 == 0
}

pub fn den16(x: u32) -> bool {
    x & 0x7c00 == 0 && x & 0x3ff != 0
}

pub fn nan64(x: u64) -> bool {
    x & 0x7fff_ffff_ffff_ffff > 0x7ff0_0000_0000_0000
}

pub fn snan64(x: u64) -> bool {
    nan64(x) && x & 0x0008_0000_0000_0000 == 0
}

pub const QNAN32: u32 = 0x7fc0_0000;
pub const QNAN16: u32 = 0x7e00;
pub const QNAN64: u64 = 0x7ff8_0000_0000_0000;

pub fn h2f(h: u32) -> f32 {
    let h = h & 0xffff;
    let s = (h >> 15) << 31;
    let e = (h >> 10) & 31;
    let m = h & 0x3ff;
    if e == 31 {
        return f32::from_bits(s | 0x7f80_0000 | (m << 13));
    }
    if e == 0 {
        let v = m as f32 * (1.0 / 16_777_216.0);
        return if s != 0 { -v } else { v };
    }
    f32::from_bits(s | ((e + 112) << 23) | (m << 13))
}

pub fn h2d(h: u32) -> f64 {
    h2f(h) as f64
}

fn round_to_f16(s: u32, mag: f64, rtz: bool) -> u32 {
    if mag == 0.0 {
        return s;
    }
    let e = mag.log2().floor() as i32;
    let e = if 2f64.powi(e) > mag { e - 1 } else if 2f64.powi(e + 1) <= mag { e + 1 } else { e };
    let q = if e < -14 { -24 } else { e - 10 };
    let scaled = mag / 2f64.powi(q);
    let fl = scaled.floor();
    let fr = scaled - fl;
    let mut n = fl as u64;
    if !rtz && (fr > 0.5 || (fr == 0.5 && n & 1 == 1)) {
        n += 1;
    }
    let v = n as f64 * 2f64.powi(q);
    if v >= 65536.0 || (rtz && v > 65504.0) {
        return if rtz { s | 0x7bff } else { s | 0x7c00 };
    }
    if v > 65504.0 {
        return s | 0x7c00;
    }
    if v < 6.103515625e-5 {
        return s | (v / 2f64.powi(-24)) as u32;
    }
    let e2 = v.log2().floor() as i32;
    let e2 = if 2f64.powi(e2) > v { e2 - 1 } else if 2f64.powi(e2 + 1) <= v { e2 + 1 } else { e2 };
    let m = (v / 2f64.powi(e2 - 10)) as u32 - 1024;
    s | (((e2 + 15) as u32) << 10) | m
}

pub fn d2h(x: f64) -> u32 {
    let s = if x.is_sign_negative() { 0x8000 } else { 0 };
    if x.is_nan() {
        return QNAN16;
    }
    if x.is_infinite() {
        return s | 0x7c00;
    }
    round_to_f16(s, x.abs(), false)
}

pub fn f2h_rne(x: f32) -> u32 {
    let bits = x.to_bits();
    if nan32(bits) {
        return ((bits >> 16) & 0x8000) | 0x7e00 | ((bits >> 13) & 0x3ff);
    }
    d2h(x as f64)
}

pub fn f2h_rtz(x: f32) -> u32 {
    let bits = x.to_bits();
    if nan32(bits) {
        return ((bits >> 16) & 0x8000) | 0x7e00 | ((bits >> 13) & 0x3ff);
    }
    let s = if x.is_sign_negative() { 0x8000 } else { 0 };
    if x.is_infinite() {
        return s | 0x7c00;
    }
    round_to_f16(s, (x as f64).abs(), true)
}

pub fn q16(h: u32) -> u32 {
    if nan16(h) { QNAN16 } else { h }
}

pub fn ftz32(x: u32) -> u32 {
    if den32(x) { x & 0x8000_0000 } else { x }
}

pub fn ldexp32(x: f32, e: i32) -> f32 {
    let e = e.clamp(-400, 400);
    let mut r = x as f64;
    let mut k = e;
    while k > 0 {
        let s = k.min(500);
        r *= 2f64.powi(s);
        k -= s;
    }
    while k < 0 {
        let s = (-k).min(500);
        r /= 2f64.powi(s);
        k += s;
    }
    r as f32
}

pub fn ldexp64(x: f64, e: i32) -> f64 {
    let e = e.clamp(-2200, 2200);
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let bits = x.to_bits();
    let ex = ((bits >> 52) & 0x7ff) as i32;
    let (m, ex) = if ex == 0 {
        let y = x * 2f64.powi(64);
        (y, ((y.to_bits() >> 52) & 0x7ff) as i32 - 64)
    } else {
        (x, ex)
    };
    let ne = ex + e;
    let frac = f64::from_bits((m.to_bits() & 0x800f_ffff_ffff_ffff) | (1023u64 << 52));
    if ne >= 2047 {
        return if x < 0.0 { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    if ne >= 1 {
        return f64::from_bits((frac.to_bits() & 0x800f_ffff_ffff_ffff) | ((ne as u64) << 52));
    }
    if ne < -1100 {
        return if x < 0.0 { -0.0 } else { 0.0 };
    }
    let up = frac * 2f64.powi(ne - 1023 + 600);
    up * 2f64.powi(-600)
}

pub fn rne32(x: f32) -> f32 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 { (2.0 * (x / 2.0).round()).copysign(x) } else { r.copysign(x) }
}

pub fn rne64(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 { (2.0 * (x / 2.0).round()).copysign(x) } else { r.copysign(x) }
}

pub fn cvt_i32(x: f64) -> u32 {
    if x.is_nan() { 0 } else if x >= 2147483647.0 { 0x7fff_ffff } else if x <= -2147483648.0 { 0x8000_0000 } else { x.trunc() as i32 as u32 }
}

pub fn cvt_u32(x: f64) -> u32 {
    if x.is_nan() || x <= 0.0 { 0 } else if x >= 4294967295.0 { 0xffff_ffff } else { x.trunc() as u32 }
}

pub fn min_num32(a: u32, b: u32) -> Option<u32> {
    if snan32(a) || snan32(b) {
        return None;
    }
    if nan32(a) && nan32(b) {
        return Some(QNAN32);
    }
    if nan32(a) {
        return Some(b);
    }
    if nan32(b) {
        return Some(a);
    }
    let (x, y) = (f(a), f(b));
    if x == 0.0 && y == 0.0 && a != b {
        return None;
    }
    Some(if x <= y { a } else { b })
}

pub fn max_num32(a: u32, b: u32) -> Option<u32> {
    if snan32(a) || snan32(b) {
        return None;
    }
    if nan32(a) && nan32(b) {
        return Some(QNAN32);
    }
    if nan32(a) {
        return Some(b);
    }
    if nan32(b) {
        return Some(a);
    }
    let (x, y) = (f(a), f(b));
    if x == 0.0 && y == 0.0 && a != b {
        return None;
    }
    Some(if x >= y { a } else { b })
}

pub fn min_num16(a: u32, b: u32) -> Option<u32> {
    let (a, b) = (a & 0xffff, b & 0xffff);
    if snan16(a) || snan16(b) {
        return None;
    }
    if nan16(a) && nan16(b) {
        return Some(QNAN16);
    }
    if nan16(a) {
        return Some(b);
    }
    if nan16(b) {
        return Some(a);
    }
    let (x, y) = (h2f(a), h2f(b));
    if x == 0.0 && y == 0.0 && a != b {
        return None;
    }
    Some(if x <= y { a } else { b })
}

pub fn max_num16(a: u32, b: u32) -> Option<u32> {
    let (a, b) = (a & 0xffff, b & 0xffff);
    if snan16(a) || snan16(b) {
        return None;
    }
    if nan16(a) && nan16(b) {
        return Some(QNAN16);
    }
    if nan16(a) {
        return Some(b);
    }
    if nan16(b) {
        return Some(a);
    }
    let (x, y) = (h2f(a), h2f(b));
    if x == 0.0 && y == 0.0 && a != b {
        return None;
    }
    Some(if x >= y { a } else { b })
}

pub fn min_num64(a: u64, b: u64) -> Option<u64> {
    if snan64(a) || snan64(b) {
        return None;
    }
    if nan64(a) && nan64(b) {
        return Some(QNAN64);
    }
    if nan64(a) {
        return Some(b);
    }
    if nan64(b) {
        return Some(a);
    }
    let (x, y) = (f64::from_bits(a), f64::from_bits(b));
    if x == 0.0 && y == 0.0 && a != b {
        return None;
    }
    Some(if x <= y { a } else { b })
}

pub fn max_num64(a: u64, b: u64) -> Option<u64> {
    if snan64(a) || snan64(b) {
        return None;
    }
    if nan64(a) && nan64(b) {
        return Some(QNAN64);
    }
    if nan64(a) {
        return Some(b);
    }
    if nan64(b) {
        return Some(a);
    }
    let (x, y) = (f64::from_bits(a), f64::from_bits(b));
    if x == 0.0 && y == 0.0 && a != b {
        return None;
    }
    Some(if x >= y { a } else { b })
}

pub fn class_bits32(x: u32) -> u32 {
    let neg = x >> 31 != 0;
    if nan32(x) {
        return if snan32(x) { 1 } else { 2 };
    }
    let a = x & 0x7fff_ffff;
    let k = if a == 0x7f80_0000 { 9 } else if a >= 0x0080_0000 { 8 } else if a != 0 { 7 } else { 6 };
    1 << if neg { 11 - k } else { k }
}

pub fn class_bits16(x: u32) -> u32 {
    let x = x & 0xffff;
    let neg = x >> 15 != 0;
    if nan16(x) {
        return if snan16(x) { 1 } else { 2 };
    }
    let a = x & 0x7fff;
    let k = if a == 0x7c00 { 9 } else if a >= 0x0400 { 8 } else if a != 0 { 7 } else { 6 };
    1 << if neg { 11 - k } else { k }
}

pub fn class_bits64(x: u64) -> u32 {
    let neg = x >> 63 != 0;
    if nan64(x) {
        return if snan64(x) { 1 } else { 2 };
    }
    let a = x & 0x7fff_ffff_ffff_ffff;
    let k = if a == 0x7ff0_0000_0000_0000 { 9 } else if a >= 0x0010_0000_0000_0000 { 8 } else if a != 0 { 7 } else { 6 };
    1 << if neg { 11 - k } else { k }
}

pub fn fcmp(code: u32, a: f64, b: f64) -> bool {
    let un = a.is_nan() || b.is_nan();
    match code & 15 {
        0 => false,
        1 => a < b,
        2 => a == b,
        3 => a <= b,
        4 => a > b,
        5 => !un && a != b,
        6 => a >= b,
        7 => !un,
        8 => un,
        9 => !(a >= b),
        10 => !(!un && a != b),
        11 => !(a > b),
        12 => !(a <= b),
        13 => !(a == b),
        14 => !(a < b),
        _ => true,
    }
}

pub fn icmp(code: u32, a: i128, b: i128) -> bool {
    match code & 7 {
        0 => false,
        1 => a < b,
        2 => a == b,
        3 => a <= b,
        4 => a > b,
        5 => a != b,
        6 => a >= b,
        _ => true,
    }
}

pub fn f32q(x: f32) -> u32 {
    if x.is_nan() { QNAN32 } else { x.to_bits() }
}

pub fn f64q(x: f64) -> u64 {
    if x.is_nan() { QNAN64 } else { x.to_bits() }
}

pub fn h16(x: f64) -> u32 {
    d2h(x)
}
