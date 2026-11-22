fn re(v: u64, s: u32) -> u64 {
    if s == 0 {
        return v;
    }
    let w = v >> s;
    let r = v & ((1u64 << s) - 1);
    let h = 1u64 << (s - 1);
    w + (r > h || (r == h && w & 1 != 0)) as u64
}

fn bw32(v: u32) -> u32 {
    32 - v.leading_zeros()
}

fn bw64(v: u64) -> u32 {
    64 - v.leading_zeros()
}

const EXP: [[u32; 4]; 32] = [
    [268435456, 5814541, 8060268, 29], [274313427, 5941862, 8236765, 29], [280320109, 6071972, 8417127, 30], [286458319, 6204931, 8601438, 31],
    [292730941, 6340801, 8789785, 31], [299140913, 6479647, 8982256, 32], [305691246, 6621533, 9178942, 33], [312385013, 6766525, 9379934, 33],
    [319225354, 6914693, 9585328, 34], [326215479, 7066105, 9795219, 35], [333358667, 7220833, 10009707, 36], [340658273, 7378948, 10228891, 36],
    [348117717, 7540526, 10452874, 37], [355740503, 7705642, 10681762, 38], [363530205, 7874374, 10915663, 39], [371490480, 8046800, 11154685, 40],
    [379625062, 8223002, 11398940, 41], [387937769, 8403062, 11648545, 42], [396432501, 8587066, 11903614, 42], [405113242, 8775098, 12164270, 43],
    [413984066, 8967247, 12430633, 44], [423049137, 9163605, 12702828, 45], [432312707, 9364261, 12980984, 46], [441779122, 9569312, 13265230, 47],
    [451452826, 9778853, 13555701, 48], [461338356, 9992982, 13852532, 49], [471440350, 10211799, 14155863, 50], [481763549, 10435409, 14465836, 52],
    [492312797, 10663914, 14782597, 53], [503093043, 10897424, 15106293, 54], [514109347, 11136046, 15437078, 55], [525366876, 11379894, 15775106, 56],
];

pub fn exp(x: u32) -> u32 {
    let mag = x & 0x7fff_ffff;
    if mag >= 0x7f80_0000 {
        if mag > 0x7f80_0000 {
            return x;
        }
        return if x >> 31 != 0 { 0 } else { 0x7f80_0000 };
    }
    if mag < 0x3380_0000 {
        return 0x3f80_0000;
    }
    let neg = x >> 31 != 0;
    if !neg && mag >= 0x4300_0000 {
        return 0x7f80_0000;
    }
    if neg && mag > 0x42fc_0000 {
        return 0;
    }
    let e = (mag >> 23) as i32 - 127;
    let m = ((mag & 0x7f_ffff) | 0x80_0000) as u64;
    let sh = e + 6;
    let abs = if sh >= 0 { m << sh } else { m >> -sh };
    let phase: i64 = if neg { -(abs as i64) - 1 } else { abs as i64 };
    let fr = (phase as u32 & 0xff_ffff) as u64;
    let c = EXP[((phase as u64 >> 24) & 31) as usize];
    let lp = c[1] as u64 * fr;
    let lin = if lp >= 1 << 47 { re(lp, 24) } else { re(lp, 23) >> 1 };
    let co = fr >> 6;
    let sq = (co * co) >> 12;
    let inner = re(((c[2] as u64) << 7) + c[3] as u64 * co, 7);
    let pr = inner * sq;
    let quad = if pr >= 1 << 47 { re(pr, 24) * 2 } else { re(pr, 23) };
    let sum = ((c[0] as u64 + lin) << 8) + quad;
    (((phase >> 29) + 126) * 0x80_0000 + re(sum, 13) as i64) as u32
}

const LOG: [[u32; 4]; 33] = [
    [15, 12102179, 12095174, 1925858], [47667825, 11735468, 11378345, 1799053], [93912517, 11390291, 10714741, 1610016], [138816594, 11064855, 10111507, 1477533],
    [182455591, 10757500, 9557803, 1359325], [224898847, 10466758, 9048378, 1253542], [266210154, 10191316, 8578611, 1158417], [306448312, 9930003, 8144494, 1072666],
    [345667679, 9681753, 7742500, 995087], [383918558, 9445614, 7369567, 924970], [421247645, 9220718, 7022931, 861214], [457698312, 9006284, 6700175, 803096],
    [493310964, 8801596, 6399165, 750066], [528123258, 8606005, 6118007, 701755], [562170399, 8418918, 5854955, 657310], [595485263, 8239793, 5608532, 616715],
    [628098719, 8068131, 5377341, 579376], [660039693, 7903475, 5160150, 544955], [691335339, 7745406, 4955875, 513361], [722011229, 7593536, 4763477, 484038],
    [752091435, 7447506, 4582059, 456830], [781598650, 7306987, 4410807, 431594], [810554297, 7171673, 4248984, 408222], [838978616, 7041279, 4095937, 386761],
    [866890752, 6915542, 3950988, 366593], [894308853, 6794217, 3813578, 347636], [921250083, 6677075, 3683220, 330396], [947730761, 6563905, 3559471, 313859],
    [973766367, 6454506, 3441820, 298469], [999371610, 6348694, 3329908, 284397], [1024560488, 6246298, 3223482, 270890], [1049346329, 6147160, 3121894, 258238],
    [47667912, 11735350, 11363743, 1719989],
];

fn log_below_one(bits: u32) -> u32 {
    let k = 0x3f80_0000 - bits;
    let up = k >= 0x2_0000;
    let (c1, c2, c3) = if up { (12102221u64, 12097323u64, 2088428u64) } else { (12102203, 12101979, 2035248) };
    let w = bw32(k);
    let nz = if w < 18 { 4 * ((18 - w) / 4) } else { 0 };
    let ls = 14u32.saturating_sub(nz);
    let ss = 12u32.saturating_sub(nz);
    let k = k as u64;
    let lin = ((2 * c1 * k) >> ls) << ls;
    let sq = ((k * k) >> ss) << ss;
    let inner = re((c2 << 22) + c3 * (k - 1), 22);
    let ps = 38 - nz;
    let quad = (inner * sq + ((1u64 << ps) - 1)) >> ps;
    let fixed = ((lin << 2) + (quad << (16 - nz))).wrapping_sub(if up { 100u64 << 16 } else { 0 });
    let mut lead = bw64(fixed) - 1;
    let sh = lead - 23;
    let mut m = (fixed + ((1u64 << sh) - 1)) >> sh;
    if m == 1 << 24 {
        m >>= 1;
        lead += 1;
    }
    0x8000_0000 | ((lead + 127 - 50) << 23) | (m as u32 & 0x7f_ffff)
}

fn log_above_one(bits: u32) -> u32 {
    let i = bits - 0x3f80_0000;
    let row = if i < 0x1_0000 { 0 } else if i < 0x2_0000 { 1 } else { 2 };
    let cs: [u64; 3] = [0, 192, 2856];
    let ls: [u64; 3] = [12102203, 12102186, 12102072];
    let qs: [u64; 3] = [12102094, 12097614, 12084310];
    let cb: [u64; 3] = [1999052, 1948887, 1883467];
    let w = bw32(i);
    let nz = if row != 0 { 0 } else { 4 * ((18 - w) / 4) };
    let i = i as u64;
    let n = i << nz;
    let lin = ((ls[row] * n) >> 13) << 2;
    let sq = (n * i) >> 12;
    let inner = re((qs[row] << 22).wrapping_sub(cb[row] * (2 * i + 1)), 22);
    let pr = inner * sq;
    let quad = if pr >= 1 << 47 { re(pr, 24) * 2 } else { re(pr, 23) };
    let fixed = (cs[row] + lin).wrapping_sub(quad);
    let mut lead = bw64(fixed) - 1;
    let sh = lead - 23;
    let mut m = (fixed >> sh) + 1;
    if m == 1 << 24 {
        m >>= 1;
        lead += 1;
    }
    ((lead + 127 - 35 - nz) << 23) | (m as u32 & 0x7f_ffff)
}

fn log_ordinary(bits: u32) -> u32 {
    let fr = (bits & 0x3_ffff) as u64;
    let mut idx = ((bits >> 18) & 31) as usize;
    if idx == 1 && fr >= 0x2_0000 {
        idx = 32;
    }
    let c = LOG[idx];
    let lin = (c[1] as u64 * fr) >> 16;
    let inner = re(((c[2] as u64) << 22).wrapping_sub(c[3] as u64 * (2 * fr + 1)), 22);
    let sq = (fr * fr) >> 12;
    let pr = inner * sq;
    let quad = if pr >= 1 << 47 { re(pr, 24) * 2 } else { re(pr, 23) };
    let e = (bits >> 23) as i64 - 127;
    let fixed = e * (1i64 << 35) + (((c[0] as u64 + lin) << 5).wrapping_sub(quad) as i64);
    let neg = fixed < 0;
    let mag = fixed.unsigned_abs();
    let mut lead = 63 - mag.leading_zeros();
    let sh = lead - 23;
    let mut m = (mag + if neg { (1u64 << sh) - 1 } else { 0 }) >> sh;
    if m == 1 << 24 {
        m >>= 1;
        lead += 1;
    }
    (if neg { 0x8000_0000 } else { 0 }) | ((lead + 127 - 35) << 23) | (m as u32 & 0x7f_ffff)
}

pub fn log(bits: u32) -> u32 {
    let mag = bits & 0x7fff_ffff;
    if mag > 0x7f80_0000 {
        return bits;
    }
    if mag < 0x80_0000 {
        return 0xff80_0000;
    }
    if bits & 0x8000_0000 != 0 {
        return 0xffc0_0000;
    }
    if mag == 0x7f80_0000 {
        return 0x7f80_0000;
    }
    if bits == 0x3f80_0000 {
        return 0;
    }
    if (0x3f7c_0000..0x3f80_0000).contains(&bits) {
        return log_below_one(bits);
    }
    if bits > 0x3f80_0000 && bits < 0x3f84_0000 {
        return log_above_one(bits);
    }
    log_ordinary(bits)
}

const TRIG: [[[i32; 4]; 16]; 2] = [
    [
        [0, 3294200, 0, -1322], [3292874, 3290232, -3968, -1320], [6577818, 3278340, -7928, -1312], [9846914, 3258548, -11864, -1304],
        [13092288, 3230908, -15778, -1290], [16306122, 3195480, -19650, -1274], [19480674, 3152356, -23476, -1256], [22608294, 3101636, -27246, -1234],
        [25681450, 3043444, -30948, -1210], [28692736, 2977924, -34576, -1182], [31634898, 2905224, -38122, -1152], [34500850, 2825532, -41576, -1118],
        [37283686, 2739032, -44928, -1082], [39976702, 2645928, -48176, -1042], [42573412, 2546452, -51304, -1002], [45067558, 2440844, -54310, -958],
    ],
    [
        [67108864, 0, -80872, 32], [67028028, -161636, -80774, 98], [66785716, -322884, -80482, 162], [66382510, -483352, -79996, 226],
        [65819384, -642664, -79318, 290], [65097694, -800416, -78448, 352], [64219178, -956248, -77390, 414], [63185952, -1109776, -76144, 476],
        [62000504, -1260628, -74716, 536], [60665694, -1408448, -73104, 594], [59184734, -1552872, -71320, 652], [57561192, -1693552, -69364, 708],
        [55798980, -1830152, -67242, 762], [53902344, -1962348, -64956, 814], [51875852, -2089812, -62514, 864], [49724388, -2212244, -59922, 912],
    ],
];

fn fixed_to_float(v: u64, frac: u32) -> u32 {
    if v == 0 {
        return 0;
    }
    let lead = bw64(v) - 1;
    let m = (if lead >= 23 { v >> (lead - 23) } else { v << (23 - lead) }) as u32;
    let e = lead as i32 - frac as i32 + 127;
    ((e as u32) << 23) | (m & 0x7f_ffff)
}

fn linear_fraction(fr: u32, sh: u32, refl: u32) -> u32 {
    let v = fr.wrapping_sub(refl);
    let block = v >> (20 + sh);
    let rounded = v.wrapping_add(1 << (sh - 1)) >> sh;
    let limit = ((block + 1) << 20) - 1;
    rounded.min(limit) << sh
}

fn small_sine(bits: u32) -> u32 {
    let mag = bits & 0x7fff_ffff;
    let eb = mag >> 23;
    let m = ((mag & 0x7f_ffff) | if eb != 0 { 0x80_0000 } else { 0 }) as u64;
    let e = if eb != 0 { eb as i32 - 127 } else { -126 };
    let p = m * 0xc9_0fd5;
    if p == 0 {
        return bits & 0x8000_0000;
    }
    let lead = bw64(p) - 1;
    let mut re_ = e - 44 + lead as i32;
    let r = if re_ < -126 {
        re(p, 21) as u32
    } else {
        let mut rd = re(p, lead - 23) as u32;
        if rd == 0x100_0000 {
            rd >>= 1;
            re_ += 1;
        }
        (((re_ + 127) as u32) << 23) | (rd & 0x7f_ffff)
    };
    r | (bits & 0x8000_0000)
}

pub fn trig(bits: u32, cos: bool) -> u32 {
    let mag = bits & 0x7fff_ffff;
    if mag >= 0x7f80_0000 {
        return if mag == 0x7f80_0000 { 0xffc0_0000 } else { bits };
    }
    if mag == 0 {
        return if cos { 0x3f80_0000 } else { bits & 0x8000_0000 };
    }
    if !cos && mag < 0x39c0_0000 {
        return small_sine(bits);
    }
    let e = (mag >> 23) as i32 - 127;
    if e >= 23 {
        return if cos { 0x3f80_0000 } else { 0 };
    }
    let m = ((mag & 0x7f_ffff) | 0x80_0000) as u64;
    let sh = e + 8;
    let phase = if sh >= 0 { ((m << sh) & 0x7fff_ffff) as u32 } else if -sh >= 64 { 0 } else { (m >> -sh) as u32 };
    let quadrant = phase >> 29;
    let qp = phase & 0x1fff_ffff;
    let refl = (qp >= 0x1000_0000) as u32;
    let reduced = if refl != 0 { 0x2000_0000 - qp } else { qp };
    let table = (cos as u32) ^ (quadrant & 1) ^ refl;
    let index = reduced.wrapping_sub(refl) >> 24;
    let fr = reduced - (index << 24);
    let c = TRIG[table as usize][index as usize];
    let result = if table != 0 && reduced.wrapping_sub(refl) < 46592 {
        0x3f80_0000
    } else if table == 0 && reduced == 0 {
        0
    } else {
        let sq_in = (fr.wrapping_sub(refl) >> 7) as u64;
        let mut ic = if c[3] <= 0 { fr.wrapping_sub(refl) } else { (1u32 << 24).wrapping_sub(fr).wrapping_add(refl).wrapping_sub(1) };
        ic &= !63;
        let iv = if c[3] <= 0 {
            (-(c[2] as i64) * (1i64 << 24) - c[3] as i64 * ic as i64) as u64
        } else {
            (-((c[2] + c[3]) as i64) * (1i64 << 24) + c[3] as i64 * ic as i64) as u64
        };
        let si = re(iv, 17);
        if table == 0 && index < 2 {
            let w = bw32(fr);
            let nz = if index != 0 { 0 } else { 4 * if w < 24 { (24 - w) / 4 } else { 0 } };
            let os = if index != 0 { 0 } else { 4 * if w < 25 { (25 - w) / 4 } else { 0 } };
            let mut li = if index != 0 { linear_fraction(fr, 2, refl) as u64 } else { (fr as u64) << os };
            if index == 0 && refl != 0 && ((fr as u64) << nz) & 0xf_ffff == 0 {
                li = li.wrapping_sub(1);
            }
            let lprec = if index != 0 { 30 } else { 32 + nz };
            let qprec = 34 + nz;
            let lin = ((c[1] as u32 as u64).wrapping_mul(li) >> (50 + os - lprec)) as i64;
            let sprec = 34u32.min(24 + nz);
            let ssq = (sq_in * sq_in) >> (34 - sprec);
            let quad = -(re(si * ssq, 33 + sprec - qprec) as i64);
            let sum = ((c[0] as i64) << (qprec - 26)) + (lin << (qprec - lprec)) + quad;
            fixed_to_float(sum as u64, qprec)
        } else {
            let li = linear_fraction(fr, 4, refl);
            let p = c[1] as i64 * li as i64;
            let a = p.unsigned_abs();
            let prec = if a >= 1 << 45 { 28 } else { 29 };
            let lin = re(a, 50 - prec) as i64 * if p < 0 { -1 } else { 1 } * if prec == 28 { 2 } else { 1 };
            let ssq = (sq_in * sq_in) >> 10;
            let qp2 = si * ssq;
            let bias = 1u64 << if qp2 >= 1 << 47 { 23 } else { 22 };
            let quad = if qp2 <= bias { 0 } else { -(((qp2 - bias + (1 << 29) - 1) >> 29) as i64) };
            let sum = c[0] as i64 * 8 + lin + quad * 2;
            fixed_to_float(sum as u64, 29)
        }
    };
    if result == 0 {
        return 0;
    }
    let neg = (quadrant >> 1) ^ ((cos as u32) & quadrant & 1) ^ ((!cos && bits >> 31 != 0) as u32);
    result | (neg << 31)
}

const HLSL_BODY: &str = r#"uint64_t tb_a_re(uint64_t v, uint s) {
    if (s == 0u) return v;
    uint64_t w = v >> s, r = v & ((((uint64_t)1) << s) - 1), h = ((uint64_t)1) << (s - 1u);
    return w + ((r > h || (r == h && (w & 1) != 0)) ? (uint64_t)1 : (uint64_t)0);
}
uint tb_a_bw32(uint v) { return v == 0u ? 0u : firstbithigh(v) + 1u; }
uint tb_a_bw64(uint64_t v) { uint hi = (uint)(v >> 32); return hi != 0u ? firstbithigh(hi) + 33u : tb_a_bw32((uint)v); }
uint tb_a_exp(uint x) {
    uint mag = x & 0x7fffffffu;
    if (mag >= 0x7f800000u) { if (mag > 0x7f800000u) return x; return (x >> 31) != 0u ? 0u : 0x7f800000u; }
    if (mag < 0x33800000u) return 0x3f800000u;
    bool neg = (x >> 31) != 0u;
    if (!neg && mag >= 0x43000000u) return 0x7f800000u;
    if (neg && mag > 0x42fc0000u) return 0u;
    int e = (int)(mag >> 23) - 127;
    uint64_t m = (uint64_t)((mag & 0x7fffffu) | 0x800000u);
    int sh = e + 6;
    uint64_t ab = sh >= 0 ? (m << (uint)sh) : (m >> (uint)(-sh));
    int64_t ph = neg ? -(int64_t)ab - 1 : (int64_t)ab;
    uint64_t fr = (uint64_t)((uint)ph & 0xffffffu);
    uint ci = (uint)(((uint64_t)ph >> 24) & 31) * 4u;
    uint64_t lp = (uint64_t)TB_A_EXP[ci + 1u] * fr;
    uint64_t lin = lp >= (((uint64_t)1) << 47) ? tb_a_re(lp, 24u) : (tb_a_re(lp, 23u) >> 1);
    uint64_t co = fr >> 6;
    uint64_t sq = (co * co) >> 12;
    uint64_t inner = tb_a_re((((uint64_t)TB_A_EXP[ci + 2u]) << 7) + (uint64_t)TB_A_EXP[ci + 3u] * co, 7u);
    uint64_t pr = inner * sq;
    uint64_t quad = pr >= (((uint64_t)1) << 47) ? tb_a_re(pr, 24u) * 2 : tb_a_re(pr, 23u);
    uint64_t sum = (((uint64_t)TB_A_EXP[ci] + lin) << 8) + quad;
    return (uint)(((ph >> 29) + 126) * 0x800000 + (int64_t)tb_a_re(sum, 13u));
}
uint tb_a_log_lo(uint bits) {
    uint k = 0x3f800000u - bits;
    bool up = k >= 0x20000u;
    uint64_t c1 = up ? 12102221 : 12102203;
    uint64_t c2 = up ? 12097323 : 12101979;
    uint64_t c3 = up ? 2088428 : 2035248;
    uint w = tb_a_bw32(k);
    uint nz = w < 18u ? 4u * ((18u - w) / 4u) : 0u;
    uint ls = nz < 14u ? 14u - nz : 0u;
    uint ss = nz < 12u ? 12u - nz : 0u;
    uint64_t kk = (uint64_t)k;
    uint64_t lin = ((2 * c1 * kk) >> ls) << ls;
    uint64_t sq = ((kk * kk) >> ss) << ss;
    uint64_t inner = tb_a_re((c2 << 22) + c3 * (kk - 1), 22u);
    uint ps = 38u - nz;
    uint64_t quad = (inner * sq + ((((uint64_t)1) << ps) - 1)) >> ps;
    uint64_t fixed = (lin << 2) + (quad << (16u - nz)) - (up ? (((uint64_t)100) << 16) : (uint64_t)0);
    uint lead = tb_a_bw64(fixed) - 1u;
    uint sh = lead - 23u;
    uint64_t m = (fixed + ((((uint64_t)1) << sh) - 1)) >> sh;
    if (m == (((uint64_t)1) << 24)) { m >>= 1; lead += 1u; }
    return 0x80000000u | ((lead + 127u - 50u) << 23) | ((uint)m & 0x7fffffu);
}
uint tb_a_log_hi(uint bits) {
    uint i = bits - 0x3f800000u;
    uint row = i < 0x10000u ? 0u : (i < 0x20000u ? 1u : 2u);
    uint64_t cs = row == 0u ? 0 : (row == 1u ? 192 : 2856);
    uint64_t ls = row == 0u ? 12102203 : (row == 1u ? 12102186 : 12102072);
    uint64_t qs = row == 0u ? 12102094 : (row == 1u ? 12097614 : 12084310);
    uint64_t cb = row == 0u ? 1999052 : (row == 1u ? 1948887 : 1883467);
    uint w = tb_a_bw32(i);
    uint nz = row != 0u ? 0u : 4u * ((18u - w) / 4u);
    uint64_t ii = (uint64_t)i;
    uint64_t n = ii << nz;
    uint64_t lin = ((ls * n) >> 13) << 2;
    uint64_t sq = (n * ii) >> 12;
    uint64_t inner = tb_a_re((qs << 22) - cb * (2 * ii + 1), 22u);
    uint64_t pr = inner * sq;
    uint64_t quad = pr >= (((uint64_t)1) << 47) ? tb_a_re(pr, 24u) * 2 : tb_a_re(pr, 23u);
    uint64_t fixed = cs + lin - quad;
    uint lead = tb_a_bw64(fixed) - 1u;
    uint sh = lead - 23u;
    uint64_t m = (fixed >> sh) + 1;
    if (m == (((uint64_t)1) << 24)) { m >>= 1; lead += 1u; }
    return ((lead + 127u - 35u - nz) << 23) | ((uint)m & 0x7fffffu);
}
uint tb_a_log_ord(uint bits) {
    uint64_t fr = (uint64_t)(bits & 0x3ffffu);
    uint idx = (bits >> 18) & 31u;
    if (idx == 1u && fr >= 0x20000) idx = 32u;
    uint ci = idx * 4u;
    uint64_t lin = ((uint64_t)TB_A_LOG[ci + 1u] * fr) >> 16;
    uint64_t inner = tb_a_re((((uint64_t)TB_A_LOG[ci + 2u]) << 22) - (uint64_t)TB_A_LOG[ci + 3u] * (2 * fr + 1), 22u);
    uint64_t sq = (fr * fr) >> 12;
    uint64_t pr = inner * sq;
    uint64_t quad = pr >= (((uint64_t)1) << 47) ? tb_a_re(pr, 24u) * 2 : tb_a_re(pr, 23u);
    int64_t e = (int64_t)(bits >> 23) - 127;
    int64_t fixed = e * (((int64_t)1) << 35) + (int64_t)((((uint64_t)TB_A_LOG[ci] + lin) << 5) - quad);
    bool neg = fixed < 0;
    uint64_t mag = (uint64_t)(neg ? -fixed : fixed);
    uint lead = tb_a_bw64(mag) - 1u;
    uint sh = lead - 23u;
    uint64_t m = (mag + (neg ? ((((uint64_t)1) << sh) - 1) : (uint64_t)0)) >> sh;
    if (m == (((uint64_t)1) << 24)) { m >>= 1; lead += 1u; }
    return (neg ? 0x80000000u : 0u) | ((lead + 127u - 35u) << 23) | ((uint)m & 0x7fffffu);
}
uint tb_a_log(uint bits) {
    uint mag = bits & 0x7fffffffu;
    if (mag > 0x7f800000u) return bits;
    if (mag < 0x800000u) return 0xff800000u;
    if ((bits & 0x80000000u) != 0u) return 0xffc00000u;
    if (mag == 0x7f800000u) return 0x7f800000u;
    if (bits == 0x3f800000u) return 0u;
    if (bits >= 0x3f7c0000u && bits < 0x3f800000u) return tb_a_log_lo(bits);
    if (bits > 0x3f800000u && bits < 0x3f840000u) return tb_a_log_hi(bits);
    return tb_a_log_ord(bits);
}
uint tb_a_f2f(uint64_t v, uint frac) {
    if (v == 0) return 0u;
    uint lead = tb_a_bw64(v) - 1u;
    uint m = (uint)(lead >= 23u ? (v >> (lead - 23u)) : (v << (23u - lead)));
    int e = (int)lead - (int)frac + 127;
    return ((uint)e << 23) | (m & 0x7fffffu);
}
uint tb_a_linfr(uint fr, uint sh, uint refl) {
    uint v = fr - refl;
    uint block = v >> (20u + sh);
    uint rounded = (v + (1u << (sh - 1u))) >> sh;
    uint limit = ((block + 1u) << 20) - 1u;
    return min(rounded, limit) << sh;
}
uint tb_a_ssin(uint bits) {
    uint mag = bits & 0x7fffffffu;
    uint eb = mag >> 23;
    uint64_t m = (uint64_t)((mag & 0x7fffffu) | (eb != 0u ? 0x800000u : 0u));
    int e = eb != 0u ? (int)eb - 127 : -126;
    uint64_t p = m * 0xc90fd5;
    if (p == 0) return bits & 0x80000000u;
    uint lead = tb_a_bw64(p) - 1u;
    int rex = e - 44 + (int)lead;
    uint r;
    if (rex < -126) {
        r = (uint)tb_a_re(p, 21u);
    } else {
        uint rd = (uint)tb_a_re(p, lead - 23u);
        if (rd == 0x1000000u) { rd >>= 1; rex += 1; }
        r = ((uint)(rex + 127) << 23) | (rd & 0x7fffffu);
    }
    return r | (bits & 0x80000000u);
}
uint tb_a_trig(uint bits, bool cs) {
    uint mag = bits & 0x7fffffffu;
    if (mag >= 0x7f800000u) return mag == 0x7f800000u ? 0xffc00000u : bits;
    if (mag == 0u) return cs ? 0x3f800000u : (bits & 0x80000000u);
    if (!cs && mag < 0x39c00000u) return tb_a_ssin(bits);
    int e = (int)(mag >> 23) - 127;
    if (e >= 23) return cs ? 0x3f800000u : 0u;
    uint64_t m = (uint64_t)((mag & 0x7fffffu) | 0x800000u);
    int sh = e + 8;
    uint phase = sh >= 0 ? (uint)((m << (uint)sh) & 0x7fffffff) : (-sh >= 64 ? 0u : (uint)(m >> (uint)(-sh)));
    uint quadrant = phase >> 29;
    uint qp = phase & 0x1fffffffu;
    uint refl = qp >= 0x10000000u ? 1u : 0u;
    uint reduced = refl != 0u ? 0x20000000u - qp : qp;
    uint table = (cs ? 1u : 0u) ^ (quadrant & 1u) ^ refl;
    uint index = (reduced - refl) >> 24;
    uint fr = reduced - (index << 24);
    uint ci = table * 64u + index * 4u;
    int c0 = TB_A_TRIG[ci], c1 = TB_A_TRIG[ci + 1u], c2 = TB_A_TRIG[ci + 2u], c3 = TB_A_TRIG[ci + 3u];
    uint result;
    if (table != 0u && reduced - refl < 46592u) {
        result = 0x3f800000u;
    } else if (table == 0u && reduced == 0u) {
        result = 0u;
    } else {
        uint64_t sqin = (uint64_t)((fr - refl) >> 7);
        uint ic = c3 <= 0 ? fr - refl : (1u << 24) - fr + refl - 1u;
        ic &= ~63u;
        uint64_t iv = c3 <= 0 ? (uint64_t)(-(int64_t)c2 * (((int64_t)1) << 24) - (int64_t)c3 * (int64_t)ic) : (uint64_t)(-(int64_t)(c2 + c3) * (((int64_t)1) << 24) + (int64_t)c3 * (int64_t)ic);
        uint64_t si = tb_a_re(iv, 17u);
        if (table == 0u && index < 2u) {
            uint w = tb_a_bw32(fr);
            uint nz = index != 0u ? 0u : 4u * (w < 24u ? (24u - w) / 4u : 0u);
            uint os = index != 0u ? 0u : 4u * (w < 25u ? (25u - w) / 4u : 0u);
            uint64_t li = index != 0u ? (uint64_t)tb_a_linfr(fr, 2u, refl) : (((uint64_t)fr) << os);
            if (index == 0u && refl != 0u && ((((uint64_t)fr) << nz) & 0xfffff) == 0) li -= 1;
            uint lprec = index != 0u ? 30u : 32u + nz;
            uint qprec = 34u + nz;
            int64_t lin = (int64_t)(((uint64_t)(uint)c1 * li) >> (50u + os - lprec));
            uint sprec = min(34u, 24u + nz);
            uint64_t ssq = (sqin * sqin) >> (34u - sprec);
            int64_t quad = -(int64_t)tb_a_re(si * ssq, 33u + sprec - qprec);
            int64_t sum = (((int64_t)c0) << (qprec - 26u)) + (lin << (qprec - lprec)) + quad;
            result = tb_a_f2f((uint64_t)sum, qprec);
        } else {
            uint li = tb_a_linfr(fr, 4u, refl);
            int64_t p = (int64_t)c1 * (int64_t)li;
            uint64_t a = (uint64_t)(p < 0 ? -p : p);
            uint prec = a >= (((uint64_t)1) << 45) ? 28u : 29u;
            int64_t lin = (int64_t)tb_a_re(a, 50u - prec) * (p < 0 ? -1 : 1) * (prec == 28u ? 2 : 1);
            uint64_t ssq = (sqin * sqin) >> 10;
            uint64_t q2 = si * ssq;
            uint64_t bias = ((uint64_t)1) << (q2 >= (((uint64_t)1) << 47) ? 23u : 22u);
            int64_t quad = q2 <= bias ? 0 : -(int64_t)((q2 - bias + (((uint64_t)1) << 29) - 1) >> 29);
            int64_t sum = (int64_t)c0 * 8 + lin + quad * 2;
            result = tb_a_f2f((uint64_t)sum, 29u);
        }
    }
    if (result == 0u) return 0u;
    uint neg = (quadrant >> 1) ^ ((cs ? 1u : 0u) & quadrant & 1u) ^ ((!cs && (bits >> 31) != 0u) ? 1u : 0u);
    return result | (neg << 31);
}
uint tb_t32_amd(uint op, uint x) {
    if (op == 4u) return tb_a_log(x);
    if (op == 5u) return tb_a_exp(x);
    return tb_a_trig(x, op == 7u);
}
"#;

pub fn hlsl() -> String {
    let join = |v: Vec<String>| v.join(", ");
    let exp: Vec<String> = EXP.iter().flatten().map(|v| format!("{v}u")).collect();
    let log: Vec<String> = LOG.iter().flatten().map(|v| format!("{v}u")).collect();
    let trig: Vec<String> = TRIG.iter().flatten().flatten().map(|v| format!("{v}")).collect();
    let mut h = format!("static const uint TB_A_EXP[{}] = {{ {} }};\n", exp.len(), join(exp));
    h += &format!("static const uint TB_A_LOG[{}] = {{ {} }};\n", log.len(), join(log));
    h += &format!("static const int TB_A_TRIG[{}] = {{ {} }};\n", trig.len(), join(trig));
    h + HLSL_BODY
}
