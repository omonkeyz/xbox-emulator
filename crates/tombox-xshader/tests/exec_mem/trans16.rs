use super::*;
use tombox_xshader::trans16;

const THREADS: u32 = 1024;
const PER: u32 = 64;
const OPCODES: [u32; 7] = [0x54, 0x56, 0x55, 0x57, 0x58, 0x60, 0x61];

fn store_dword(vdata: u32, vaddr: u32) -> [u32; 2] {
    [0xe000_0000 | (0x1c << 18) | (1 << 12), vaddr | (vdata << 8) | (2 << 16) | (0x80 << 24)]
}

fn sweep_case(op: usize) -> CsCase {
    let opw = vop1(OPCODES[op], 10, v(1));
    let mut w = smem(2, 8, 4, 0, SNULL).to_vec();
    w.extend_from_slice(&[vop2(0x1a, 3, ic(6), 0), vop1(1, 11, ic(0)), sop1(3, 20, ic(0))]);
    let body = [vop2(0x25, 1, 20, 3), vop1(1, 10, ic(0)), opw, vop2(0x1a, 12, ic(5), 11), vop2(0x25, 11, v(11), 12), vop2(0x1d, 11, v(10), 11), sop2(0, 20, 20, ic(1)), sopc(0x0a, 20, ic(PER as i32))];
    w.extend_from_slice(&body);
    w.push(sopp(5, (-(body.len() as i32 + 1)) as u16));
    w.extend_from_slice(&[vop2(0x1a, 2, ic(2), 0)]);
    w.extend_from_slice(&store_dword(11, 2));
    w.push(S_ENDPGM);
    let mem = (0..THREADS)
        .map(|t| {
            let h = (0..PER).fold(0u32, |h, i| h.wrapping_mul(33) ^ trans16::eval(op, (t * PER + i) as u16) as u32);
            (t as usize, h)
        })
        .collect();
    CsCase { m: trans16::OPS[op], op: vec![opw], words: w, threads: THREADS, mem, init: vec![] }
}

#[test]
fn f16_transcendentals_exact_on_every_input() {
    let cs: Vec<CsCase> = (0..trans16::OPS.len()).map(sweep_case).collect();
    run_cs_cases("trans16_all_inputs", &cs);
}
