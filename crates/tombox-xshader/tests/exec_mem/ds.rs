use super::*;

pub fn ds(op: u32, offset: u32, addr: u32, d0: u32, d1: u32, vdst: u32) -> [u32; 2] {
    [0xd800_0000 | (op << 18) | (offset & 0xffff), addr | (d0 << 8) | (d1 << 16) | (vdst << 24)]
}

fn lane_value(l: u32) -> u32 {
    (l * l * 0x1111 + 7) ^ 0xdead_0000
}

fn lane_prog(p: &mut Prog) {
    p.raw(&[vop1(0x07, 10, v(2)), vop1(0x07, 11, v(3))]);
    p.raw(&[vop2(0x1a, 12, ic(1), 11), vop2(0x25, 12, v(12), 10)]);
    p.raw(&[vop2(0x0b, 13, v(12), 12), vop2(0x0b, 13, LIT, 13), 0x1111]);
    p.raw(&[vop2(0x25, 13, ic(7), 13), vop2(0x1d, 13, LIT, 13), 0xdead_0000]);
}

#[test]
fn ds_swizzle_quad() {
    let mut cs = Vec::new();
    for &m in &[0xe4u32, 0x1b, 0x55, 0xaa, 0xff, 0x00, 0x4e, 0xb1, 0x93, 0x39] {
        for k in 0..4u32 {
            let op = ds(0x35, 0x8000 | m, 13, 0, 0, 14);
            let mut p = Prog::new();
            lane_prog(&mut p);
            p.raw(&op).raw(&ds(0x35, 0x8000 | k, 14, 0, 0, 0));
            let want = lane_value((m >> (2 * k)) & 3);
            cs.push(MCase { m: "ds_swizzle_b32", op: op.to_vec(), words: p.words(), env: Env { pos: true, ..Default::default() }, rt: vec![want], uav: vec![], tol: 0.0 });
        }
    }
    run_mem_cases("ds_swizzle", &cs);
}

const T: u32 = 64;

pub fn dv(t: u32) -> u32 {
    (t * 0x2f5b3) ^ 0x8080_8080
}

pub fn ev(t: u32) -> u32 {
    dv(t) ^ 0x0f0f_1234
}

fn store(vdata: u32, vaddr: u32, k: u32) -> [u32; 2] {
    [0xe000_0000 | (0x1c << 18) | (1 << 12) | (k * 256), vaddr | (vdata << 8) | (2 << 16) | (0x80 << 24)]
}

pub fn cs_prog(body: &[u32], outs: &[u32]) -> Vec<u32> {
    let mut p = Prog::new();
    p.raw(&smem(2, 8, 4, 0, SNULL));
    p.raw(&[vop2(0x1a, 1, ic(2), 0)]);
    p.raw(&[vop2(0x0b, 2, LIT, 0), 0x2f5b3, vop2(0x1d, 2, LIT, 2), 0x8080_8080]);
    p.raw(&[vop2(0x1d, 4, LIT, 2), 0x0f0f_1234]);
    p.raw(&[vop2(0x1d, 3, ic(1), 0), vop2(0x1a, 3, ic(2), 3)]);
    p.raw(&[vop2(0x1a, 5, ic(3), 0), vop2(0x1d, 6, ic(1), 0), vop2(0x1a, 6, ic(3), 6)]);
    p.raw(&[vop2(0x1d, 7, ic(1), 0), vop2(0x1a, 8, ic(1), 0), vop2(0x1a, 9, ic(1), 7)]);
    p.raw(body);
    for (k, &r) in outs.iter().enumerate() {
        p.raw(&store(r, 1, k as u32));
    }
    let mut w = p.w;
    w.push(S_ENDPGM);
    w
}

fn outs_of(f: &dyn Fn(u32) -> Vec<u32>) -> Vec<(usize, u32)> {
    let mut m = Vec::new();
    for t in 0..T {
        for (k, v) in f(t).into_iter().enumerate() {
            m.push((k * 64 + t as usize, v));
        }
    }
    m
}

fn cc(m: &'static str, op: [u32; 2], body: Vec<u32>, outs: &[u32], f: &dyn Fn(u32) -> Vec<u32>) -> CsCase {
    CsCase { m, op: op.to_vec(), words: cs_prog(&body, outs), threads: T, mem: outs_of(f), init: vec![] }
}

#[test]
fn ds_lds_compute() {
    let mut cs = Vec::new();
    let nb = |t: u32| t ^ 1;
    for &off in &[0u32, 0x100, 0x1004] {
        let w = ds(0x0d, off, 1, 2, 0, 0);
        let r = ds(0x36, off, 3, 0, 0, 10);
        let mut b = w.to_vec();
        b.extend_from_slice(&r);
        cs.push(cc("ds_write_b32", w, b.clone(), &[10], &|t| vec![dv(nb(t))]));
        cs.push(cc("ds_read_b32", r, b, &[10], &|t| vec![dv(nb(t))]));
    }
    for &(o0, o1) in &[(0u32, 1u32), (1, 0), (2, 3)] {
        let w = ds(0x0e, o0 | (o1 << 8), 5, 2, 4, 0);
        let r = ds(0x37, o1 | (o0 << 8), 6, 0, 0, 10);
        let mut b = w.to_vec();
        b.extend_from_slice(&r);
        cs.push(cc("ds_write2_b32", w, b.clone(), &[10, 11], &|t| vec![ev(nb(t)), dv(nb(t))]));
        cs.push(cc("ds_read2_b32", r, b, &[10, 11], &|t| vec![ev(nb(t)), dv(nb(t))]));
        let w = ds(0x0f, o0 | (o1 << 8), 1, 2, 4, 0);
        let r = ds(0x38, o1 | (o0 << 8), 3, 0, 0, 10);
        let mut b = w.to_vec();
        b.extend_from_slice(&r);
        if o0 != o1 {
            cs.push(cc("ds_write2st64_b32", w, b.clone(), &[10, 11], &|t| vec![ev(nb(t)), dv(nb(t))]));
            cs.push(cc("ds_read2st64_b32", r, b, &[10, 11], &|t| vec![ev(nb(t)), dv(nb(t))]));
        }
    }
    for &off in &[0u32, 0x41] {
        let w = ds(0x1e, off, 0, 2, 0, 0);
        for (m, op, f) in [("ds_read_u8", 0x3au32, (|x: u32| x & 0xff) as fn(u32) -> u32), ("ds_read_i8", 0x39, |x: u32| (x as u8 as i8) as i32 as u32)] {
            let r = ds(op, off, 7, 0, 0, 10);
            let mut b = w.to_vec();
            b.extend_from_slice(&r);
            cs.push(cc("ds_write_b8", w, b.clone(), &[10], &move |t| vec![f(dv(nb(t)))]));
            cs.push(cc(m, r, b, &[10], &move |t| vec![f(dv(nb(t)))]));
        }
        let w = ds(0x1f, off & !1, 8, 2, 0, 0);
        for (m, op, f) in [("ds_read_u16", 0x3cu32, (|x: u32| x & 0xffff) as fn(u32) -> u32), ("ds_read_i16", 0x3b, |x: u32| (x as u16 as i16) as i32 as u32)] {
            let r = ds(op, off & !1, 9, 0, 0, 10);
            let mut b = w.to_vec();
            b.extend_from_slice(&r);
            cs.push(cc("ds_write_b16", w, b.clone(), &[10], &move |t| vec![f(dv(nb(t)))]));
            cs.push(cc(m, r, b, &[10], &move |t| vec![f(dv(nb(t)))]));
        }
        type Ext = fn(u32) -> u32;
        let w8 = ds(0x1e, off, 0, 2, 0, 0);
        let w16 = ds(0x1f, off & !1, 8, 2, 0, 0);
        let d16: [(&'static str, u32, bool, Ext); 6] = [
            ("ds_read_u8_d16", 0xa2, false, |x| x & 0xff),
            ("ds_read_u8_d16_hi", 0xa3, true, |x| x & 0xff),
            ("ds_read_i8_d16", 0xa4, false, |x| (x as u8 as i8) as i16 as u16 as u32),
            ("ds_read_i8_d16_hi", 0xa5, true, |x| (x as u8 as i8) as i16 as u16 as u32),
            ("ds_read_u16_d16", 0xa6, false, |x| x & 0xffff),
            ("ds_read_u16_d16_hi", 0xa7, true, |x| x & 0xffff),
        ];
        for (m, op, hi, f) in d16 {
            let wide = op >= 0xa6;
            let (w, raddr, roff) = if wide { (w16, 9, off & !1) } else { (w8, 7, off) };
            let r = ds(op, roff, raddr, 0, 0, 10);
            let mut b = w.to_vec();
            b.push(vop1(1, 10, v(4)));
            b.extend_from_slice(&r);
            cs.push(cc(m, r, b, &[10], &move |t| {
                let (old, x) = (ev(t), f(dv(nb(t))));
                vec![if hi { (old & 0xffff) | (x << 16) } else { (old & 0xffff_0000) | x }]
            }));
        }
        for byte in 0..4u32 {
            let at = (off & !3) + byte;
            let w_init = ds(0x0d, off & !3, 1, 4, 0, 0);
            let w_hi = ds(0xa0, at, 1, 2, 0, 0);
            let r_full = ds(0x36, off & !3, 3, 0, 0, 10);
            let mut b = w_init.to_vec();
            b.extend_from_slice(&w_hi);
            b.extend_from_slice(&r_full);
            let sh = byte * 8;
            cs.push(cc("ds_write_b8_d16_hi", w_hi, b, &[10], &move |t| {
                let n = nb(t);
                vec![(ev(n) & !(0xffu32 << sh)) | (((dv(n) >> 16) & 0xff) << sh)]
            }));
        }
        let base_off = off & !3;
        for half_off in [base_off, base_off + 2] {
            let w_init = ds(0x0d, base_off, 1, 4, 0, 0);
            let w_hi = ds(0xa1, half_off, 1, 2, 0, 0);
            let r_full = ds(0x36, base_off, 3, 0, 0, 10);
            let mut b = w_init.to_vec();
            b.extend_from_slice(&w_hi);
            b.extend_from_slice(&r_full);
            let hi_shift = (half_off & 2) * 8;
            cs.push(cc("ds_write_b16_d16_hi", w_hi, b, &[10], &move |t| {
                let n = nb(t);
                let orig = ev(n);
                let hi = dv(n) >> 16;
                vec![(orig & !(0xffffu32 << hi_shift)) | (hi << hi_shift)]
            }));
        }
    }
    type At = fn(u32, u32) -> u32;
    let atoms: [(&'static str, &'static str, u32, At); 6] = [
        ("ds_add_u32", "ds_add_rtn_u32", 0x00, |a, b| a.wrapping_add(b)),
        ("ds_min_u32", "ds_min_rtn_u32", 0x07, |a, b| a.min(b)),
        ("ds_max_u32", "ds_max_rtn_u32", 0x08, |a, b| a.max(b)),
        ("ds_and_b32", "ds_and_rtn_b32", 0x09, |a, b| a & b),
        ("ds_or_b32", "ds_or_rtn_b32", 0x0a, |a, b| a | b),
        ("ds_xor_b32", "ds_xor_rtn_b32", 0x0b, |a, b| a ^ b),
    ];
    let init = 0x5aa5_0ff0u32;
    for (m, mr, op, f) in atoms {
        let mut b = vec![vop1(1, 12, LIT), init, vop1(1, 13, ic(0))];
        b.extend_from_slice(&ds(0x0d, 0x200, 13, 12, 0, 0));
        let a = ds(op, 0x200, 13, 2, 0, 0);
        b.extend_from_slice(&a);
        b.extend_from_slice(&ds(0x36, 0x200, 13, 0, 0, 10));
        let total = (0..T).fold(init, |acc, t| f(acc, dv(t)));
        cs.push(cc(m, a, b, &[10], &move |_| vec![total]));
        let mut b = ds(0x0d, 0, 1, 4, 0, 0).to_vec();
        let a = ds(op + 0x20, 0, 1, 2, 0, 11);
        b.extend_from_slice(&a);
        b.extend_from_slice(&ds(0x36, 0, 3, 0, 0, 10));
        cs.push(cc(mr, a, b, &[11, 10], &move |t| vec![ev(t), f(ev(nb(t)), dv(nb(t)))]));
    }
    run_cs_cases("ds_lds", &cs);
}

fn reg_val(r: u32, t: u32) -> u32 {
    match r {
        0 => t,
        2 => dv(t),
        4 => ev(t),
        7 => t ^ 1,
        13 => 0,
        _ => unreachable!(),
    }
}

type At3 = fn(u32, u32, u32) -> u32;

#[test]
fn ds_lds_atomics_ext() {
    let mut cs = Vec::new();
    let nb = |t: u32| t ^ 1;
    let general: &[(u32, u32, u32)] = &[(4, 2, 0), (4, 4, 0), (0, 7, 0), (13, 2, 0), (13, 13, 0), (4, 0, 0), (0, 4, 0)];
    let cmp: &[(u32, u32, u32)] = &[(4, 4, 2), (4, 2, 0), (0, 0, 4), (13, 13, 7), (0, 7, 2)];
    let ops: [(Option<&'static str>, &'static str, u32, At3); 8] = [
        (Some("ds_sub_u32"), "ds_sub_rtn_u32", 0x01, |o, a, _| o.wrapping_sub(a)),
        (Some("ds_rsub_u32"), "ds_rsub_rtn_u32", 0x02, |o, a, _| a.wrapping_sub(o)),
        (Some("ds_inc_u32"), "ds_inc_rtn_u32", 0x03, |o, a, _| if o >= a { 0 } else { o + 1 }),
        (Some("ds_dec_u32"), "ds_dec_rtn_u32", 0x04, |o, a, _| if o == 0 || o > a { a } else { o - 1 }),
        (Some("ds_min_i32"), "ds_min_rtn_i32", 0x05, |o, a, _| (o as i32).min(a as i32) as u32),
        (Some("ds_max_i32"), "ds_max_rtn_i32", 0x06, |o, a, _| (o as i32).max(a as i32) as u32),
        (Some("ds_cmpst_b32"), "ds_cmpst_rtn_b32", 0x10, |o, a, b| if o == a { b } else { o }),
        (None, "ds_wrxchg_rtn_b32", 0x0d, |_, a, _| a),
    ];
    for (m, mr, op, f) in ops {
        let inputs = if op == 0x10 { cmp } else { general };
        for &(old, d0, d1) in inputs {
            let want = move |t: u32| f(reg_val(old, t), reg_val(d0, t), reg_val(d1, t));
            for rtn in [false, true] {
                let Some(name) = (if rtn { Some(mr) } else { m }) else { continue };
                let a = ds(op + if rtn { 0x20 } else { 0 }, 0, 1, d0, d1, 11);
                let mut b = vec![vop1(1, 13, ic(0))];
                b.extend_from_slice(&ds(0x0d, 0, 1, old, 0, 0));
                b.extend_from_slice(&a);
                b.extend_from_slice(&ds(0x36, 0, 3, 0, 0, 10));
                if rtn {
                    cs.push(cc(name, a, b, &[11, 10], &move |t| vec![reg_val(old, t), want(nb(t))]));
                } else {
                    cs.push(cc(name, a, b, &[10], &move |t| vec![want(nb(t))]));
                }
            }
        }
    }
    let init = 0x5aa5_0ff0u32;
    type At = fn(u32, u32) -> u32;
    let shared: [(&'static str, u32, u32, At); 5] = [
        ("ds_sub_u32", 0x01, 2, |a, b| a.wrapping_sub(b)),
        ("ds_min_i32", 0x05, 2, |a, b| (a as i32).min(b as i32) as u32),
        ("ds_max_i32", 0x06, 2, |a, b| (a as i32).max(b as i32) as u32),
        ("ds_inc_u32", 0x03, 14, |a, _| a + 1),
        ("ds_dec_u32", 0x04, 14, |a, _| a - 1),
    ];
    for (m, op, data, f) in shared {
        let mut b = vec![vop1(1, 12, LIT), init, vop1(1, 13, ic(0)), vop1(1, 14, LIT), 0xffff_ffff];
        b.extend_from_slice(&ds(0x0d, 0x200, 13, 12, 0, 0));
        let a = ds(op, 0x200, 13, data, 0, 0);
        b.extend_from_slice(&a);
        b.extend_from_slice(&ds(0x36, 0x200, 13, 0, 0, 10));
        let total = (0..T).fold(init, |acc, t| f(acc, dv(t)));
        cs.push(cc(m, a, b, &[10], &move |_| vec![total]));
    }
    run_cs_cases("ds_lds_ext", &cs);
}

#[test]
fn ds_lds_mskor_wrxchg2() {
    let mut cs = Vec::new();
    let nb = |t: u32| t ^ 1;
    let grid: &[(u32, u32, u32)] = &[(4, 2, 0), (2, 4, 7), (0, 13, 4), (4, 4, 4), (13, 13, 2), (2, 2, 13), (4, 0, 2), (2, 7, 4)];
    for (k, &(old, d0, d1)) in grid.iter().enumerate() {
        let off = if k % 2 == 0 { 0 } else { 0x104 };
        let want = move |t: u32| (reg_val(old, t) & !reg_val(d0, t)) | reg_val(d1, t);
        for rtn in [false, true] {
            let a = ds(if rtn { 0x2c } else { 0x0c }, off, 1, d0, d1, 11);
            let mut b = vec![vop1(1, 13, ic(0)), vop1(1, 11, v(4))];
            b.extend_from_slice(&ds(0x0d, off, 1, old, 0, 0));
            b.extend_from_slice(&a);
            b.extend_from_slice(&ds(0x36, off, 3, 0, 0, 10));
            if rtn {
                cs.push(cc("ds_mskor_rtn_b32", a, b, &[11, 10], &move |t| vec![reg_val(old, t), want(nb(t))]));
            } else {
                cs.push(cc("ds_mskor_b32", a, b, &[11, 10], &move |t| vec![ev(t), want(nb(t))]));
            }
        }
    }
    let mut b = vec![vop1(1, 12, LIT), 0x5aa5_0ff0, vop1(1, 13, ic(0)), vop1(1, 15, ic(1))];
    b.extend_from_slice(&[vop2(0x1a, 14, v(0), 15), vop2(0x1b, 16, ic(1), 0), vop2(0x1a, 17, v(0), 16)]);
    b.extend_from_slice(&ds(0x0d, 0x200, 13, 12, 0, 0));
    let a = ds(0x0c, 0x200, 13, 14, 17, 0);
    b.extend_from_slice(&a);
    b.extend_from_slice(&ds(0x36, 0x200, 13, 0, 0, 10));
    cs.push(cc("ds_mskor_b32", a, b, &[10], &|_| vec![0xaaaa_aaaa]));
    for &(o0, o1) in &[(0u32, 1u32), (1, 0), (2, 3)] {
        let offs = o0 | (o1 << 8);
        for (m, wr, xc, rd, own, other) in [("ds_wrxchg2_rtn_b32", 0x0eu32, 0x2eu32, 0x37u32, 5u32, 6u32), ("ds_wrxchg2st64_rtn_b32", 0x0f, 0x2f, 0x38, 1, 3)] {
            let x = ds(xc, offs, own, 4, 0, 10);
            let mut b = ds(wr, offs, own, 2, 4, 0).to_vec();
            b.extend_from_slice(&x);
            b.extend_from_slice(&ds(rd, offs, other, 0, 0, 12));
            cs.push(cc(m, x, b, &[10, 11, 12, 13], &|t| vec![dv(t), ev(t), ev(nb(t)), nb(t)]));
        }
    }
    let x = ds(0x2e, 0x0101, 5, 4, 0, 10);
    let mut b = ds(0x0d, 4, 5, 2, 0, 0).to_vec();
    b.extend_from_slice(&x);
    b.extend_from_slice(&ds(0x36, 4, 6, 0, 0, 12));
    cs.push(cc("ds_wrxchg2_rtn_b32", x, b, &[10, 11, 12], &|t| vec![dv(t), ev(t), nb(t)]));
    let x = ds(0x2e, 0x0100, 14, 15, 0, 14);
    let mut b = vec![vop1(1, 14, v(5)), vop1(1, 15, v(4))];
    b.extend_from_slice(&ds(0x0e, 0x0100, 5, 2, 4, 0));
    b.extend_from_slice(&x);
    b.extend_from_slice(&ds(0x37, 0x0100, 6, 0, 0, 12));
    cs.push(cc("ds_wrxchg2_rtn_b32", x, b, &[14, 15, 12, 13], &|t| vec![dv(t), ev(t), ev(nb(t)), nb(t)]));
    run_cs_cases("ds_lds_mskor_wrxchg2", &cs);
}

#[test]
fn ds_lds_b64_compute() {
    let mut cs = Vec::new();
    let nb = |t: u32| t ^ 1;
    let sh4 = [vop2(0x1a, 14, ic(4), 0), vop2(0x1a, 15, ic(4), 7)];
    for &off in &[0u32, 8, 0x404] {
        let w = ds(0x4d, off, 5, 2, 0, 0);
        for (vd, outs) in [(10u32, [10u32, 11]), (6, [6, 7])] {
            let r = ds(0x76, off, 6, 0, 0, vd);
            let mut b = w.to_vec();
            b.extend_from_slice(&r);
            cs.push(cc("ds_write_b64", w, b.clone(), &outs, &|t| vec![dv(nb(t)), t * 4]));
            cs.push(cc("ds_read_b64", r, b, &outs, &|t| vec![dv(nb(t)), t * 4]));
        }
    }
    for &(o0, o1) in &[(0u32, 1u32), (1, 0)] {
        let w = ds(0x4e, o0 | (o1 << 8), 14, 2, 4, 0);
        let r = ds(0x77, o0 | (o1 << 8), 15, 0, 0, 10);
        let mut b = sh4.to_vec();
        b.extend_from_slice(&w);
        b.extend_from_slice(&r);
        let f = |t: u32| vec![dv(t ^ 1), t * 4, ev(t ^ 1), (t ^ 1) * 8];
        cs.push(cc("ds_write2_b64", w, b.clone(), &[10, 11, 12, 13], &f));
        cs.push(cc("ds_read2_b64", r, b, &[10, 11, 12, 13], &f));
        let w = ds(0x4f, o0 | (o1 << 8), 5, 2, 4, 0);
        let r = ds(0x78, o0 | (o1 << 8), 6, 0, 0, 10);
        let mut b = w.to_vec();
        b.extend_from_slice(&r);
        cs.push(cc("ds_write2st64_b64", w, b.clone(), &[10, 11, 12, 13], &f));
        cs.push(cc("ds_read2st64_b64", r, b, &[10, 11, 12, 13], &f));
    }
    let r = ds(0x77, 0x0100, 15, 0, 0, 14);
    let w = ds(0x4e, 0x0100, 14, 2, 4, 0);
    let mut b = sh4.to_vec();
    b.extend_from_slice(&[vop2(0x1a, 15, ic(4), 7), vop1(1, 16, v(15))]);
    b.extend_from_slice(&w);
    b.extend_from_slice(&[vop1(1, 15, v(16))]);
    b.extend_from_slice(&r);
    cs.push(cc("ds_read2_b64", r, b, &[14, 15, 16, 17], &|t| vec![dv(t ^ 1), t * 4, ev(t ^ 1), (t ^ 1) * 8]));
    run_cs_cases("ds_lds_b64", &cs);
}

fn perm_body(mul: u32, add: u32, shr: u32) -> Vec<u32> {
    vec![
        vop2(0x09, 16, LIT, 0),
        mul,
        vop2(0x25, 16, LIT, 16),
        add,
        vop2(0x16, 16, ic(shr as i32), 16),
        vop2(0x1a, 16, ic(2), 16),
        vop2(0x25, 16, LIT, 16),
        0x8000_0080,
    ]
}

#[test]
fn ds_permute_compute() {
    let mut cs = Vec::new();
    let g = |t: u32| t & !31;
    for &(mul, add, off) in &[(7u32, 3u32, 0u32), (1, 0, 4), (13, 5, 0x7c), (0, 9, 0)] {
        let tgt = move |t: u32, o: u32| (((t * mul + add) * 4 + 0x8000_0080).wrapping_add(o) >> 2) & 31;
        let op = ds(0xb3, off, 16, 2, 0, 10);
        let mut b = perm_body(mul, add, 0);
        b.extend_from_slice(&op);
        cs.push(cc("ds_bpermute_b32", op, b, &[10], &move |t| vec![dv(g(t) + tgt(t, off))]));
        let op = ds(0xb2, off, 16, 2, 0, 10);
        let mut b = perm_body(mul, add, 0);
        b.extend_from_slice(&op);
        cs.push(cc("ds_permute_b32", op, b, &[10], &move |t| {
            let src = (0..32u32).filter(|&k| tgt(g(t) + k, off) == t & 31).max();
            vec![src.map(|k| dv(g(t) + k)).unwrap_or(0)]
        }));
    }
    let op = ds(0xb2, 0, 16, 2, 0, 10);
    let mut b = perm_body(1, 0, 1);
    b.extend_from_slice(&op);
    cs.push(cc("ds_permute_b32", op, b, &[10], &move |t| {
        let src = (0..32u32).filter(|&k| (((g(t) + k) >> 1) & 31) == t & 31).max();
        vec![src.map(|k| dv(g(t) + k)).unwrap_or(0)]
    }));
    run_cs_cases("ds_permute", &cs);
}

#[test]
fn ds_swizzle_compute() {
    let mut cs = Vec::new();
    let g = |t: u32| t & !31;
    let bit = |off: u32, l: u32| (((l & (off & 0x1f)) | ((off >> 5) & 0x1f)) ^ ((off >> 10) & 0x1f)) & 31;
    let quad = |off: u32, l: u32| (l & !3) | ((off >> (2 * (l & 3))) & 3);
    for &off in &[0x001fu32, 0x041f, 0x081f, 0x101f, 0x401f, 0x0010, 0x03e0, 0x00a0, 0x0c1b, 0x7c00, 0x1fa5, 0x7fff] {
        let op = ds(0x35, off, 2, 0, 0, 10);
        cs.push(cc("ds_swizzle_b32", op, op.to_vec(), &[10], &move |t| vec![dv(g(t) + bit(off, t & 31))]));
    }
    for &m in &[0xe4u32, 0x1b, 0x55, 0xaa, 0x4e, 0xb1, 0x00, 0xff] {
        let off = 0x8000 | m;
        let op = ds(0x35, off, 2, 0, 0, 10);
        cs.push(cc("ds_swizzle_b32", op, op.to_vec(), &[10], &move |t| vec![dv(g(t) + quad(off, t & 31))]));
    }
    run_cs_cases("ds_swizzle_cs", &cs);
}

fn rotate_source(off: u32, lane: u32) -> u32 {
    let mask = off & 31;
    let n = (off >> 5) & 31;
    let amount = if off & 0x400 == 0 { n } else { (32 - n) % 32 };
    (lane & mask) | (((lane + amount) & 31) & !mask & 31)
}

fn fft_source(off: u32, lane: u32) -> u32 {
    let mask = off & 31;
    let reversed = (0..5).fold(0, |a, b| a | (((lane >> b) & 1) << (4 - b)));
    (reversed >> mask.count_ones()) | (lane & mask)
}

#[test]
fn ds_swizzle_rotate_and_fft_compute() {
    assert_eq!((0..4).map(|l| fft_source(0xe000, l)).collect::<Vec<_>>(), [0, 16, 8, 24]);
    assert!((0..32).all(|l| fft_source(0xe01f, l) == l));
    assert_eq!([1, 16, 17].map(|l| fft_source(0xe010, l)), [8, 16, 24]);
    assert_eq!((0..4).map(|l| rotate_source(0xc020, l)).collect::<Vec<_>>(), [1, 2, 3, 4]);
    assert_eq!((0..4).map(|l| rotate_source(0xc420, l)).collect::<Vec<_>>(), [31, 0, 1, 2]);
    assert_eq!((0..8).map(|l| rotate_source(0xc060 | 0x18, l)).collect::<Vec<_>>(), [3, 4, 5, 6, 7, 0, 1, 2]);
    let g = |t: u32| t & !31;
    let mut cs = Vec::new();
    let mut offsets = Vec::new();
    for dir in [0u32, 1] {
        for n in [0u32, 1, 3, 31] {
            for mask in [0x00u32, 0x18, 0x1c, 0x01] {
                offsets.push(0xc000 | (dir << 10) | (n << 5) | mask);
            }
        }
    }
    offsets.extend([0xd800u32 | 0x0460 | 0x18, 0xc000 | (3 << 5) | 0x1f, 0xdbff]);
    for &off in &offsets {
        let op = ds(0x35, off, 2, 0, 0, 10);
        cs.push(cc("ds_swizzle_b32", op, op.to_vec(), &[10], &move |t| vec![dv(g(t) + rotate_source(off, t & 31))]));
    }
    for &mask in &[0x00u32, 0x10, 0x1f, 0x18, 0x1c, 0x1e, 0x01, 0x0f, 0x15, 0x08] {
        let off = 0xe000 | mask;
        let op = ds(0x35, off, 2, 0, 0, 10);
        cs.push(cc("ds_swizzle_b32", op, op.to_vec(), &[10], &move |t| vec![dv(g(t) + fft_source(off, t & 31))]));
    }
    for &off in &[0xffe0u32, 0xf550, 0xfffe, 0xe3f1] {
        let op = ds(0x35, off, 2, 0, 0, 10);
        cs.push(cc("ds_swizzle_b32", op, op.to_vec(), &[10], &move |t| vec![dv(g(t) + fft_source(off, t & 31))]));
    }
    run_cs_cases("ds_swizzle_rotate_fft", &cs);
}

#[test]
fn ds_permute_with_lds_is_refused() {
    let mut b = perm_body(1, 0, 0);
    b.extend_from_slice(&ds(0x0d, 0, 1, 2, 0, 0));
    b.extend_from_slice(&ds(0xb3, 0, 16, 2, 0, 10));
    let w = cs_prog(&b, &[10]);
    let e = match translate(&cs_extract(&w, T)) {
        Ok(_) => panic!("permute with LDS translated"),
        Err(e) => e,
    };
    assert!(e.contains("groupshared"), "{e}");
}
