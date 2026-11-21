use super::*;

fn flat(op: u32, seg: u32, offset: i32, addr: u32, saddr: u32, vdst: u32) -> [u32; 2] {
    [0xdc00_0000 | (op << 18) | (seg << 14) | (offset as u32 & 0xfff), addr | (saddr << 16) | (vdst << 24)]
}

const TABLE: [u32; 12] = [0x0102_0304, 0xa5a5_5a5a, 0xdead_beef, 0x7fff_ffff, 0x8000_0001, 0x1234_5678, 0x0, 0xffff_ffff, 0x3f80_0000, 0xbf80_0000, 0x0bad_f00d, 0xcafe_babe];

fn prog(op: [u32; 2], global: bool, idx_bytes: i64, n: u32) -> Vec<u32> {
    let mk = |disp: u32| {
        let mut p = Prog::new();
        p.raw(&[sop1(0x1f, 20, 0)]);
        if global {
            p.raw(&[sop2(0, 20, 20, LIT), disp]);
            p.vv(1, idx_bytes as u32).vv(2, 0);
        } else {
            p.raw(&[vop1(1, 1, 20)]).vv(2, 0);
            p.raw(&[vop2(0x25, 1, LIT, 1), disp.wrapping_add(idx_bytes as u32)]);
        }
        p.raw(&op);
        for k in 0..n {
            p.raw(&[vop1(1, k, v(4 + k))]);
        }
        p.words()
    };
    let len = mk(0).len() as u32;
    mk(4 * len - 4)
}

#[test]
fn flat_global_code_loads() {
    let mut cs = Vec::new();
    for (m, opc, n) in [("flat_load_dword", 0x0cu32, 1u32), ("flat_load_dwordx2", 0x0d, 2), ("flat_load_dwordx3", 0x0f, 3), ("flat_load_dwordx4", 0x0e, 4)] {
        // Encoded offset field: global takes it as 12-bit signed; FLAT as 11-bit unsigned with
        // the MSB ignored (RDNA2 ISA, FLAT fields), so 0x810 means +16 there.
        for &(idx, off) in &[(0i64, 0i32), (4, 0), (8, 16), (32, -8), (16, 4), (8, 0x810)] {
            for (global, seg, saddr) in [(false, 0u32, 0x7du32), (true, 2, 20)] {
                let eff = if global { off << 20 >> 20 } else { off & 0x7ff };
                if idx + (eff as i64) < 0 || (idx + eff as i64) / 4 + n as i64 > TABLE.len() as i64 {
                    continue;
                }
                let op = flat(opc, seg, off, 1, saddr, 4);
                let w0 = ((idx + eff as i64) / 4) as usize;
                let rt: Vec<u32> = (0..n as usize).map(|k| TABLE[w0 + k]).collect();
                cs.push(MCase { m, op: op.to_vec(), words: prog(op, global, idx, n), env: Env { code: Some(&TABLE), ..Default::default() }, rt, uav: vec![], tol: 0.0 });
            }
        }
    }
    run_mem_cases("flat", &cs);
}
