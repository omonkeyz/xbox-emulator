//! Decoder conformance against LLVM.
//!
//! `data/llvm_gfx10/` holds every GFX10 instruction encoding that LLVM's AMDGPU assembler
//! and disassembler tests check, with the assembly LLVM prints for it. LLVM implements the
//! same public ISA independently, so it is an oracle for the decoder that needs no title
//! dump, no DXC and no GPU: these tests run on any host. They check instruction lengths
//! and that the translator returns for every instruction. Opcode names are not checked
//! here.
//!
//! Regenerate the vectors with `tools/dev/llvm_vectors.py`. They are split into three files
//! because `tools/publish_source.py` skips files over 2 MB.

use tombox_xshader::hlsl::translate;
use tombox_xshader::meta::SigElement;
use tombox_xshader::rdna2::disassemble;
use tombox_xshader::{ShaderExtract, Stage};

const VECTORS: [&str; 3] = [
    include_str!("data/llvm_gfx10/scalar_mem.txt"),
    include_str!("data/llvm_gfx10/valu.txt"),
    include_str!("data/llvm_gfx10/vcmp.txt"),
];

const S_ENDPGM: u32 = 0xbf81_0000;
const EXP_MRT0: [u32; 2] = [0xf800_180f, 0x0302_0100];

struct Vector {
    words: Vec<u32>,
    asm: &'static str,
}

impl Vector {
    fn isa(&self) -> Vec<u8> {
        self.words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    fn hex(&self) -> String {
        self.words.iter().map(|w| format!("{w:08x}")).collect::<Vec<_>>().join(" ")
    }
}

fn vectors() -> Vec<Vector> {
    let v: Vec<Vector> = VECTORS
        .iter()
        .flat_map(|file| file.lines())
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let (hex, asm) = l.split_once(" | ").unwrap_or_else(|| panic!("bad vector line: {l}"));
            let words = hex.split(' ').map(|h| u32::from_str_radix(h, 16).unwrap_or_else(|_| panic!("bad dword in: {l}"))).collect();
            Vector { words, asm }
        })
        .collect();
    assert!(v.len() > 40_000, "only {} vectors: is a data file truncated?", v.len());
    v
}

/// Fail with the first few offenders and the total, so one broken opcode does not print
/// thousands of lines.
fn report(what: &str, total: usize, bad: &[String]) {
    assert!(bad.is_empty(), "{what}: {} of {total} vectors\n{}", bad.len(), bad.iter().take(25).cloned().collect::<Vec<_>>().join("\n"));
}

fn shader(stage: Stage, body: &[u32]) -> ShaderExtract {
    let mut words = body.to_vec();
    let mut output_sig = Vec::new();
    if stage == Stage::Pixel {
        words.extend_from_slice(&EXP_MRT0);
        output_sig.push(SigElement { name: "SV_Target".into(), index: 0, sysval: 64, comp_type: 3, register: 0, mask: 0xf, rw_mask: 0 });
    }
    words.push(S_ENDPGM);
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
        has_root_sig: false,
        input_sig: vec![],
        output_sig,
        resources: Vec::new(),
        gs_info: None,
        root_sig: None,
    }
}

/// Instruction length is what keeps a whole program in step, so it is checked for every
/// vector: literals, SDWA and DPP modifier dwords, NSA address dwords.
#[test]
fn each_vector_is_one_instruction_of_the_encoded_length() {
    let all = vectors();
    let mut bad = Vec::new();
    for v in &all {
        let insns = disassemble(&v.isa());
        if insns.len() != 1 || insns[0].dwords != v.words.len() {
            let got: Vec<String> = insns.iter().map(|i| format!("{} ({} dwords)", i.mnemonic, i.dwords)).collect();
            bad.push(format!("{} | {} -> {}", v.hex(), v.asm, got.join(", ")));
        }
    }
    report("wrong instruction length", all.len(), &bad);
}

/// A program cut short inside an instruction must still decode to the dwords it has.
#[test]
fn truncated_instructions_decode_without_panicking() {
    let all = vectors();
    let mut bad = Vec::new();
    for v in &all {
        let isa = v.isa();
        for cut in 1..v.words.len() {
            let used: usize = disassemble(&isa[..cut * 4]).iter().map(|i| i.dwords).sum();
            if used != cut {
                bad.push(format!("{} cut to {cut} dwords decodes {used}", v.hex()));
            }
        }
    }
    report("truncated decode covers the wrong number of dwords", all.len(), &bad);
}

/// The translator may refuse an instruction, but it must return, not panic. Each vector is
/// translated as a one-instruction pixel shader and compute shader.
#[test]
fn translator_returns_for_every_vector() {
    let all = vectors();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let bad: Vec<String> = std::thread::scope(|s| {
        let chunks: Vec<_> = all.chunks(all.len().div_ceil(threads)).map(|chunk| {
            s.spawn(move || {
                let mut bad = Vec::new();
                for v in chunk {
                    for stage in [Stage::Pixel, Stage::Compute] {
                        let ex = shader(stage, &v.words);
                        if std::panic::catch_unwind(|| translate(&ex).is_ok()).is_err() {
                            bad.push(format!("{} | {} ({stage:?})", v.hex(), v.asm));
                        }
                    }
                }
                bad
            })
        }).collect();
        chunks.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    std::panic::set_hook(hook);
    report("translator panicked", all.len(), &bad);
}

/// A 64-bit operation can take a 32-bit operand in v255, which has no upper half.
#[test]
fn f64_operations_take_v255_as_a_32_bit_operand() {
    // v_cvt_f64_i32 v[5:6], v255
    let t = translate(&shader(Stage::Pixel, &[0x7e0a_09ff])).expect("v_cvt_f64_i32 v[5:6], v255");
    assert!(t.hlsl.contains("(double)asint(v255)"), "{}", t.hlsl);
    // v_cvt_i32_f64 v255, v[1:2]
    let t = translate(&shader(Stage::Pixel, &[0x7ffe_0701])).expect("v_cvt_i32_f64 v255, v[1:2]");
    assert!(t.hlsl.contains("asdouble(v1, v2)") && t.hlsl.contains("v255 = "), "{}", t.hlsl);
    // v_ldexp_f64 v[5:6], v[1:2], v255
    let t = translate(&shader(Stage::Pixel, &[0xd568_0005, 0x0203_ff01])).expect("v_ldexp_f64 v[5:6], v[1:2], v255");
    assert!(t.hlsl.contains("tb_ldexp64(asdouble(v1, v2), asint(v255))"), "{}", t.hlsl);
}

/// A 64-bit operand based at v255 would need v256. It is refused, not wrapped to v0.
#[test]
fn f64_operand_based_at_v255_is_refused() {
    // v_cvt_i32_f64 v5, v[255:256]
    let e = translate(&shader(Stage::Pixel, &[0x7e0a_07ff])).err().expect("v[255:256] accepted");
    assert!(e.contains("unmodelled operand code 512"), "{e}");
}

/// SMEM opcodes the translator does not lower (scratch loads, stores, atomics, cache
/// discard, wave id) are refused, not run through the `s_buffer_load` path.
#[test]
fn unlowered_smem_opcodes_are_refused() {
    for (asm, words) in [
        ("s_scratch_load_dword s5, s[2:3], s101", [0xf414_0141u32, 0xca00_0000]),
        ("s_store_dword s1, s[4:5], 0x0", [0xf440_0042, 0xfa00_0000]),
        ("s_buffer_store_dword s1, s[8:11], 0x0", [0xf460_0044, 0xfa00_0000]),
        ("s_dcache_discard s[2:3], 0x64", [0xf4a0_0001, 0xfa00_0064]),
        ("s_get_waveid_in_workgroup s0", [0xf4a8_0000, 0x0000_0000]),
        ("s_atomic_add s5, s[2:3], 0x64", [0xf608_0141, 0xfa00_0064]),
    ] {
        let e = translate(&shader(Stage::Pixel, &words)).err().unwrap_or_else(|| panic!("{asm}: accepted"));
        assert!(e.contains("unsupported SMEM op"), "{asm}: {e}");
    }
}
