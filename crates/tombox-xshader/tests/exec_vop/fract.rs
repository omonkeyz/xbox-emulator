use super::*;

const QNAN32: u32 = 0x7fc0_0000;
const BELOW_ONE32: u32 = 0x3f7f_ffff;
const QNAN16: u32 = 0x7e00;
const BELOW_ONE16: u32 = 0x3bff;
const QNAN64: u64 = 0x7ff8_0000_0000_0000;
const BELOW_ONE64: u64 = 0x3fef_ffff_ffff_ffff;

fn d1b(c: &mut Cases, m: &'static str, op: u32, x: u64, out: u64) {
    vc(c, m, &[(2, x as u32), (3, (x >> 32) as u32)], &[vop1(op, 10, v(2))], &[10, 11], &[out as u32, (out >> 32) as u32]);
}

#[test]
fn vop1_fract_special_values() {
    let mut c = Cases::new();
    for &(a, exp) in &[
        (0x7f80_0000u32, QNAN32),
        (0xff80_0000, QNAN32),
        (0x7fc0_0000, QNAN32),
        (0xffc0_0001, QNAN32),
        (0x7f80_0001, QNAN32),
        (0x8000_0000, 0x0000_0000),
        (0x0000_0000, 0x0000_0000),
        (BELOW_ONE32, BELOW_ONE32),
        (0x3f7f_fffe, 0x3f7f_fffe),
        (0xbf80_0000, 0x0000_0000),
        (0xb380_0000, BELOW_ONE32),
        (0x8000_0001, BELOW_ONE32),
        (0xc049_0fdb, 0x3f5b_c094),
    ] {
        v1c(&mut c, "v_fract_f32", 0x20, a, exp);
    }
    for &(h, exp) in &[
        (0x7c00u32, 0xfe00),
        (0xfc00, 0xfe00),
        (0x7e00, 0x7e00),
        (0xfe01, 0xfe01),
        (0x7c01, 0x7c01),
        (0x8000, 0x0000),
        (0x0000, 0x0000),
        (BELOW_ONE16, BELOW_ONE16),
        (0x3bfe, 0x3bfe),
        (0xbc00, 0x0000),
        (0x8001, BELOW_ONE16),
        (0x9400, 0x3bfe),
        (0x3e00, 0x3800),
    ] {
        v16(&mut c, "v_fract_f16", 0x5f, h, exp);
    }
    for &(x, exp) in &[
        (0x7ff0_0000_0000_0000u64, QNAN64),
        (0xfff0_0000_0000_0000, QNAN64),
        (0x7ff8_0000_0000_0000, QNAN64),
        (0xfff8_0000_0000_0001, QNAN64),
        (0x8000_0000_0000_0000, 0),
        (0, 0),
        (BELOW_ONE64, BELOW_ONE64),
        (0x3fef_ffff_ffff_fffe, 0x3fef_ffff_ffff_fffe),
        (0xbff0_0000_0000_0000, 0),
        (0x8000_0000_0000_0001, BELOW_ONE64),
        (0xbe70_0000_0000_0000, 0x3fef_ffff_e000_0000),
        (0x4004_0000_0000_0000, 0x3fe0_0000_0000_0000),
    ] {
        d1b(&mut c, "v_fract_f64", 0x3e, x, exp);
    }
    c.run("vop1_fract_special_values");
    let mut rt = Cases::new();
    for k in &c.v {
        rt.v.push(Case { m: k.m, pre: opaque(&k.pre), op: k.op.clone(), post: k.post.clone(), exp: k.exp.clone() });
    }
    rt.run("vop1_fract_special_values_runtime_inputs");
}
