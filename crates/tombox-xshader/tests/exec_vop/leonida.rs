
use super::super::*;
use super::lanes::{all_exec, hw_requested, run_lane_cases, Dev, LaneCase, LANE};

fn case(c: &mut Cases, m: &'static str, pre: &Prog, op: &[u32], outs: &[u32], exp: &[u32]) {
    let mut post = pr();
    for (k, &o) in outs.iter().enumerate() {
        if o >= 1000 { post.s2v(k as u32, o - 1000) } else { post.raw(&[vop1(1, k as u32, v(o))]) };
    }
    c.add(m, pre, op, &post, exp);
}

fn sdwa_pre() -> Prog {
    let mut p = pr();
    p.vv(0, 0x8081_f0f7).vv(1, 0x0001_0005);
    p
}

#[test]
fn leonida_ps_vectors() {
    let mut c = Cases::new();
    case(&mut c, "v_add_nc_u32", &sdwa_pre(), &[0x4a04_02f9, 0x0501_0600], &[2], &[0xf1]);
    case(&mut c, "v_subrev_nc_u32", &sdwa_pre(), &[0x4e06_00f9, 0x0806_0601], &[3], &[0xfffe_fff2]);
    let mut p = sdwa_pre();
    p.vv(4, 0x1122_3344).vv(5, 0x3f80_0000);
    case(&mut c, "v_cvt_f16_f32", &p, &[0x7e08_14f9, 0x0006_1505], &[4], &[0x3c00_3344]);
    let mut p = sdwa_pre();
    p.vv(6, 0x5a5a_5a5a);
    case(&mut c, "v_or_b32", &p, &[0x380c_02f9, 0x0000_0a00], &[6], &[0xfff7_0000]);

    let mut p = pr();
    p.vv(0, 0x1234_8001).vv(2, 0x0008_0004).vv(1, 0x5a5a_5a5a).vv(3, 0x5a5a_5a5a);
    case(&mut c, "v_pk_lshlrev_b16", &p, &[0xcc04_4001, 0x1802_0102], &[1], &[0x3400_0010]);
    case(&mut c, "v_pk_lshrrev_b16", &p, &[0xcc05_4003, 0x1802_0102], &[3], &[0x0012_0800]);
    case(&mut c, "v_pk_lshlrev_b16", &p, &[0xcc04_4801, 0x1002_0102], &[1], &[0x2340_0100]);
    case(&mut c, "v_pk_lshrrev_b16", &p, &[0xcc05_5003, 0x0802_0102], &[3], &[0x0080_0123]);
    let mut p = pr();
    p.vv(0, 0x1234_8001).vv(2, 0x0013_0011).vv(1, 0x5a5a_5a5a).vv(3, 0x5a5a_5a5a);
    case(&mut c, "v_pk_lshlrev_b16", &p, &[0xcc04_4001, 0x1802_0102], &[1], &[0x91a0_0002]);
    case(&mut c, "v_pk_lshrrev_b16", &p, &[0xcc05_4003, 0x1802_0102], &[3], &[0x0246_4000]);
    let mut p = pr();
    p.vv(4, 0xbeef_0000).vv(5, 0xaaaa_0001).vv(6, 0xffff);
    case(&mut c, "v_add_nc_u16", &p, &[0xd703_0004, 0x0002_0d05], &[4], &[0xbeef_0000]);
    let mut p = pr();
    p.vv(7, 0x1234_0000).vv(8, 0x3400);
    case(&mut c, "v_sin_f16", &p, &[0x7e0e_c108], &[7], &[0x1234_3c00]);

    let mut p = pr();
    p.vv(0, 0).vv(1, 0x8000_0000).vv(2, 0x7f80_0000).vv(3, 0xff80_0000).vv(4, 0x7fc0_0000).vv(5, 0x4000_0000).vv(6, 0x4040_0000);
    let ops = [0x0e0e_0500u32, 0x0e10_0900, 0x0e12_0701, 0x0e14_0d05]; // v7=v0*v2, v8=v0*v4, v9=v1*v3, v10=v5*v6
    case(&mut c, "v_mul_legacy_f32", &p, &ops, &[7, 8, 9, 10], &[0, 0, 0, 0x40c0_0000]);
    c.run_both("leonida_ps");
}

#[test]
fn leonida_wave64_scalar_masks() {
    let mut c = Cases::new();
    let bfm = [sop2(0x25, 6, 20, 21), 0xbe88_1006, 0xbe89_1406];
    for &(w, o, lo, hi, cnt, ff) in &[(16u32, 8u32, 0x00ff_ff00u32, 0u32, 16u32, 8u32), (8, 40, 0, 0x0000_ff00, 8, 40)] {
        let mut p = pr();
        p.sv(20, w).sv(21, o);
        case(&mut c, "s_bfm_b64", &p, &bfm, &[1008, 1009, 1006, 1007], &[cnt, ff, lo, hi]);
    }
    let head = [
        0xbefe_03ff, 0xffff_0000, // s_mov_b32 exec_lo, 0xffff0000
        0xbeff_03ff, 0x0000_ffff, // s_mov_b32 exec_hi, 0xffff
        0xbe86_047e,              // s_mov_b64 s[6:7], exec
        0xbf82_0000,              // s_branch 0
        0xbe88_1006,              // s_bcnt1_i32_b64 s8, s[6:7]
        0xbe89_1406,              // s_ff1_i32_b64 s9, s[6:7]
        0xbe8a_0f7e,              // s_bcnt1_i32_b32 s10, exec_lo
        0xbe8b_137f,              // s_ff1_i32_b32 s11, exec_hi
        0xbe8c_0f07,              // s_bcnt1_i32_b32 s12, s7
        0xbefe_04c1,              // s_mov_b64 exec, -1
    ];
    case(&mut c, "s_mov_b32", &pr(), &head, &[1008, 1009, 1010, 1011], &[32, 16, 16, 0]);
    case(&mut c, "s_mov_b32", &pr(), &head, &[1012, 1006, 1007], &[16, 0xffff_0000, 0x0000_ffff]);
    let cmp = [
        0xbefe_0380,              // s_mov_b32 exec_lo, 0
        0xbeff_03ff, 0x0000_ffff, // s_mov_b32 exec_hi, 0xffff
        0xbe86_047e,              // s_mov_b64 s[6:7], exec
        0xbf06_8006, 0x8508_8081, // s_cmp_eq_u32 s6, 0; s_cselect_b32 s8, 1, 0
        0xbf07_8006, 0x8509_8081, // s_cmp_lg_u32 s6, 0; s_cselect_b32 s9, 1, 0
        0xbf07_8007, 0x850a_8081, // s_cmp_lg_u32 s7, 0; s_cselect_b32 s10, 1, 0
        0xbf06_807e, 0x850b_8081, // s_cmp_eq_u32 exec_lo, 0; s_cselect_b32 s11, 1, 0
        0xbefe_04c1,              // s_mov_b64 exec, -1
    ];
    case(&mut c, "s_mov_b32", &pr(), &cmp, &[1008, 1009, 1010, 1011], &[1, 0, 1, 1]);
    c.run_both("leonida_wave64_scalar");
}

fn scc_rfl_case(width: u32) -> LaneCase {
    let (thr, mask) = if width >= 32 { (16u32, 0xffff_0000u32) } else { (2, 0xc) };
    let mut b = vec![
        sop1(3, 14, ic(0)),                // s_mov_b32 s14, 0
        vopc(0xc4, ic(thr as i32), LANE), // v_cmp_gt_u32 vcc_lo, thr, lane
        0xbe86_246a,                       // s_and_saveexec_b64 s[6:7], vcc
        sopp(4, 1),                        // s_cbranch_scc0 1
        sop1(3, 14, ic(55)),               // s_mov_b32 s14, 55
        0xbefe_04c1,                       // s_mov_b64 exec, -1
        vop2(0x25, 4, ic(32), LANE),       // v_add_nc_u32 v4, 32, lane
        0xbefe_03ff, mask,                 // s_mov_b32 exec_lo, mask
        0xbeff_0380,                       // s_mov_b32 exec_hi, 0
    ];
    let at = b.len();
    b.push(0x7e1e_0504); // v_readfirstlane_b32 s15, v4
    b.extend_from_slice(&all_exec());
    b.extend_from_slice(&[vop1(1, 0, 14), vop1(1, 1, 15), vop1(1, 2, v(LANE)), vop1(1, 3, ic(0))]);
    let first = 32 + mask.trailing_zeros();
    LaneCase { m: "v_readfirstlane_b32", width, body: b, at, exp: (0..width).map(|l| [55, first, l, 0]).collect() }
}

#[test]
#[ignore = "lane model: SCC from saveexec and v_readfirstlane are per invocation, not per wave (see hlsl.rs module docs)"]
fn wave_scc_readfirstlane_isa() {
    run_lane_cases("leonida_scc_rfl_warp", Dev::Warp, &[scc_rfl_case(4)]);
    if hw_requested() {
        run_lane_cases("leonida_scc_rfl_hw", Dev::Hw, &[scc_rfl_case(32)]);
    }
}

#[test]
fn leonida_nan_and_fract() {
    let mut c = Cases::new();
    let a = [(1, 0xaedb_e6ffu32), (2, 0x0000_8800), (3, 0x7fc1_0001), (4, 0xc020_0000)];
    vc(&mut c, "v_fract_f32", &a, &[0x7e0a_4101], &[5], &[0x3f7f_ffff]); // v_fract_f32 v5, v1
    vc(&mut c, "v_fract_f16", &a, &[0x7e04_bf02], &[2], &[0x0000_3bff]); // v_fract_f16 v2, v2
    vc(&mut c, "v_max_f32", &a, &[0x200c_0903], &[6], &[0xc020_0000]); // v_max_f32 v6, v3, v4
    vc(&mut c, "v_min_f32", &a, &[0x1e0e_0704], &[7], &[0xc020_0000]); // v_min_f32 v7, v4, v3
    let b = [(1, 0x7fc1_0001u32), (2, 0x40e0_0000), (3, 0x40a0_0000), (4, 0x7fc1_0001), (5, 0x4060_0000), (6, 0xc020_0000)];
    vc(&mut c, "v_med3_f32", &b, &[0xd557_0007, 0x040e_0501], &[7], &[0x40a0_0000]); // v_med3_f32 v7, v1, v2, v3
    vc(&mut c, "v_med3_f32", &b, &[0xd557_0008, 0x041a_0b04], &[8], &[0xc020_0000]); // v_med3_f32 v8, v4, v5, v6
    vc(&mut c, "v_min3_f32", &b, &[0xd551_0009, 0x0416_0d01], &[9], &[0xc020_0000]); // v_min3_f32 v9, v1, v6, v5
    vc(&mut c, "v_max3_f32", &b, &[0xd554_000a, 0x0416_0d01], &[10], &[0x4060_0000]); // v_max3_f32 v10, v1, v6, v5
    c.run_both("leonida_nan_and_fract");
}

#[test]
fn leonida_mubuf_bounds() {
    let root = tombox_xshader::meta::RootSig { version: 2, flags: 1, params: vec![rtable(1, 8)], num_static_samplers: 0, static_samplers: Vec::new() };
    let mut pre = pr();
    pre.vv(1, 1020).vv(2, 1024).vv(8, 0x1_0000).vv(4, 0x1234_5678).vv(3, 0xdead_beef).vv(5, 0x5555).vv(6, 0x5555).vv(7, 0x5555).vv(9, 0x5555);
    let mut body = vec![0xf408_0004, 0xfa00_0000]; // s_load_dwordx4 s[0:3], s[8:9], 0
    body.extend_from_slice(&mubuf(0x1c, 0, true, false, false, 1, 4)); // store v4 at 1020 (last dword)
    body.extend_from_slice(&mubuf(0x1c, 0, true, false, false, 2, 3)); // store v3 at 1024 (dropped)
    body.extend_from_slice(&mubuf(0x0c, 0, true, false, false, 1, 5)); // load 1020
    body.extend_from_slice(&mubuf(0x0c, 0, true, false, false, 2, 6)); // load 1024 -> 0
    body.extend_from_slice(&mubuf(0x0c, 4, true, false, false, 1, 7)); // load 1020 + offset:4 -> 0
    body.extend_from_slice(&mubuf(0x0c, 0, true, false, false, 8, 9)); // load 0x10000 -> 0
    body.extend_from_slice(&[vop1(1, 0, v(5)), vop1(1, 1, v(6)), vop1(1, 2, v(7)), vop1(1, 3, v(9))]);
    let mut fails = Vec::new();
    for rt in [false, true] {
        let mut p = Prog::new();
        p.raw(&if rt { super::opaque(&pre.w) } else { pre.w.clone() }).raw(&body);
        match run_res_ex(&p.words(), root.clone(), Some(&BUF), None, sampler(0, 3, 0), rt) {
            Ok(o) => {
                let want_rt = [0x1234_5678, 0, 0, 0];
                let mem = [(255usize, 0x1234_5678u32), (0, BUF[0]), (1, BUF[1])];
                let got: Vec<u32> = mem.iter().map(|&(k, _)| o.buf[k]).collect();
                if o.rt != want_rt || mem.iter().zip(&got).any(|(&(_, w), &g)| w != g) {
                    fails.push(format!("runtime inputs {rt}: rt {:08x?} want {want_rt:08x?}; mem {got:08x?} want {mem:08x?}", o.rt));
                }
            }
            Err(e) => fails.push(format!("runtime inputs {rt}: {}", e.lines().take(3).collect::<Vec<_>>().join(" | "))),
        }
    }
    assert!(fails.is_empty(), "leonida_mubuf_bounds:\n{}", fails.join("\n"));
}
