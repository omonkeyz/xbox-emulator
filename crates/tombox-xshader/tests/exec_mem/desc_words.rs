use super::mimg::mimg;
use super::*;

const S_ADD_U32: u32 = 0x00;
const S_AND_B32: u32 = 0x0e;
const S_OR_B32: u32 = 0x10;
const S_LSHL_B32: u32 = 0x1e;
const S_LSHR_B32: u32 = 0x20;
const S_BFE_U32: u32 = 0x27;

fn zero(_m: u32, _l: u32, _x: u32, _y: u32) -> [u32; 4] {
    [0; 4]
}

const IMG_2D: Img = Img { w: 38, h: 21, layers: 1, array: false, mips: 3, fmt: R32G32B32A32_FLOAT, texel: zero };
const IMG_ARR: Img = Img { w: 8, h: 8, layers: 3, array: true, mips: 2, fmt: R32G32B32A32_FLOAT, texel: zero };
const IMG_UAV: Img = Img { w: 8, h: 8, layers: 1, array: false, mips: 4, fmt: R32G32B32A32_FLOAT, texel: zero };

fn bfe(p: &mut Prog, dst: u32, src: u32, lo: u32, width: u32) {
    p.raw(&[sop2(S_BFE_U32, dst, src, LIT), lo | (width << 16)]);
}

fn size_fields(p: &mut Prog, d: u32) {
    bfe(p, 20, d + 2, 0, 12);
    p.raw(&[sop2(S_LSHL_B32, 20, 20, ic(2))]);
    p.raw(&[sop2(S_LSHR_B32, 21, d + 1, ic(30))]);
    p.raw(&[sop2(S_OR_B32, 20, 20, 21), sop2(S_ADD_U32, 20, 20, ic(1))]);
    bfe(p, 22, d + 2, 14, 14);
    p.raw(&[sop2(S_ADD_U32, 22, 22, ic(1))]);
}

fn level_fields(p: &mut Prog, d: u32) {
    bfe(p, 23, d + 3, 16, 4);
    p.raw(&[sop2(S_LSHR_B32, 24, d + 3, ic(28))]);
    bfe(p, 25, d + 3, 12, 4);
    bfe(p, 26, d + 4, 0, 13);
    bfe(p, 27, d + 4, 16, 13);
    bfe(p, 28, d + 5, 4, 4);
}

fn program_dim(uav: bool, dim: u32, regs: [u32; 4], fields: &dyn Fn(&mut Prog, u32)) -> Vec<u32> {
    let d = if uav { 16 } else { 0 };
    let mut p = Prog::new();
    if uav {
        p.raw(&smem(3, 16, P_UAV, 0, 0x7d));
        p.vv(1, 0).vv(2, 0).vv(3, 0);
        p.raw(&mimg(0x00, 0x1, dim, 1, 8, 16, 0));
    } else {
        p.raw(&smem(3, 0, P_SRV, 0, 0x7d));
        p.vv(1, 0);
        p.raw(&mimg(0x0e, 0x1, dim, 1, 8, 0, 12));
    }
    fields(&mut p, d);
    for (k, r) in regs.iter().enumerate() {
        p.s2v(k as u32, *r);
    }
    p.words()
}

#[test]
fn image_descriptor_words_read_by_salu_come_from_the_view() {
    let size = |p: &mut Prog, d: u32| size_fields(p, d);
    let lvl = |p: &mut Prog, d: u32| level_fields(p, d);
    let cases: Vec<(&str, Vec<u32>, Env, [u32; 4])> = vec![
        ("2d size", program_dim(false, 1, [20, 22, 21, 20], &size), Env { tex: Some(&IMG_2D), ..Default::default() }, [38, 21, 1, 38]),
        ("2d levels", program_dim(false, 1, [23, 24, 25, 28], &lvl), Env { tex: Some(&IMG_2D), ..Default::default() }, [2, 9, 0, 2]),
        ("2d depth", program_dim(false, 1, [26, 27, 23, 24], &lvl), Env { tex: Some(&IMG_2D), ..Default::default() }, [0, 0, 2, 9]),
        ("array size", program_dim(false, 5, [20, 22, 21, 20], &size), Env { tex: Some(&IMG_ARR), ..Default::default() }, [8, 8, 3, 8]),
        ("array levels", program_dim(false, 5, [23, 24, 26, 28], &lvl), Env { tex: Some(&IMG_ARR), ..Default::default() }, [1, 13, 2, 1]),
        ("uav size", program_dim(true, 1, [20, 22, 21, 20], &size), Env { uav: Some(&IMG_UAV), ..Default::default() }, [8, 8, 3, 8]),
        ("uav levels", program_dim(true, 1, [23, 24, 25, 28], &lvl), Env { uav: Some(&IMG_UAV), ..Default::default() }, [0, 9, 0, 0]),
    ];
    let mut fails = Vec::new();
    for (i, (name, words, env, want)) in cases.iter().enumerate() {
        let t = translate(&extract(words, false));
        if let Err(e) = &t {
            fails.push(format!("#{i} {name}: translate {e}"));
            continue;
        }
        match exec(words, env) {
            Ok(o) if o.rt == *want => {}
            Ok(o) => fails.push(format!("#{i} {name}: rt {:?} want {want:?}\n{}", o.rt, t.map(|t| t.hlsl).unwrap_or_default())),
            Err(e) => fails.push(format!("#{i} {name}: {}", e.lines().take(4).collect::<Vec<_>>().join(" | "))),
        }
    }
    println!("desc_words: {} cases", cases.len());
    assert!(fails.is_empty(), "{} failing\n{}", fails.len(), fails.join("\n\n"));
}

#[test]
fn image_descriptor_fields_without_a_pc_source_are_refused() {
    let cases: Vec<(&str, Box<dyn Fn(&mut Prog, u32)>, &str)> = vec![
        ("dst_sel", Box::new(|p: &mut Prog, d: u32| bfe(p, 20, d + 3, 0, 3)), "DST_SEL"),
        ("base address", Box::new(|p: &mut Prog, d: u32| {
            p.raw(&[sop2(S_ADD_U32, 20, d, ic(1))]);
        }), "BASE_ADDRESS"),
        ("format", Box::new(|p: &mut Prog, d: u32| bfe(p, 20, d + 1, 20, 9)), "FORMAT"),
        ("sw_mode", Box::new(|p: &mut Prog, d: u32| {
            p.raw(&[sop2(S_LSHR_B32, 20, d + 3, ic(20))]);
        }), "SW_MODE"),
    ];
    for (name, f, needle) in cases {
        let words = program_dim(false, 1, [20, 20, 20, 20], &*f);
        match translate(&extract(&words, false)) {
            Ok(t) => panic!("{name}: expected a refusal naming {needle}, got:\n{}", t.hlsl),
            Err(e) => assert!(e.contains(needle) && e.contains("PC view does not carry"), "{name}: {e}"),
        }
    }
    let edit = program_dim(false, 1, [3, 3, 3, 3], &|p: &mut Prog, d: u32| {
        p.raw(&[sop2(S_AND_B32, d + 3, d + 3, LIT), 0xffff_f000]);
    });
    assert!(translate(&extract(&edit, false)).is_ok(), "an in-place descriptor edit must still translate");
}
