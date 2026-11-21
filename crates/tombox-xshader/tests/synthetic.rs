//! Translator regression tests on hand-assembled RDNA2 programs.
//!
//! These cover behaviour that was found wrong while running the title, without needing a
//! game shader as a fixture: each program is a few instructions written out as raw dwords,
//! wrapped in a `ShaderExtract` with a made-up signature.

use tombox_xshader::hlsl::translate;
use tombox_xshader::meta::{RootParam, RootParamKind, RootRange, RootSig, SigElement};
use tombox_xshader::{ShaderExtract, Stage};

const S_ENDPGM: u32 = 0xbf81_0000;
const S_NOP: u32 = 0xbf80_0000;
/// `v_cvt_pkrtz_f16_f32 v0, v2, v2` / `v1, v2, v2`: the default colour-export packing.
const PACK_V0_V2_V2: u32 = 0x5e00_0502;
const PACK_V1_V2_V2: u32 = 0x5e02_0502;
/// `exp mrt0 v0, v1` compressed, done, valid-mask.
const EXP_MRT0_COMPR: [u32; 2] = [0xf800_1c0f, 0x0000_0100];

fn sig(name: &str, sysval: u32, comp_type: u32, register: u32, mask: u8) -> SigElement {
    SigElement { name: name.into(), index: 0, sysval, comp_type, register, mask, rw_mask: 0 }
}

fn table(range_type: u32, num: u32, visibility: u32) -> RootParam {
    RootParam {
        visibility,
        kind: RootParamKind::Table(vec![RootRange { range_type, num, base_register: 0, space: 0, offset_in_table: u32::MAX }]),
    }
}

fn shader(stage: Stage, words: &[u32], input_sig: Vec<SigElement>, output_sig: Vec<SigElement>, root: Option<RootSig>) -> ShaderExtract {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ShaderExtract {
        stage,
        shader_model: (6, 0),
        bitcode_stripped: true,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: isa.clone(),
        isa,
        isa_offset_in_fssm: 0,
        isa_preamble: [0; 4],
        has_input_sig: true,
        has_output_sig: true,
        has_psv0: false,
        has_root_sig: root.is_some(),
        input_sig,
        output_sig,
        resources: Vec::new(),
        gs_info: None,
        root_sig: root,
    }
}

/// A pixel shader that writes the integer 5 to a `uint4` render target through the default
/// pack-and-export tail. Converting the integer through float-to-half (what the tail does
/// literally) would give 0; the integer itself must be exported.
#[test]
fn integer_colour_export_keeps_the_integer() {
    let words = [
        0x7e04_0285, // v_mov_b32 v2, 5
        S_NOP,
        S_NOP,
        PACK_V0_V2_V2,
        PACK_V1_V2_V2,
        EXP_MRT0_COMPR[0],
        EXP_MRT0_COMPR[1],
        S_ENDPGM,
    ];
    let ex = shader(Stage::Pixel, &words, vec![], vec![sig("SV_Target", 64, 1, 0, 0xf)], None);
    let t = translate(&ex).expect("translate");
    assert!(t.hlsl.contains("uint4 o0 : SV_Target0"), "{}", t.hlsl);
    assert!(t.hlsl.contains("o.o0 = (uint4)(uint4(tb_pk0a, tb_pk0b, tb_pk1a, tb_pk1b).xyzw);"), "{}", t.hlsl);
    assert!(t.hlsl.contains("tb_pk0a = v2;"), "{}", t.hlsl);
    let body = t.hlsl.split("#define TB_RTFMT7 0u\n#endif").nth(1).unwrap_or(&t.hlsl);
    assert!(!body.contains("f16tof32"), "integer export must not go through half floats:\n{}", t.hlsl);
}

/// The same tail feeding a float render target exports the unpacked floats (the target's own
/// format conversion applies, not a fixed half-float one).
#[test]
fn float_colour_export_is_not_squeezed_through_half() {
    let words = [
        0x7e04_02f2, // v_mov_b32 v2, 1.0
        S_NOP,
        S_NOP,
        PACK_V0_V2_V2,
        PACK_V1_V2_V2,
        EXP_MRT0_COMPR[0],
        EXP_MRT0_COMPR[1],
        S_ENDPGM,
    ];
    let ex = shader(Stage::Pixel, &words, vec![], vec![sig("SV_Target", 64, 3, 0, 0xf)], None);
    let t = translate(&ex).expect("translate");
    assert!(
        t.hlsl.contains("o.o0 = (float4)(float4(tb_rtfmt(asfloat(tb_pk0a), TB_RTFMT0, 0u), tb_rtfmt(asfloat(tb_pk0b), TB_RTFMT0, 1u), tb_rtfmt(asfloat(tb_pk1a), TB_RTFMT0, 2u), tb_rtfmt(asfloat(tb_pk1b), TB_RTFMT0, 3u)).xyzw);"),
        "{}",
        t.hlsl
    );
}

#[test]
fn float_colour_export_follows_the_target_format() {
    let words = [
        0x7e04_02ff, 0x4974_2400,
        S_NOP,
        S_NOP,
        PACK_V0_V2_V2,
        PACK_V1_V2_V2,
        EXP_MRT0_COMPR[0],
        EXP_MRT0_COMPR[1],
        S_ENDPGM,
    ];
    let mut out = sig("SV_Target", 64, 3, 0, 0xf);
    out.index = 0;
    let ex = shader(Stage::Pixel, &words, vec![], vec![out], None);
    let t = translate(&ex).expect("translate");
    assert!(t.hlsl.contains("#ifndef TB_RTFMT0\n#define TB_RTFMT0 0u\n#endif"), "{}", t.hlsl);
    assert!(t.hlsl.contains("uint tb_f16rtz(float x)"), "{}", t.hlsl);
    assert!(t.hlsl.contains("float tb_rtfmt(float x, uint k, uint c)"), "{}", t.hlsl);
    assert!(t.hlsl.contains("tb_rtfmt(asfloat(tb_pk0a), TB_RTFMT0, 0u)"), "{}", t.hlsl);
    if tombox_xshader::dxc::available() {
        let profile = tombox_xshader::dxc::profile_for(t.profile);
        for defs in ["", "#define TB_RTFMT0 1u\n", "#define TB_RTFMT0 2u\n"] {
            let src = format!("{defs}{}", t.hlsl);
            tombox_xshader::dxc::compile(&src, &profile).unwrap_or_else(|e| panic!("dxc ({defs:?}): {e}\n{src}"));
        }
    }
}

#[test]
fn pkrtz_rounds_toward_zero() {
    let words = [
        0x7e04_02f2,
        0x5e06_0502,
        0x7e00_0303,
        0x7e02_0303,
        EXP_MRT0_COMPR[0],
        EXP_MRT0_COMPR[1],
        S_ENDPGM,
    ];
    let ex = shader(Stage::Pixel, &words, vec![], vec![sig("SV_Target", 64, 3, 0, 0xf)], None);
    let t = translate(&ex).expect("translate");
    assert!(t.hlsl.contains("(tb_f16rtz(asfloat(v2)) | (tb_f16rtz(asfloat(v2)) << 16))"), "{}", t.hlsl);
}

/// A packed export whose operands do not come from the pack instructions directly before it
/// keeps the literal behaviour (unpack the halves).
#[test]
fn packed_export_without_the_tail_is_left_alone() {
    let words = [
        0x7e00_0280, // v_mov_b32 v0, 0
        0x7e02_0280, // v_mov_b32 v1, 0
        EXP_MRT0_COMPR[0],
        EXP_MRT0_COMPR[1],
        S_ENDPGM,
    ];
    let ex = shader(Stage::Pixel, &words, vec![], vec![sig("SV_Target", 64, 3, 0, 0xf)], None);
    let t = translate(&ex).expect("translate");
    assert!(t.hlsl.contains("f16tof32"), "{}", t.hlsl);
    assert!(!t.hlsl.contains("tb_pk"), "{}", t.hlsl);
    // ...and an integer element cannot be produced from it: that has to fail loudly.
    let ex = shader(Stage::Pixel, &words, vec![], vec![sig("SV_Target", 64, 1, 0, 0xf)], None);
    assert!(translate(&ex).is_err());
}

/// A vertex shader exporting an integer parameter (the primitive id handed to the pixel
/// shader) must pass the register bits through, not convert them as if they were a float.
#[test]
fn integer_parameter_export_keeps_the_integer() {
    let words = [
        0x7e10_0287, // v_mov_b32 v8, 7
        0x7e02_02f2, // v_mov_b32 v1, 1.0
        0xf800_08cf, // exp pos0 v1, v1, v1, v1 (done)
        0x0101_0101,
        0xf800_020f, // exp param0 v8, v8, v8, v8
        0x0808_0808,
        S_ENDPGM,
    ];
    let out = vec![sig("PRIMITIVE_ID", 0, 1, 0, 0x1), sig("SV_Position", 1, 3, 1, 0xf)];
    let ex = shader(Stage::Vertex, &words, vec![], out, None);
    let t = translate(&ex).expect("translate");
    assert!(t.hlsl.contains("o.o0 = (uint)(uint4(v8, v8, v8, v8).x);"), "{}", t.hlsl);
}

/// Texel type of a texture read with texel fetches: integer when the loaded value is used as
/// an integer, float when it is used as a float. (The lighting-channel mask is an R16_UINT
/// texture; declared as float its bits never matched the light's channel mask.)
#[test]
fn texel_type_follows_the_use_of_the_loaded_value() {
    let root = || Some(RootSig { version: 2, flags: 1, params: vec![table(0, 64, 5), table(2, 16, 5)], num_static_samplers: 0 });
    let program = |use_op: u32| {
        vec![
            0xf40c_0004, // s_load_dwordx8 s0, s8 [off=0]: texture descriptor t0
            0xfa00_0000,
            0x7e00_0280, // v_mov_b32 v0, 0
            0x7e02_0280, // v_mov_b32 v1, 0
            0xf000_1108, // image_load v0 <- 2D texture, coords v0..
            0x0000_0000,
            use_op,
            S_NOP,
            S_NOP,
            PACK_V0_V2_V2,
            PACK_V1_V2_V2,
            EXP_MRT0_COMPR[0],
            EXP_MRT0_COMPR[1],
            S_ENDPGM,
        ]
    };
    // v_and_b32 v2, 1, v0
    let ex = shader(Stage::Pixel, &program(0x3604_0081), vec![], vec![sig("SV_Target", 64, 3, 0, 0xf)], root());
    let t = translate(&ex).expect("translate (integer use)");
    assert!(t.hlsl.contains("Texture2D<uint4> tex0 : register(t0);"), "{}", t.hlsl);
    assert!(t.hlsl.contains("uint4 t0 = tex0.Load("), "{}", t.hlsl);
    // v_add_f32 v2, v0, v0
    let ex = shader(Stage::Pixel, &program(0x0604_0100), vec![], vec![sig("SV_Target", 64, 3, 0, 0xf)], root());
    let t = translate(&ex).expect("translate (float use)");
    assert!(t.hlsl.contains("Texture2D<float4> tex0 : register(t0);"), "{}", t.hlsl);
    assert!(t.hlsl.contains("float4 t0 = tex0.Load("), "{}", t.hlsl);
}

/// The same shader must always translate to the same HLSL: the runtime's compiled-shader
/// cache is keyed by the HLSL text. The loop-flag declarations came from iterating a
/// `HashMap`, and every `HashMap` gets its own random seed, so repeated translations in one
/// process differ when a shader has several loops.
#[test]
fn loops_translate_deterministically() {
    let mut words = Vec::new();
    for s in 0..6u32 {
        words.extend([
            0xbe80_0384 | s << 16,     // s_mov_b32 s<s>, 4
            0x8080_8100 | s << 16 | s, // s_sub_u32 s<s>, s<s>, 1
            0xbf07_8000 | s,           // s_cmp_lg_u32 s<s>, 0
            0xbf85_fffd,               // s_cbranch_scc1 back to the s_sub_u32
        ]);
    }
    words.push(S_ENDPGM);
    let mut ex = shader(Stage::Compute, &words, vec![], vec![], None);
    ex.isa_preamble = [0, 1, 1, 1];
    (ex.has_input_sig, ex.has_output_sig) = (false, false);
    let first = translate(&ex).expect("translate").hlsl;
    assert_eq!(first.matches("do {").count(), 6, "{first}");
    for _ in 0..16 {
        assert!(translate(&ex).expect("translate").hlsl == first, "translation differs between runs");
    }
}
