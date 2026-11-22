
use super::super::*;

const D: u32 = 0xaaaa_5555;

fn t(c: &mut Cases, m: &'static str, ins: &[(u32, u32)], op: &[u32], out: u32, exp: u32) {
    vc(c, m, ins, op, &[out], &[exp]);
}

#[test]
fn hw_f16_unary() {
    let mut c = Cases::new();
    let a = [(0, 0xbe00_4100u32), (1, 0x4300_ba00), (2, D), (3, D), (4, D), (5, D)];
    t(&mut c, "v_rndne_f16", &a, &[0x7e04_bd00], 2, 0xaaaa_4000); // v_rndne_f16 v2, v0: 2.5 -> 2.0 (ties to even)
    t(&mut c, "v_floor_f16", &a, &[0x7e06_b701], 3, 0xaaaa_bc00); // v_floor_f16 v3, v1: -1.0
    t(&mut c, "v_ceil_f16", &a, &[0xd5dc_0004, 0x2000_0100], 4, 0xaaaa_c000); // v_ceil_f16_e64 v4, -v0: -2.0
    t(&mut c, "v_trunc_f16", &a, &[0x7e0a_bb01], 5, 0xaaaa_8000); // v_trunc_f16 v5, v1: -0.0
    let b = [(0, 0x3400_4400u32), (1, 0x3e00_c180), (2, D), (3, D), (4, D), (5, D)];
    t(&mut c, "v_fract_f16", &b, &[0x7e04_bf01], 2, 0xaaaa_3400); // v_fract_f16 v2, v1: 0.25
    t(&mut c, "v_sqrt_f16", &b, &[0x7e06_ab00], 3, 0xaaaa_4000); // v_sqrt_f16 v3, v0: 2.0
    t(&mut c, "v_rcp_f16", &b, &[0xd5d4_0004, 0x2000_0100], 4, 0xaaaa_b400); // v_rcp_f16_e64 v4, -v0: -0.25
    t(&mut c, "v_rsq_f16", &b, &[0x7e0a_ad00], 5, 0xaaaa_3800); // v_rsq_f16 v5, v0: 0.5
    c.run_both("hw_f16_unary");
}

#[test]
fn hw_f16_binary() {
    let mut c = Cases::new();
    let a = [(0, 0x4400_3e00u32), (1, 0x4000_3800), (2, D), (3, D), (4, D), (5, 0x4200_3c00)];
    t(&mut c, "v_add_f16", &a, &[0x6404_0300], 2, 0xaaaa_4000); // v_add_f16 v2, v0, v1: 2.0
    t(&mut c, "v_sub_f16", &a, &[0xd533_0203, 0x2002_0300], 3, 0xaaaa_c000); // v_sub_f16_e64 v3, -v0, |v1|: -2.0
    t(&mut c, "v_mul_f16", &a, &[0x6a08_0300], 4, 0xaaaa_3a00); // v_mul_f16 v4, v0, v1: 0.75
    t(&mut c, "v_fmac_f16", &a, &[0x6c0a_0300], 5, 0x4200_3f00); // v_fmac_f16 v5, v0, v1: 1.75
    t(&mut c, "v_max_f16", &a, &[0x7204_0300], 2, 0xaaaa_3e00); // v_max_f16 v2, v0, v1: 1.5
    t(&mut c, "v_min_f16", &a, &[0xd53a_0003, 0x4002_0300], 3, 0xaaaa_b800); // v_min_f16_e64 v3, v0, -v1: -0.5
    t(&mut c, "v_add_f16", &a, &[0xd532_8004, 0x0002_0300], 4, 0xaaaa_3c00); // v_add_f16_e64 v4, v0, v1 clamp: 1.0
    t(&mut c, "v_fmac_f16", &a, &[0xd536_0205, 0x2002_0300], 5, 0x4200_3400); // v_fmac_f16_e64 v5, -v0, |v1|: 0.25
    c.run_both("hw_f16_binary");
}

#[test]
fn hw_f16_inline_constants() {
    let mut c = Cases::new();
    let a = [(0, 0x4200_3400u32), (1, 0x3400_4400), (2, D), (3, D), (4, D), (5, D)];
    t(&mut c, "v_add_f16", &a, &[0x6404_00f2], 2, 0xaaaa_3d00); // v_add_f16 v2, 1.0, v0: 1.25
    t(&mut c, "v_med3_f16", &a, &[0xd757_4803, 0x03c1_ef00], 3, 0x3800_5555);
    t(&mut c, "v_fma_f16", &a, &[0xd74b_2004, 0x03c1_e900], 4, 0xaaaa_3800);
    t(&mut c, "v_cvt_f32_f16", &a, &[0x7e0a_16f0], 5, 0x3f00_0000); // v_cvt_f32_f16 v5, 0.5
    t(&mut c, "v_pk_add_f16", &a, &[0xcc0f_4002, 0x1802_00f2], 2, 0x4200_3d00);
    t(&mut c, "v_pk_add_f16", &a, &[0xcc0f_5803, 0x0002_00f2], 3, 0x3d00_4200);
    t(&mut c, "v_cos_f16", &a, &[0x7e08_c2f0], 4, 0xaaaa_bc00); // v_cos_f16 v4, 0.5 (turns): -1.0
    t(&mut c, "v_cmp_lt_f16", &a, &[0xd4c9_0001, 0x0002_00f0, 0xd501_0005, 0x0005_0e83], 5, 3);
    t(&mut c, "v_pack_b32_f16", &a, &[0xd711_0002, 0x0002_00f2], 2, 0x3400_3c00); // v_pack_b32_f16 v2, 1.0, v0
    t(&mut c, "v_pack_b32_f16", &a, &[0xd711_0003, 0x0001_e100], 3, 0x3800_3400); // v_pack_b32_f16 v3, v0, 0.5
    t(&mut c, "v_pack_b32_f16", &a, &[0xd711_0804, 0x0002_00f4], 4, 0x3400_0000);
    t(&mut c, "v_pack_b32_f16", &a, &[0xd711_0805, 0x0002_0300], 5, 0x4400_4200);
    t(&mut c, "v_fma_f16", &[(0, 0x6400), (1, 0xd918), (2, D)], &[0xd74b_0002, 0x0405_f100], 2, 0xaaaa_0000);
    c.run_both("hw_f16_inline_constants");
}

#[test]
fn hw_f16_med3_opsel() {
    let mut c = Cases::new();
    let a = [(0, 0x4100_3c00u32), (1, 0x4800_4000), (2, 0x3e00_4200), (3, D), (4, D), (5, D), (6, D)];
    t(&mut c, "v_med3_f16", &a, &[0xd757_0803, 0x040a_0300], 3, 0xaaaa_4100); // op_sel:[1,0,0,0]: med(2.5, 2, 3)
    t(&mut c, "v_med3_f16", &a, &[0xd757_1004, 0x040a_0300], 4, 0xaaaa_4200); // op_sel:[0,1,0,0]: med(1, 8, 3)
    t(&mut c, "v_med3_f16", &a, &[0xd757_2005, 0x040a_0300], 5, 0xaaaa_3e00); // op_sel:[0,0,1,0]: med(1, 2, 1.5)
    t(&mut c, "v_med3_f16", &a, &[0xd757_4006, 0x040a_0300], 6, 0x4000_5555); // op_sel:[0,0,0,1]: med(1, 2, 3) high
    let b = [(0, 0xc100_3800u32), (1, 0xc800_4000), (2, 0x3a00_3c00), (3, D), (4, D), (5, 0xc100_3800), (6, D)];
    t(&mut c, "v_med3_f16", &b, &[0xd757_5a03, 0x240a_0300], 3, 0x4100_5555);
    t(&mut c, "v_med3_f16", &b, &[0xd757_fb04, 0x040a_0300], 4, 0x3c00_5555);
    t(&mut c, "v_med3_f16", &b, &[0xd757_6c05, 0x2406_0305], 5, 0x4100_3800);
    t(&mut c, "v_med3_f16", &b, &[0xd757_3406, 0x840a_0300], 6, 0xaaaa_ba00);
    c.run_both("hw_f16_med3_opsel");
}

#[test]
fn hw_nan_and_fract() {
    let mut c = Cases::new();
    let a = [(1, 0xaedb_e6ffu32), (2, 0x0000_8800), (3, 0x7fc1_0001), (4, 0xc020_0000)];
    t(&mut c, "v_fract_f32", &a, &[0x7e0a_4101], 5, 0x3f7f_ffff); // v_fract_f32 v5, v1
    t(&mut c, "v_fract_f16", &a, &[0x7e04_bf02], 2, 0x0000_3bff); // v_fract_f16 v2, v2
    t(&mut c, "v_max_f32", &a, &[0x200c_0903], 6, 0xc020_0000); // v_max_f32 v6, v3, v4
    t(&mut c, "v_min_f32", &a, &[0x1e0e_0704], 7, 0xc020_0000); // v_min_f32 v7, v4, v3
    let b = [(1, 0x7fc1_0001u32), (2, 0x40e0_0000), (3, 0x40a0_0000), (4, 0x7fc1_0001), (5, 0x4060_0000), (6, 0xc020_0000)];
    t(&mut c, "v_med3_f32", &b, &[0xd557_0007, 0x040e_0501], 7, 0x40a0_0000); // v_med3_f32 v7, v1, v2, v3
    t(&mut c, "v_med3_f32", &b, &[0xd557_0008, 0x041a_0b04], 8, 0xc020_0000); // v_med3_f32 v8, v4, v5, v6
    t(&mut c, "v_min3_f32", &b, &[0xd551_0009, 0x0416_0d01], 9, 0xc020_0000); // v_min3_f32 v9, v1, v6, v5
    t(&mut c, "v_max3_f32", &b, &[0xd554_000a, 0x0416_0d01], 10, 0x4060_0000); // v_max3_f32 v10, v1, v6, v5
    c.run_both("hw_nan_and_fract");
}

#[test]
fn hw_fma_single_rounding() {
    let mut c = Cases::new();
    let a = [(0, 0x3f80_0800u32), (3, 0x3f80_1000), (10, 0xbf80_1000), (5, 0x3f80_0001), (6, 0x3f7f_fffe), (13, 0xbf80_0002), (12, 0xbf80_1000), (15, 0x6ce1), (16, 0x4200), (17, 0x8004), (18, 0)];
    t(&mut c, "v_fma_f32", &a, &[0xd54b_0004, 0x840e_0100], 4, 0x3380_0000); // v_fma_f32 v4, v0, v0, -v3
    t(&mut c, "v_fma_f32", &a, &[0xd54b_0007, 0x03ce_0d05], 7, 0xa880_0000); // v_fma_f32 v7, v5, v6, -1.0: -2^-46
    t(&mut c, "v_fma_f32", &a, &[0xd54b_0008, 0x03ce_0100], 8, 0x3a00_0400); // v_fma_f32 v8, v0, v0, -1.0
    t(&mut c, "v_fmaak_f32", &a, &[0x5a12_0100, 0xbf80_1000], 9, 0x3380_0000); // v_fmaak_f32 v9, v0, v0, -p
    t(&mut c, "v_fmamk_f32", &a, &[0x5816_1500, 0x3f80_0800], 11, 0x3380_0000); // v_fmamk_f32 v11, v0, a, v10
    t(&mut c, "v_fmac_f32", &a, &[0x5618_0100], 12, 0x3380_0000); // v_fmac_f32 v12, v0, v0
    t(&mut c, "v_fma_f32", &a, &[0xd54b_000e, 0x0436_0b05], 14, 0x2880_0000); // v_fma_f32 v14, v5, v5, v13: 2^-46
    t(&mut c, "v_fma_f16", &a, &[0xd74b_0012, 0x0446_210f], 18, 0x0000_7351);
    c.run_both("hw_fma_single_rounding");
}
