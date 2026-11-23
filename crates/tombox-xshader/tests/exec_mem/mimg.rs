use super::*;

pub fn mimg(op: u32, dmask: u32, dim: u32, vaddr: u32, vdata: u32, srsrc: u32, ssamp: u32) -> [u32; 2] {
    [0xf000_0000 | ((op & 0x7f) << 18) | (op >> 7) | (dmask << 8) | (dim << 3), vaddr | (vdata << 8) | ((srsrc / 4) << 16) | ((ssamp / 4) << 21)]
}

fn lin(p: &mut Prog, dst: u32, src: u32, k: f32, b: f32) {
    p.raw(&[vop2(0x08, dst, LIT, src), fb(k)]);
    p.raw(&[vop2(0x03, dst, LIT, dst), fb(b)]);
}

fn build(pre: &Prog, op: &[u32], vdata: u32, n: u32, uav: bool) -> Vec<u32> {
    let mut p = Prog::new();
    if uav {
        p.raw(&smem(3, 16, P_UAV, 0, SNULL));
    } else {
        p.raw(&smem(3, 0, P_SRV, 0, SNULL)).raw(&smem(2, 12, P_SAMP, 0, SNULL));
    }
    p.raw(&pre.w).raw(op);
    for k in 0..n {
        p.raw(&[vop1(1, k, v(vdata + k))]);
    }
    p.words()
}

fn ta(m: u32, _l: u32, x: u32, y: u32) -> [u32; 4] {
    let v = (m * 100 + y * 10 + x) as f32;
    [fb(v), fb(v + 1000.0), fb(v + 2000.0), fb(v + 3000.0)]
}

fn ea(m: u32, x: u32, y: u32) -> [u32; 4] {
    ta(m, 0, x, y)
}

fn td(m: u32, _l: u32, x: u32, y: u32) -> [u32; 4] {
    let s = 8 >> m;
    [fb(m as f32 * 0.5 + (y * s + x) as f32 / 64.0), 0, 0, 0]
}

fn tzero(_m: u32, _l: u32, _x: u32, _y: u32) -> [u32; 4] {
    [0; 4]
}

const IMG_A: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 4, fmt: R32G32B32A32_FLOAT, texel: ta };
const IMG_B: Img = Img { w: 16, h: 4, layers: 1, array: false, mips: 3, fmt: R32G32B32A32_FLOAT, texel: ta };
const IMG_C: Img = Img { w: 8, h: 8, layers: 3, array: true, mips: 2, fmt: R32G32B32A32_FLOAT, texel: ta };
const IMG_D: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 1, fmt: R32_FLOAT, texel: td };
const IMG_E: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 2, fmt: R32_FLOAT, texel: td };
const IMG_U: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 4, fmt: R32G32B32A32_FLOAT, texel: tzero };

fn pt(m: u32, u: f32, v: f32, off: (i32, i32)) -> (u32, u32) {
    let s = (8u32 >> m) as f32;
    let c = |t: f32, o: i32| ((t * s).floor() as i32 + o).clamp(0, s as i32 - 1) as u32;
    (c(u, off.0), c(v, off.1))
}

fn nearest(lod: f32, mips: u32) -> u32 {
    let l = lod.max(0.0);
    let m = if l <= 0.5 { 0.0 } else { (l + 0.5).ceil() - 1.0 };
    (m as u32).min(mips - 1)
}

fn packoff(x: i32, y: i32) -> u32 {
    ((x as u32) & 0x3f) | (((y as u32) & 0x3f) << 8)
}

fn pick(t: [u32; 4], dmask: u32) -> Vec<u32> {
    (0..4).filter(|c| (dmask >> c) & 1 != 0).map(|c| t[c]).collect()
}

const PS_K: [(f32, f32, i32); 3] = [(0.125, 0.3, 0), (0.25, 0.2, 1), (0.5, 0.1, 2)];

fn pos_uv(p: &mut Prog, du: u32, dv: u32, k: f32, b: f32) -> (f32, f32) {
    pos_at(p, du, dv, k, 0.5 * k + b, 0.5 * k + b + 0.25)
}

fn pos_at(p: &mut Prog, du: u32, dv: u32, k: f32, u0: f32, v0: f32) -> (f32, f32) {
    lin(p, du, 2, k, u0 - 0.5 * k);
    lin(p, dv, 3, k, v0 - 0.5 * k);
    (0.5 * k + (u0 - 0.5 * k), 0.5 * k + (v0 - 0.5 * k))
}

fn case<'a>(m: &'static str, op: [u32; 2], words: Vec<u32>, env: Env<'a>, rt: Vec<u32>) -> MCase<'a> {
    MCase { m, op: op.to_vec(), words, env, rt, uav: vec![], tol: 0.0 }
}

fn ptsamp() -> Option<[u8; 52]> {
    Some(sampler(0, 3, 0))
}

#[test]
fn image_mimg_sample() {
    let mut cs = Vec::new();
    let (u, v) = (0.3f32, 0.6f32);
    let tex = || Env { tex: Some(&IMG_A), samp: ptsamp(), ..Default::default() };
    for &(cl, dmask) in &[(0.0f32, 0xfu32), (1.2, 0xf), (1.7, 0x5), (2.2, 0x8), (9.0, 0x3)] {
        let op = mimg(0x21, dmask, 1, 1, 4, 0, 12);
        let mut pre = Prog::new();
        pre.vv(1, fb(u)).vv(2, fb(v)).vv(3, fb(cl));
        let m = nearest(cl, 4);
        let (x, y) = pt(m, u, v, (0, 0));
        cs.push(case("image_sample_cl", op, build(&pre, &op, 4, dmask.count_ones(), false), tex(), pick(ea(m, x, y), dmask)));
    }
    for &(k, b, lod) in &PS_K {
        for &cl in &[0.0f32, 1.2, 2.7] {
            let op = mimg(0x21, 0xf, 1, 6, 4, 0, 12);
            let mut pre = Prog::new();
            let (u0, v0) = pos_uv(&mut pre, 6, 7, k, b);
            pre.vv(8, fb(cl));
            let m = nearest((lod as f32).max(cl), 4);
            let (x, y) = pt(m, u0, v0, (0, 0));
            cs.push(case("image_sample_cl", op, build(&pre, &op, 4, 4, false), Env { pos: true, ..tex() }, ea(m, x, y).to_vec()));
        }
    }
    for &(bias, cl) in &[(2.0f32, 1.2f32), (0.3, 2.2), (-3.0, 0.0)] {
        let op = mimg(0x26, 0xf, 1, 1, 4, 0, 12);
        let m = nearest(cl, 4);
        let (x, y) = pt(m, u, v, (0, 0));
        let mut pre = Prog::new();
        pre.vv(1, fb(bias)).vv(2, fb(u)).vv(3, fb(v)).vv(4, fb(cl));
        cs.push(case("image_sample_b_cl", op, build(&pre, &op, 4, 4, false), tex(), ea(m, x, y).to_vec()));
    }
    for &(k, b, lod) in &PS_K {
        for &(bias, cl) in &[(1.0f32, 0.0f32), (1.0, 2.6), (-1.0, 0.0), (0.4, 0.0)] {
            let op = mimg(0x26, 0xf, 1, 6, 4, 0, 12);
            let mut pre = Prog::new();
            pre.vv(6, fb(bias));
            let (u0, v0) = pos_uv(&mut pre, 7, 8, k, b);
            pre.vv(9, fb(cl));
            let m = nearest((lod as f32 + bias).max(cl), 4);
            let (x, y) = pt(m, u0, v0, (0, 0));
            cs.push(case("image_sample_b_cl", op, build(&pre, &op, 4, 4, false), Env { pos: true, ..tex() }, ea(m, x, y).to_vec()));
        }
    }
    for &(g, cl, lod) in &[([0.25f32, 0.0, 0.0, 0.25], 0.0f32, 1.0f32), ([0.25, 0.0, 0.0, 0.25], 1.7, 1.0), ([0.125, 0.0, 0.0, 0.125], 0.0, 0.0), ([0.0, 0.5, 0.0625, 0.0], 0.0, 2.0), ([0.0625, 0.0, 0.0, 1.0], 1.2, 3.0)] {
        let op = mimg(0x23, 0xf, 1, 10, 4, 0, 12);
        let mut pre = Prog::new();
        pre.vv(10, fb(g[0])).vv(11, fb(g[1])).vv(12, fb(g[2])).vv(13, fb(g[3])).vv(14, fb(u)).vv(15, fb(v)).vv(16, fb(cl));
        let m = nearest(lod.max(cl), 4);
        let (x, y) = pt(m, u, v, (0, 0));
        cs.push(case("image_sample_d_cl", op, build(&pre, &op, 4, 4, false), tex(), ea(m, x, y).to_vec()));
    }
    for &(ox, oy) in &[(1i32, -1i32), (-2, 2), (0, 0), (5, 0), (-8, 7)] {
        let op = mimg(0x30, 0xf, 1, 1, 4, 0, 12);
        let mut pre = Prog::new();
        pre.vv(1, packoff(ox, oy)).vv(2, fb(u)).vv(3, fb(v));
        let (x, y) = pt(0, u, v, (ox, oy));
        cs.push(case("image_sample_o", op, build(&pre, &op, 4, 4, false), tex(), ea(0, x, y).to_vec()));
    }
    for &(k, b, lod) in &PS_K {
        let op = mimg(0x30, 0xf, 1, 6, 4, 0, 12);
        let mut pre = Prog::new();
        pre.vv(6, packoff(1, 1));
        let (u0, v0) = pos_uv(&mut pre, 7, 8, k, b);
        let m = lod as u32;
        let (x, y) = pt(m, u0, v0, (1, 1));
        cs.push(case("image_sample_o", op, build(&pre, &op, 4, 4, false), Env { pos: true, ..tex() }, ea(m, x, y).to_vec()));
    }
    for &(lod, ox, oy) in &[(1.0f32, 1i32, 1i32), (2.0, -1, 0), (0.6, 0, -1), (0.0, 2, 3), (3.4, 0, 0)] {
        let op = mimg(0x34, 0xf, 1, 1, 4, 0, 12);
        let mut pre = Prog::new();
        pre.vv(1, packoff(ox, oy)).vv(2, fb(u)).vv(3, fb(v)).vv(4, fb(lod));
        let m = nearest(lod, 4);
        let (x, y) = pt(m, u, v, (ox, oy));
        cs.push(case("image_sample_l_o", op, build(&pre, &op, 4, 4, false), tex(), ea(m, x, y).to_vec()));
    }
    run_mem_cases("mimg_sample", &cs);
}

#[test]
fn image_mimg_compare_gather() {
    let mut cs = Vec::new();
    let (u, v) = (0.3f32, 0.6f32);
    let d = |x: u32, y: u32| (y * 8 + x) as f32 / 64.0;
    let cmpf = |f: u32, z: f32, t: f32| -> bool {
        match f {
            2 => z < t,
            3 => z == t,
            4 => z <= t,
            5 => z > t,
            6 => z != t,
            7 => z >= t,
            8 => true,
            _ => false,
        }
    };
    let one = |b: bool| fb(if b { 1.0 } else { 0.0 });
    for &(f, z) in &[(2u32, 0.5f32), (2, 0.6), (5, 0.6), (5, 0.5), (3, 34.0 / 64.0), (7, 34.0 / 64.0), (1, 0.0), (8, 0.0)] {
        let op = mimg(0x28, 1, 1, 1, 4, 0, 12);
        let mut pre = Prog::new();
        pre.vv(1, fb(z)).vv(2, fb(u)).vv(3, fb(v));
        let (x, y) = pt(0, u, v, (0, 0));
        cs.push(case("image_sample_c", op, build(&pre, &op, 4, 1, false), Env { tex: Some(&IMG_D), samp: Some(sampler(0x80, 3, f)), ..Default::default() }, vec![one(cmpf(f, z, d(x, y)))]));
    }
    let foot = |m: u32, u: f32, v: f32| -> [(u32, u32); 4] {
        let s = (8u32 >> m) as f32;
        let i0 = (u * s - 0.5).floor() as i32;
        let j0 = (v * s - 0.5).floor() as i32;
        let c = |a: i32| a.clamp(0, s as i32 - 1) as u32;
        [(c(i0), c(j0 + 1)), (c(i0 + 1), c(j0 + 1)), (c(i0 + 1), c(j0)), (c(i0), c(j0))]
    };
    let (gu, gv) = (3.0f32 / 8.0, 4.0f32 / 8.0);
    for &(f, z) in &[(2u32, 30.5f32 / 64.0), (2, 34.5 / 64.0), (5, 30.5 / 64.0), (4, 27.0 / 64.0)] {
        for (mn, opc) in [("image_gather4_c_lz", 0x4fu32), ("image_gather4_c", 0x48)] {
            let op = mimg(opc, 1, 1, 1, 4, 0, 12);
            let mut pre = Prog::new();
            pre.vv(1, fb(z)).vv(2, fb(gu)).vv(3, fb(gv));
            let exp: Vec<u32> = foot(0, gu, gv).iter().map(|&(x, y)| one(cmpf(f, z, f32::from_bits(td(0, 0, x, y)[0])))).collect();
            cs.push(case(mn, op, build(&pre, &op, 4, 4, false), Env { tex: Some(&IMG_E), samp: Some(sampler(0x80, 3, f)), ..Default::default() }, exp));
        }
    }
    for &(k, lod, z) in &[(0.25f32, 1u32, 0.5f32 + 6.5 / 64.0), (0.125, 0, 30.5 / 64.0), (0.25, 1, 0.5 + 5.5 / 64.0)] {
        let op = mimg(0x48, 1, 1, 5, 8, 0, 12);
        let mut pre = Prog::new();
        pre.vv(5, fb(z));
        let (u0, v0) = pos_at(&mut pre, 6, 7, k, 0.55, 0.45);
        let exp: Vec<u32> = foot(lod, u0, v0).iter().map(|&(x, y)| one(cmpf(2, z, f32::from_bits(td(lod, 0, x, y)[0])))).collect();
        cs.push(case("image_gather4_c", op, build(&pre, &op, 8, 4, false), Env { tex: Some(&IMG_E), samp: Some(sampler(0x80, 3, 2)), pos: true, ..Default::default() }, exp));
    }
    for &(f, z, lod) in &[(2u32, 0.5f32, 0.0f32), (2, 0.5, 1.0), (2, 0.5 + 9.5 / 64.0, 1.0), (5, 0.5 + 9.5 / 64.0, 0.6), (5, 34.5 / 64.0, 0.4), (7, 34.0 / 64.0, 0.0)] {
        let op = mimg(0x2c, 1, 1, 1, 5, 0, 12);
        let mut pre = Prog::new();
        pre.vv(1, fb(z)).vv(2, fb(u)).vv(3, fb(v)).vv(4, fb(lod));
        let m = nearest(lod, 2);
        let (x, y) = pt(m, u, v, (0, 0));
        cs.push(case("image_sample_c_l", op, build(&pre, &op, 5, 1, false), Env { tex: Some(&IMG_E), samp: Some(sampler(0x80, 3, f)), ..Default::default() }, vec![one(cmpf(f, z, f32::from_bits(td(m, 0, x, y)[0])))]));
    }
    for &dmask in &[1u32, 2, 4, 8] {
        let ch = dmask.trailing_zeros() as usize;
        let op = mimg(0x40, dmask, 1, 1, 4, 0, 12);
        let mut pre = Prog::new();
        pre.vv(1, fb(gu)).vv(2, fb(gv));
        let exp: Vec<u32> = foot(0, gu, gv).iter().map(|&(x, y)| ea(0, x, y)[ch]).collect();
        cs.push(case("image_gather4", op, build(&pre, &op, 4, 4, false), Env { tex: Some(&IMG_A), samp: ptsamp(), ..Default::default() }, exp));
    }
    for &(k, lod, dmask) in &[(0.25f32, 1u32, 1u32), (0.5, 2, 2), (0.125, 0, 8), (1.0, 3, 1)] {
        let ch = dmask.trailing_zeros() as usize;
        let op = mimg(0x40, dmask, 1, 6, 8, 0, 12);
        let mut pre = Prog::new();
        let (u0, v0) = pos_at(&mut pre, 6, 7, k, 0.55, 0.45);
        let exp: Vec<u32> = foot(lod, u0, v0).iter().map(|&(x, y)| ea(lod, x, y)[ch]).collect();
        cs.push(case("image_gather4", op, build(&pre, &op, 8, 4, false), Env { tex: Some(&IMG_A), samp: ptsamp(), pos: true, ..Default::default() }, exp));
    }
    run_mem_cases("mimg_compare_gather", &cs);
}

#[test]
fn image_mimg_query() {
    let mut cs = Vec::new();
    for &mip in &[0u32, 1, 2] {
        for &dmask in &[0xbu32, 0x8, 0x2, 0x1] {
            let op = mimg(0x0e, dmask, 1, 1, 4, 0, 12);
            let mut pre = Prog::new();
            pre.vv(1, mip);
            let full = [16 >> mip, (4u32 >> mip).max(1), 1, 3];
            cs.push(case("image_get_resinfo", op, build(&pre, &op, 4, dmask.count_ones(), false), Env { tex: Some(&IMG_B), ..Default::default() }, pick(full, dmask)));
        }
    }
    for &mip in &[0u32, 1] {
        let op = mimg(0x0e, 0xf, 5, 1, 4, 0, 12);
        let mut pre = Prog::new();
        pre.vv(1, mip);
        cs.push(case("image_get_resinfo", op, build(&pre, &op, 4, 4, false), Env { tex: Some(&IMG_C), ..Default::default() }, vec![8 >> mip, 8 >> mip, 3, 2]));
    }
    for &(k, lod) in &[(0.125f32, 0.0f32), (0.25, 1.0), (0.5, 2.0), (0.0625, -1.0), (2.0, 4.0)] {
        for &dmask in &[0x3u32, 0x2] {
            let op = mimg(0x60, dmask, 1, 6, 4, 0, 12);
            let mut pre = Prog::new();
            pos_uv(&mut pre, 6, 7, k, 0.1);
            let full = [fb(lod.clamp(0.0, 3.0)), fb(lod), 0, 0];
            let mut c = case("image_get_lod", op, build(&pre, &op, 4, dmask.count_ones(), false), Env { tex: Some(&IMG_A), samp: Some(sampler(1, 1, 0)), pos: true, ..Default::default() }, pick(full, dmask));
            c.tol = if std::env::var_os("TOMBOX_EXEC_HW").is_some() { 1.0 / 256.0 } else { 1.0e-5 };
            cs.push(c);
        }
    }
    run_mem_cases("mimg_query", &cs);
}

#[test]
fn image_mimg_store_mip() {
    let mut cs = Vec::new();
    for &(x, y, mip) in &[(2u32, 3u32, 0u32), (1, 1, 1), (0, 1, 2), (0, 0, 3), (7, 7, 0)] {
        let op = mimg(0x09, 0xf, 1, 1, 4, 16, 0);
        let data = [fb(1.5 + mip as f32), fb(-2.25), fb(1e10), fb(0.125 * x as f32)];
        let mut pre = Prog::new();
        pre.vv(1, x).vv(2, y).vv(3, mip).vv(4, data[0]).vv(5, data[1]).vv(6, data[2]).vv(7, data[3]);
        let mw = 8u32 >> mip;
        let base = ((y * mw + x) * 4) as usize;
        let mut uav: Vec<(usize, usize, u32)> = (0..4).map(|k| (mip as usize, base + k, data[k])).collect();
        let other = ((y * mw + (x ^ 1).min(mw - 1)) * 4) as usize;
        if other != base {
            uav.push((mip as usize, other, 0));
        }
        let nm = ((mip + 1) % 4) as usize;
        uav.push((nm, 0, 0));
        cs.push(MCase { m: "image_store_mip", op: op.to_vec(), words: build(&pre, &op, 4, 0, true), env: Env { uav: Some(&IMG_U), ..Default::default() }, rt: vec![], uav, tol: 0.0 });
    }
    run_mem_cases("mimg_store_mip", &cs);
}

fn h16(x: f32) -> u32 {
    let b = x.to_bits();
    let s = (b >> 16) & 0x8000;
    let a = b & 0x7fff_ffff;
    let r = a - 0x3800_0000;
    s | ((r + 0xfff + ((r >> 13) & 1)) >> 13)
}

fn d16(op: [u32; 2]) -> [u32; 2] {
    [op[0], op[1] | 0x8000_0000]
}

fn pack16(vals: &[u32]) -> Vec<u32> {
    vals.chunks(2).map(|p| h16(f32::from_bits(p[0])) | p.get(1).map_or(0, |&v| h16(f32::from_bits(v)) << 16)).collect()
}

#[test]
fn image_mimg_d16_packs_halves() {
    let mut cs = Vec::new();
    let tex = || Env { tex: Some(&IMG_A), samp: ptsamp(), ..Default::default() };
    for &dmask in &[0xfu32, 0x7, 0x5, 0x2] {
        let (u, v) = (0.45f32, 0.3f32);
        let op = d16(mimg(0x27, dmask, 1, 1, 4, 0, 12));
        let mut pre = Prog::new();
        pre.vv(1, fb(u)).vv(2, fb(v));
        let (x, y) = pt(0, u, v, (0, 0));
        let n = dmask.count_ones().div_ceil(2);
        cs.push(case("image_sample_lz", op, build(&pre, &op, 4, n, false), tex(), pack16(&pick(ea(0, x, y), dmask))));
    }
    for &(x, y, dmask) in &[(3u32, 3u32, 0xfu32), (2, 5, 0xb), (7, 0, 0x8)] {
        let op = d16(mimg(0x00, dmask, 1, 1, 4, 0, 12));
        let mut pre = Prog::new();
        pre.vv(1, x).vv(2, y);
        let n = dmask.count_ones().div_ceil(2);
        cs.push(case("image_load", op, build(&pre, &op, 4, n, false), tex(), pack16(&pick(ea(0, x, y), dmask))));
    }
    for &(x, y, dmask) in &[(2u32, 3u32, 0xfu32), (5, 1, 0x7), (0, 6, 0x6)] {
        let op = d16(mimg(0x08, dmask, 1, 1, 4, 16, 0));
        let vals = [1.5f32, -2.25, 0.125, 65504.0];
        let on: Vec<f32> = (0..4).filter(|c| (dmask >> c) & 1 != 0).map(|c| vals[c]).collect();
        let regs = pack16(&on.iter().map(|v| fb(*v)).collect::<Vec<_>>());
        let mut pre = Prog::new();
        pre.vv(1, x).vv(2, y);
        for (k, r) in regs.iter().enumerate() {
            pre.vv(4 + k as u32, *r);
        }
        let base = ((y * 8 + x) * 4) as usize;
        let mut k = 0usize;
        let uav = (0..4)
            .map(|c| {
                let w = if (dmask >> c) & 1 != 0 {
                    k += 1;
                    fb(on[k - 1])
                } else {
                    0
                };
                (0usize, base + c, w)
            })
            .collect();
        cs.push(MCase { m: "image_store", op: op.to_vec(), words: build(&pre, &op, 4, 0, true), env: Env { uav: Some(&IMG_U), ..Default::default() }, rt: vec![], uav, tol: 0.0 });
    }
    run_mem_cases("mimg_d16", &cs);
}

const AT_INIT: [u32; 8] = [5, 0x10, 0x8000_0000, 0xffff_ffff, 0x20, 0, 0x7fff_ffff, 0x1234_5678];

fn tat(_m: u32, _l: u32, x: u32, y: u32) -> [u32; 4] {
    [if y == 0 { AT_INIT[x as usize] } else { 0x0bad_0000 | (y * 8 + x) }, 0, 0, 0]
}

const IMG_AT: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 1, fmt: R32_UINT, texel: tat };

#[test]
fn image_mimg_atomics() {
    type At = fn(u32, u32, u32) -> u32;
    let atoms: [(&'static str, u32, At); 13] = [
        ("image_atomic_swap", 0x0f, |_, s, _| s),
        ("image_atomic_cmpswap", 0x10, |m, s, c| if m == c { s } else { m }),
        ("image_atomic_add", 0x11, |m, s, _| m.wrapping_add(s)),
        ("image_atomic_sub", 0x12, |m, s, _| m.wrapping_sub(s)),
        ("image_atomic_smin", 0x14, |m, s, _| (m as i32).min(s as i32) as u32),
        ("image_atomic_umin", 0x15, |m, s, _| m.min(s)),
        ("image_atomic_smax", 0x16, |m, s, _| (m as i32).max(s as i32) as u32),
        ("image_atomic_umax", 0x17, |m, s, _| m.max(s)),
        ("image_atomic_and", 0x18, |m, s, _| m & s),
        ("image_atomic_or", 0x19, |m, s, _| m | s),
        ("image_atomic_xor", 0x1a, |m, s, _| m ^ s),
        ("image_atomic_inc", 0x1b, |m, s, _| if m >= s { 0 } else { m + 1 }),
        ("image_atomic_dec", 0x1c, |m, s, _| if m == 0 || m > s { s } else { m - 1 }),
    ];
    let mut cs = Vec::new();
    for (m, opc, f) in atoms {
        let dmask = if opc == 0x10 { 0x3 } else { 0x1 };
        for &(x, s, cmp) in &[(0u32, 5u32, 0xffff_fff0u32), (1, 0x10, 0x10), (2, 0x8000_0000, 0x8000_0000), (3, 7, 0xffff_ffff), (4, 0x20, 1), (5, 0, 0), (6, 0xffff_ffff, 0x7fff_ffff), (7, 0x1234_5678, 3)] {
            let old = AT_INIT[x as usize];
            let mut pre = Prog::new();
            pre.vv(1, x).vv(2, 0).vv(4, s).vv(5, cmp);
            let uav = vec![(0usize, x as usize, f(old, s, cmp)), (0, (x ^ 1) as usize, AT_INIT[(x ^ 1) as usize]), (0, 8 + x as usize, 0x0bad_0008 + x)];
            for glc in [true, false] {
                let mut op = mimg(opc, dmask, 1, 1, 4, 16, 0);
                if glc {
                    op[0] |= 1 << 13;
                }
                let rt = vec![if glc { old } else { s }];
                cs.push(MCase { m, op: op.to_vec(), words: build(&pre, &op, 4, 1, true), env: Env { uav: Some(&IMG_AT), ..Default::default() }, rt, uav: uav.clone(), tol: 0.0 });
            }
        }
    }
    run_mem_cases("mimg_atomics", &cs);
}

fn tu4(_m: u32, _l: u32, x: u32, y: u32) -> [u32; 4] {
    match (x, y) {
        (0, 0) => [0x0000_1234, 0x0001_0000, 0xffff_ffff, 0x0000_0000],
        (2, 5) => [0x0000_ffff, 0x0000_8000, 0x0002_0001, 0x0000_0001],
        _ => [x * 10000, y * 20000, 0x1_2345, 0xabcd],
    }
}

fn ti4(_m: u32, _l: u32, x: u32, y: u32) -> [u32; 4] {
    let s = |v: i32| v as u32;
    match (x, y) {
        (0, 0) => [s(100), s(-100), s(50_000), s(-50_000)],
        (2, 5) => [s(32_767), s(-32_768), s(32_768), s(-32_769)],
        _ => [s(0), s(-1), s(i32::MAX), s(i32::MIN)],
    }
}

const IMG_UINT4: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 1, fmt: R32G32B32A32_UINT, texel: tu4 };
const IMG_SINT4: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 1, fmt: R32G32B32A32_SINT, texel: ti4 };
const IMG_UINT4_U: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 1, fmt: R32G32B32A32_UINT, texel: tzero };
const IMG_SINT4_U: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 1, fmt: R32G32B32A32_SINT, texel: tzero };

fn pack_u16_sat(vals: &[u32]) -> Vec<u32> {
    let s = |x: u32| x.min(0xffff);
    vals.chunks(2).map(|p| s(p[0]) | p.get(1).map_or(0, |&v| s(v) << 16)).collect()
}

fn pack_i16_sat(vals: &[u32]) -> Vec<u32> {
    let s = |x: u32| ((x as i32).clamp(-32768, 32767) as u32) & 0xffff;
    vals.chunks(2).map(|p| s(p[0]) | p.get(1).map_or(0, |&v| s(v) << 16)).collect()
}

#[test]
fn image_mimg_d16_integer_images() {
    let mut cs = Vec::new();
    for &(x, y) in &[(0u32, 0u32), (2, 5), (3, 1)] {
        for &dmask in &[0xfu32, 0x7, 0x5, 0x3, 0x1, 0x9, 0xa] {
            let op = d16(mimg(0x00, dmask, 1, 1, 4, 0, 12));
            let n = dmask.count_ones().div_ceil(2);
            let mut p = Prog::new();
            p.raw(&smem(3, 0, P_SRV, 0, SNULL))
                .vv(1, x)
                .vv(2, y)
                .raw(&op)
                .raw(&[vop2(0x14, 10, v(4), 4)]);
            for k in 0..n {
                p.raw(&[vop1(1, k, v(4 + k))]);
            }
            cs.push(case(
                "image_load",
                op,
                p.words(),
                Env { tex: Some(&IMG_UINT4), ..Default::default() },
                pack_u16_sat(&pick(tu4(0, 0, x, y), dmask)),
            ));

            let mut p = Prog::new();
            p.raw(&smem(3, 0, P_SRV, 0, SNULL))
                .vv(1, x)
                .vv(2, y)
                .raw(&op)
                .raw(&[vop2(0x12, 10, v(4), 4)]);
            for k in 0..n {
                p.raw(&[vop1(1, k, v(4 + k))]);
            }
            cs.push(case(
                "image_load",
                op,
                p.words(),
                Env { tex: Some(&IMG_SINT4), ..Default::default() },
                pack_i16_sat(&pick(ti4(0, 0, x, y), dmask)),
            ));
        }
    }

    let halves: [u16; 4] = [0x7fff, 0x8000, 0xff9c, 0xffff];
    for &dmask in &[0xfu32, 0x7, 0x5, 0x3, 0x1, 0x9, 0xa] {
        let (x, y) = (2u32, 3u32);
        let op = d16(mimg(0x08, dmask, 1, 1, 4, 16, 0));
        let on: Vec<u16> = (0..4).filter(|c| (dmask >> c) & 1 != 0).map(|c| halves[c]).collect();
        let regs: Vec<u32> = on.chunks(2).map(|p| (p[0] as u32) | p.get(1).map_or(0, |&v| (v as u32) << 16)).collect();
        let base = ((y * 8 + x) * 4) as usize;

        for (signed, img, tag_op) in [(false, &IMG_UINT4_U, 0x14u32), (true, &IMG_SINT4_U, 0x12u32)] {
            let mut pre = Prog::new();
            pre.vv(1, x).vv(2, y);
            for (k, r) in regs.iter().enumerate() {
                pre.vv(4 + k as u32, *r);
            }
            pre.raw(&[vop2(tag_op, 4, v(4), 4)]);
            let mut k = 0usize;
            let uav = (0..4)
                .map(|c| {
                    let w = if (dmask >> c) & 1 != 0 {
                        let h = on[k];
                        k += 1;
                        if signed { (h as i16 as i32) as u32 } else { h as u32 }
                    } else {
                        0
                    };
                    (0usize, base + c, w)
                })
                .collect();
            cs.push(MCase {
                m: "image_store",
                op: op.to_vec(),
                words: build(&pre, &op, 4, 0, true),
                env: Env { uav: Some(img), ..Default::default() },
                rt: vec![],
                uav,
                tol: 0.0,
            });
        }
    }
    run_mem_cases("mimg_d16_int", &cs);
}

fn unorm(op: [u32; 2]) -> [u32; 2] {
    [op[0] | (1 << 12), op[1]]
}

fn a16(op: [u32; 2]) -> [u32; 2] {
    [op[0], op[1] | (1 << 30)]
}

#[test]
fn image_mimg_unorm_a16_and_refusals() {
    let mut cs = Vec::new();
    let tex_a = || Env { tex: Some(&IMG_A), samp: ptsamp(), ..Default::default() };
    let tex_c = || Env { tex: Some(&IMG_C), samp: ptsamp(), ..Default::default() };

    for &(tx, ty) in &[(0.5f32, 0.5f32), (2.5, 4.5), (7.5, 1.5)] {
        let op = unorm(mimg(0x27, 0xf, 1, 1, 4, 0, 12));
        let mut pre = Prog::new();
        pre.vv(1, fb(tx)).vv(2, fb(ty));
        let (x, y) = (tx.floor() as u32, ty.floor() as u32);
        cs.push(case("image_sample_lz", op, build(&pre, &op, 4, 4, false), tex_a(), ea(0, x, y).to_vec()));
    }
    for &(tx, ty, slice) in &[(1.5f32, 2.5f32, 0.0f32), (3.5, 6.5, 2.0)] {
        let op = unorm(mimg(0x27, 0xf, 5, 1, 4, 0, 12));
        let mut pre = Prog::new();
        pre.vv(1, fb(tx)).vv(2, fb(ty)).vv(3, fb(slice));
        let (x, y) = (tx.floor() as u32, ty.floor() as u32);
        cs.push(case("image_sample_lz", op, build(&pre, &op, 4, 4, false), tex_c(), ta(0, slice as u32, x, y).to_vec()));
    }
    for &(tx, ty, lod) in &[(2.5f32, 4.5f32, 1.0f32), (6.5, 2.5, 2.0)] {
        let op = unorm(mimg(0x24, 0xf, 1, 1, 4, 0, 12));
        let mut pre = Prog::new();
        pre.vv(1, fb(tx)).vv(2, fb(ty)).vv(3, fb(lod));
        let m = nearest(lod, 4);
        let (x, y) = pt(m, tx / 8.0, ty / 8.0, (0, 0));
        cs.push(case("image_sample_l", op, build(&pre, &op, 4, 4, false), tex_a(), ea(m, x, y).to_vec()));
    }

    for &(x, y) in &[(2u32, 5u32), (7, 3), (0, 0)] {
        let op = a16(mimg(0x00, 0xf, 1, 1, 4, 0, 12));
        let mut pre = Prog::new();
        pre.vv(1, x | (y << 16));
        cs.push(case("image_load", op, build(&pre, &op, 4, 4, false), tex_a(), ea(0, x, y).to_vec()));
    }
    {
        let (x, y) = (4u32, 6u32);
        let op = a16(mimg(0x08, 0xf, 1, 1, 4, 16, 0));
        let vals = [fb(1.25), fb(-3.5), fb(42.0), fb(0.5)];
        let mut pre = Prog::new();
        pre.vv(1, x | (y << 16)).vv(4, vals[0]).vv(5, vals[1]).vv(6, vals[2]).vv(7, vals[3]);
        let base = ((y * 8 + x) * 4) as usize;
        let uav = (0..4).map(|c| (0usize, base + c, vals[c])).collect();
        cs.push(MCase { m: "image_store", op: op.to_vec(), words: build(&pre, &op, 4, 0, true), env: Env { uav: Some(&IMG_U), ..Default::default() }, rt: vec![], uav, tol: 0.0 });
    }
    for &(u, v) in &[(0.3125f32, 0.5625f32), (0.8125, 0.1875)] {
        let op = a16(mimg(0x27, 0xf, 1, 1, 4, 0, 12));
        let mut pre = Prog::new();
        pre.vv(1, h16(u) | (h16(v) << 16));
        let (x, y) = pt(0, u, v, (0, 0));
        cs.push(case("image_sample_lz", op, build(&pre, &op, 4, 4, false), tex_a(), ea(0, x, y).to_vec()));
    }
    {
        let (u, v, lod) = (0.3125f32, 0.5625f32, 2.0f32);
        let op = a16(mimg(0x24, 0xf, 1, 1, 4, 0, 12));
        let mut pre = Prog::new();
        pre.vv(1, h16(u) | (h16(v) << 16)).vv(2, h16(lod));
        let m = nearest(lod, 4);
        let (x, y) = pt(m, u, v, (0, 0));
        cs.push(case("image_sample_l", op, build(&pre, &op, 4, 4, false), tex_a(), ea(m, x, y).to_vec()));
    }
    run_mem_cases("mimg_unorm_a16", &cs);

    let mimg_base = mimg(0x27, 0xf, 1, 1, 4, 0, 12);
    for (bad_op, needle) in [
        ([mimg_base[0] | (1 << 16), mimg_base[1]], "TFE"),
        ([mimg_base[0] | (1 << 17), mimg_base[1]], "LWE"),
        (unorm(mimg(0x27, 0xf, 3, 1, 4, 0, 12)), "cube"),
    ] {
        let mut pre = Prog::new();
        pre.vv(1, fb(0.5)).vv(2, fb(0.5)).vv(3, fb(0.0));
        let words = build(&pre, &bad_op, 4, 4, false);
        let err = match translate(&extract(&words, false)) {
            Ok(t) => panic!("expected refusal for {needle}, got:\n{}", t.hlsl),
            Err(e) => e,
        };
        assert!(err.contains(needle), "expected {needle} in error: {err}");
    }
    for (bad_mubuf, needle) in [
        ([0xe000_0000 | (0x0c << 18) | (1 << 16), 0x0002_0401], "MUBUF LDS"),
        ([0xe000_0000 | (0x0c << 18), 0x0082_0401], "MUBUF TFE"),
    ] {
        let mut p = Prog::new();
        p.raw(&smem(2, 8, P_SRV, 0, SNULL)).vv(1, 0).raw(&bad_mubuf).raw(&[vop1(1, 0, v(4))]);
        let err = match translate(&extract(&p.words(), false)) {
            Ok(t) => panic!("expected refusal for {needle}, got:\n{}", t.hlsl),
            Err(e) => e,
        };
        assert!(err.contains(needle), "expected {needle} in error: {err}");
    }
}

#[test]
fn image_mimg_atomics_rdna2_silicon() {
    let mut cs = Vec::new();
    for &(m, opc, glc, x, s, cmp, texels, ret) in super::mimg_rdna2::ATOMICS {
        let dmask = if opc == 0x10 { 0x3 } else { 0x1 };
        let mut pre = Prog::new();
        pre.vv(1, x).vv(2, 0).vv(4, s).vv(5, cmp);
        let mut op = mimg(opc, dmask, 1, 1, 4, 16, 0);
        if glc {
            op[0] |= 1 << 13;
        }
        let uav = texels.iter().enumerate().map(|(i, &w)| (0usize, i, w)).collect();
        cs.push(MCase { m, op: op.to_vec(), words: build(&pre, &op, 4, 1, true), env: Env { uav: Some(&IMG_AT), ..Default::default() }, rt: vec![ret], uav, tol: 0.0 });
    }
    println!("expected values recorded on {}", super::mimg_rdna2::SOURCE);
    run_mem_cases("mimg_atomics_rdna2_silicon", &cs);
}
