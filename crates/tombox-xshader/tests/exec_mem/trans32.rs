use super::*;
use tombox_xshader::trans32;

const THREADS: u32 = 1024;
const PER: u32 = 1024;
const OPCODES: [u32; 8] = [0x2a, 0x2b, 0x2e, 0x33, 0x27, 0x25, 0x35, 0x36];
const RANGES: [&[(u32, u32)]; 8] = [
    &[(0x3f80_0000, 8)],
    &[(0x3f80_0000, 8)],
    &[(0x3f80_0000, 16)],
    &[(0x3f80_0000, 16)],
    &[(0x3f00_0000, 16)],
    &[(0x3f80_0000, 8), (0xbf80_0000, 8)],
    &[(0x3980_0000, 8), (0x3e80_0000, 8), (0x4080_0000, 8)],
    &[(0x3980_0000, 8), (0x3e80_0000, 8), (0x4080_0000, 8)],
];

fn store_dword(vdata: u32, vaddr: u32) -> [u32; 2] {
    [0xe000_0000 | (0x1c << 18) | (1 << 12), vaddr | (vdata << 8) | (2 << 16) | (0x80 << 24)]
}

fn sweep_case(op: usize, base: u32, spread: bool) -> CsCase {
    let opw = vop1(OPCODES[op], 10, v(1));
    let mut w = smem(2, 8, 4, 0, SNULL).to_vec();
    w.extend_from_slice(&[vop2(0x1a, 3, ic(10), 0), vop1(1, 11, ic(0)), sop1(3, 20, ic(0))]);
    let mut body = vec![vop2(0x25, 1, 20, 3)];
    if spread {
        body.extend_from_slice(&[vop2(0x0b, 1, LIT, 1), 4099]);
    } else {
        body.extend_from_slice(&[vop2(0x25, 1, LIT, 1), base]);
    }
    body.extend_from_slice(&[opw, vop2(0x1a, 12, ic(5), 11), vop2(0x25, 11, v(11), 12), vop2(0x1d, 11, v(10), 11), sop2(0, 20, 20, ic(1)), sopc(0x0a, 20, LIT), PER]);
    w.extend_from_slice(&body);
    w.push(sopp(5, (-(body.len() as i32 + 1)) as u16));
    w.extend_from_slice(&[vop2(0x1a, 2, ic(2), 0)]);
    w.extend_from_slice(&store_dword(11, 2));
    w.push(S_ENDPGM);
    let input = |t: u32, i: u32| if spread { (t * PER + i).wrapping_mul(4099) } else { base.wrapping_add(t * PER + i) };
    let mem = (0..THREADS)
        .map(|t| {
            let h = (0..PER).fold(0u32, |h, i| h.wrapping_mul(33) ^ trans32::eval(op, input(t, i)));
            (t as usize, h)
        })
        .collect();
    CsCase { m: trans32::OPS[op], op: vec![opw], words: w, threads: THREADS, mem, init: vec![] }
}

#[test]
fn f32_transcendentals_exact_on_every_mantissa() {
    let mut cs = Vec::new();
    for op in 0..trans32::OPS.len() {
        for &(base, blocks) in RANGES[op] {
            for b in 0..blocks {
                cs.push(sweep_case(op, base + b * THREADS * PER, false));
            }
        }
        cs.push(sweep_case(op, 0, true));
    }
    run_cs_cases("trans32_all_mantissas", &cs);
}

const OPCODES64: [u32; 3] = [0x2f, 0x31, 0x34];

fn sweep64_case(op: usize, hi_base: u32, lo: u32, spread: bool) -> CsCase {
    let opw = vop1(OPCODES64[op], 14, v(1));
    let mut w = smem(2, 8, 4, 0, SNULL).to_vec();
    w.extend_from_slice(&[vop2(0x1a, 3, ic(10), 0), vop1(1, 11, ic(0)), sop1(3, 20, ic(0))]);
    let mut body = vec![vop1(1, 1, LIT), lo, vop2(0x25, 2, 20, 3)];
    if spread {
        body.extend_from_slice(&[vop2(0x0b, 2, LIT, 2), 4099]);
    } else {
        body.extend_from_slice(&[vop2(0x25, 2, LIT, 2), hi_base]);
    }
    body.extend_from_slice(&[opw, vop2(0x1a, 12, ic(5), 11), vop2(0x25, 11, v(11), 12), vop2(0x1d, 11, v(14), 11), vop2(0x1a, 12, ic(5), 11), vop2(0x25, 11, v(11), 12), vop2(0x1d, 11, v(15), 11), sop2(0, 20, 20, ic(1)), sopc(0x0a, 20, LIT), PER]);
    w.extend_from_slice(&body);
    w.push(sopp(5, (-(body.len() as i32 + 1)) as u16));
    w.extend_from_slice(&[vop2(0x1a, 2, ic(2), 0)]);
    w.extend_from_slice(&store_dword(11, 2));
    w.push(S_ENDPGM);
    let input = |t: u32, i: u32| {
        let g = t * PER + i;
        let hi = if spread { g.wrapping_mul(4099) } else { hi_base.wrapping_add(g) };
        ((hi as u64) << 32) | lo as u64
    };
    let mem = (0..THREADS)
        .map(|t| {
            let h = (0..PER).fold(0u32, |h, i| {
                let r = trans32::eval64(op, input(t, i));
                (h.wrapping_mul(33) ^ r as u32).wrapping_mul(33) ^ (r >> 32) as u32
            });
            (t as usize, h)
        })
        .collect();
    CsCase { m: trans32::OPS64[op], op: vec![opw], words: w, threads: THREADS, mem, init: vec![] }
}

#[test]
fn f64_rcp_rsq_sqrt_exact_across_mantissas_and_corners() {
    let los = [0u32, 0x0780_0000, 0x9e37_79b9, 0xff80_0000, 0xffff_ffff, 0x0000_0001];
    let mut cs = Vec::new();
    for op in 0..trans32::OPS64.len() {
        for hi in [0x3ff0_0000u32, 0x4000_0000] {
            for lo in los {
                cs.push(sweep64_case(op, hi, lo, false));
            }
        }
        for lo in [0u32, 0x9e37_79b9, 0xffff_ffff] {
            cs.push(sweep64_case(op, 0, lo, true));
        }
    }
    run_cs_cases("trans64_all_mantissas", &cs);
}
