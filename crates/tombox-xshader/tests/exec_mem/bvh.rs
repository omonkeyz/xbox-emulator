use super::mimg::mimg;
use super::*;

const BASE: u64 = 0x1_0000_0000;
const TRI_OFF: usize = 128;

struct Ray {
    tmax: f32,
    o: [f32; 3],
    d: [f32; 3],
}

impl Ray {
    fn inv(&self) -> [f32; 3] {
        [1.0 / self.d[0], 1.0 / self.d[1], 1.0 / self.d[2]]
    }
}

fn box_ref(ids: [u32; 4], bx: &[[f32; 6]; 4], r: &Ray, sort: bool) -> [u32; 4] {
    let inv = r.inv().map(|x| x.clamp(-f32::MAX, f32::MAX));
    let mut dist = [f32::INFINITY; 4];
    let mut out = [0xffff_ffffu32; 4];
    for i in 0..4 {
        let b = &bx[i];
        let t0: Vec<f32> = (0..3).map(|k| (b[k] - r.o[k]) * inv[k]).collect();
        let t1: Vec<f32> = (0..3).map(|k| (b[3 + k] - r.o[k]) * inv[k]).collect();
        let lo = t0[0].min(t1[0]).max(t0[1].min(t1[1])).max(t0[2].min(t1[2]));
        let hi = t0[0].max(t1[0]).min(t0[1].max(t1[1])).min(t0[2].max(t1[2]));
        if !b[0].is_nan() && hi >= lo.max(0.0) && lo < r.tmax {
            out[i] = ids[i];
            dist[i] = lo;
        }
    }
    if sort {
        for (a, b) in [(0, 1), (2, 3), (0, 2), (1, 3), (1, 2)] {
            if dist[b] < dist[a] {
                dist.swap(a, b);
                out.swap(a, b);
            }
        }
    }
    out
}

fn tri_ref(v: [[f32; 3]; 3], r: &Ray) -> [u32; 4] {
    let miss = [f32::INFINITY.to_bits(), 1.0f32.to_bits(), 0, 0];
    let ad = r.d.map(f32::abs);
    let kz = if ad[0] >= ad[1] { if ad[0] >= ad[2] { 0 } else { 2 } } else if ad[1] >= ad[2] { 1 } else { 2 };
    let mut kx = (kz + 1) % 3;
    let mut ky = (kx + 1) % 3;
    if r.d[kz] < 0.0 {
        core::mem::swap(&mut kx, &mut ky);
    }
    let sz = 1.0 / r.d[kz];
    let (sx, sy) = (r.d[kx] * sz, r.d[ky] * sz);
    let rel = |p: [f32; 3]| [p[0] - r.o[0], p[1] - r.o[1], p[2] - r.o[2]];
    let (a, b, c) = (rel(v[0]), rel(v[1]), rel(v[2]));
    let (ax, ay) = (a[kx] - sx * a[kz], a[ky] - sy * a[kz]);
    let (bx, by) = (b[kx] - sx * b[kz], b[ky] - sy * b[kz]);
    let (cx, cy) = (c[kx] - sx * c[kz], c[ky] - sy * c[kz]);
    let u = cx * by - cy * bx;
    let vv = ax * cy - ay * cx;
    let w = bx * ay - by * ax;
    let back = u < 0.0 || vv < 0.0 || w < 0.0;
    let front = u > 0.0 || vv > 0.0 || w > 0.0;
    if back && front {
        return miss;
    }
    let det = u + (vv + w);
    if det == 0.0 {
        return miss;
    }
    let t = u * (sz * a[kz]) + vv * (sz * b[kz]) + w * (sz * c[kz]);
    if det.signum() * t < 0.0 {
        return miss;
    }
    [t.to_bits(), det.to_bits(), vv.to_bits(), w.to_bits()]
}

fn h(x: f32) -> u32 {
    f2h(x)
}

fn mem(ids: [u32; 4], bx: &[[f32; 6]; 4], half: bool, tri: [[f32; 3]; 3], tri_id: u32) -> Vec<u32> {
    let mut m = vec![0u32; 64];
    m[..4].copy_from_slice(&ids);
    for i in 0..4 {
        let b = &bx[i];
        if half {
            let w = [h(b[0]) | (h(b[1]) << 16), h(b[2]) | (h(b[3]) << 16), h(b[4]) | (h(b[5]) << 16)];
            m[4 + 3 * i..7 + 3 * i].copy_from_slice(&w);
        } else {
            for k in 0..6 {
                m[4 + 6 * i + k] = b[k].to_bits();
            }
        }
    }
    let t = TRI_OFF / 4;
    for (k, p) in tri.iter().enumerate() {
        for j in 0..3 {
            m[t + 3 * k + j] = p[j].to_bits();
        }
    }
    m[t + 12] = tri_id;
    m
}

fn prog(wide: bool, node: u32, r: &Ray, sort: bool, ij: bool) -> (Vec<u32>, [u32; 2]) {
    let mut p = Prog::new();
    p.sv(0, (BASE >> 8) as u32).sv(1, ((BASE >> 40) as u32 & 0xff) | if sort { 1 << 31 } else { 0 }).sv(2, 0xffff_ffff).sv(3, 0xff | if ij { 1 << 24 } else { 0 });
    let inv = r.inv();
    let mut a = vec![node];
    if wide {
        a.push(0);
    }
    a.push(r.tmax.to_bits());
    a.extend(r.o.iter().map(|x| x.to_bits()));
    a.extend(r.d.iter().map(|x| x.to_bits()));
    a.extend(inv.iter().map(|x| x.to_bits()));
    for (k, w) in a.iter().enumerate() {
        p.vv(8 + k as u32, *w);
    }
    let op = mimg(if wide { 0xe7 } else { 0xe6 }, 0xf, 0, 8, 4, 0, 0);
    p.raw(&op);
    for k in 0..4 {
        p.raw(&[vop1(1, k, v(4 + k))]);
    }
    (p.words(), op)
}

#[test]
fn image_mimg_bvh() {
    let ids = [0x11u32, 0x22, 0x33, 0x44];
    let mut boxes = [[2.0f32, -1.0, -1.0, 3.0, 1.0, 1.0], [5.0, -1.0, -1.0, 6.0, 1.0, 1.0], [1.0, -1.0, -1.0, 1.5, 1.0, 1.0], [2.0, 5.0, 5.0, 3.0, 6.0, 6.0]];
    let tri = [[-1.0f32, -1.0, 4.0], [3.0, -1.0, 4.0], [-1.0, 3.0, 4.0]];
    let rays = [
        Ray { tmax: 100.0, o: [0.0, 0.0, 0.0], d: [1.0, 0.0, 0.0] },
        Ray { tmax: 3.0, o: [0.0, 0.0, 0.0], d: [1.0, 0.0, 0.0] },
        Ray { tmax: 100.0, o: [0.0, 0.25, 0.5], d: [2.0, 0.0, 0.0] },
        Ray { tmax: 100.0, o: [7.0, 0.0, 0.0], d: [-1.0, 0.0, 0.0] },
        Ray { tmax: 100.0, o: [0.0, 0.0, 0.0], d: [0.0, 0.0, 1.0] },
    ];
    let tri_rays = [
        Ray { tmax: 100.0, o: [0.0, 0.0, 0.0], d: [0.0, 0.0, 1.0] },
        Ray { tmax: 100.0, o: [0.5, 0.25, 0.0], d: [0.0, 0.0, 2.0] },
        Ray { tmax: 100.0, o: [5.0, 5.0, 0.0], d: [0.0, 0.0, 1.0] },
        Ray { tmax: 100.0, o: [0.0, 0.0, 0.0], d: [0.0, 0.0, -1.0] },
        Ray { tmax: 100.0, o: [0.0, 0.0, 8.0], d: [0.0, 0.0, -1.0] },
        Ray { tmax: 100.0, o: [0.0, -1.0, 0.0], d: [0.0, 0.0, 1.0] },
        Ray { tmax: 100.0, o: [-1.0, 0.0, 0.0], d: [0.0, 0.0, 1.0] },
        Ray { tmax: 100.0, o: [0.0, 0.0, 0.0], d: [0.25, 0.125, 1.0] },
    ];
    let mut store: Vec<(Vec<u32>, Vec<u32>, [u32; 2], Vec<u32>, &'static str)> = Vec::new();
    for nan in [false, true] {
        if nan {
            boxes[2][0] = f32::NAN;
        }
        for half in [false, true] {
            let m = mem(ids, &boxes, half, tri, 0x5a5a);
            for r in &rays {
                for sort in [true, false] {
                    for wide in [false, true] {
                        let (w, op) = prog(wide, if half { 4 } else { 5 }, r, sort, true);
                        let name = if wide { "image_bvh64_intersect_ray" } else { "image_bvh_intersect_ray" };
                        store.push((w, m.clone(), op, box_ref(ids, &boxes, r, sort).to_vec(), name));
                    }
                }
            }
        }
    }
    let m = mem(ids, &boxes, false, tri, 0x5a5a);
    let node = ((TRI_OFF as u32 >> 6) << 3) | 0;
    for r in &tri_rays {
        for wide in [false, true] {
            let (w, op) = prog(wide, node, r, true, true);
            let name = if wide { "image_bvh64_intersect_ray" } else { "image_bvh_intersect_ray" };
            store.push((w, m.clone(), op, tri_ref(tri, r).to_vec(), name));
        }
    }
    let cs: Vec<MCase> = store
        .iter()
        .map(|(w, m, op, exp, name)| MCase { m: name, op: op.to_vec(), words: w.clone(), env: Env { gmem: Some((m.as_slice(), BASE)), ..Default::default() }, rt: exp.clone(), uav: vec![], tol: 0.0 })
        .collect();
    run_mem_cases("mimg_bvh", &cs);
}
