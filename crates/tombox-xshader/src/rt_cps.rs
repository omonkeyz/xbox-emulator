
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::hlsl::{bvh_global_helpers, global_helpers};
use crate::meta::{RootParamKind, RootSig};
use crate::rdna2::{disassemble, Enc, Insn, Opnd};
use crate::state_obj::{ContinuationSym, StateObjectProgram};

pub const XDXR_INTERNAL_SPACE: u32 = 214_743_647;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShaderId {
    pub bytes: [u8; 32],
}

impl ShaderId {
    pub const MAGIC: u32 = 0x5254_4944; // "RTID"

    pub fn new(program_index: u32, continuation_id: u32, name_hash: u64) -> Self {
        let mut bytes = [0u8; 32];
        bytes[0..4].copy_from_slice(&continuation_id.to_le_bytes());
        bytes[4..8].copy_from_slice(&program_index.to_le_bytes());
        bytes[8..16].copy_from_slice(&name_hash.to_le_bytes());
        bytes[16..20].copy_from_slice(&Self::MAGIC.to_le_bytes());
        Self { bytes }
    }

    pub fn continuation_id(&self) -> u32 {
        u32::from_le_bytes(self.bytes[0..4].try_into().unwrap())
    }

    pub fn program_index(&self) -> u32 {
        u32::from_le_bytes(self.bytes[4..8].try_into().unwrap())
    }
}

pub fn fnv1a_64(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[derive(Debug, Clone)]
pub struct TranslatedRtProgram {
    pub hlsl: String,
    pub entry: &'static str,
    pub profile: &'static str,
    pub exports: Vec<(String, ShaderId)>,
    pub continuations: usize,
    pub instruction_count: usize,
    pub uses_bvh: bool,
    pub uses_global: bool,
}

fn sorted_slices(prog: &StateObjectProgram) -> Vec<(&ContinuationSym, usize, usize)> {
    let base_shift = prog.isa.len().saturating_sub(prog.code_size as usize);
    let mut syms: Vec<&ContinuationSym> = prog.symbols.iter().collect();
    syms.sort_by_key(|s| s.code_offset);
    let mut out = Vec::with_capacity(syms.len());
    for i in 0..syms.len() {
        let start = (base_shift + syms[i].code_offset as usize).min(prog.isa.len());
        let end = if i + 1 < syms.len() {
            (base_shift + syms[i + 1].code_offset as usize).min(prog.isa.len())
        } else {
            prog.isa.len()
        };
        out.push((syms[i], start, end));
    }
    out
}

fn cont_id(sym: &ContinuationSym) -> u32 {
    (sym.code_offset >> 8) + 1
}

fn emit_root_decls(h: &mut String, rs: &RootSig) {
    let mut declared_b = BTreeSet::new();
    let mut declared_t = BTreeSet::new();
    let mut declared_u = BTreeSet::new();
    let mut declared_s = BTreeSet::new();

    for p in &rs.params {
        match &p.kind {
            RootParamKind::Constants { register, space, num32 } => {
                if *space == XDXR_INTERNAL_SPACE {
                    continue;
                }
                if declared_b.insert((*space, *register)) {
                    let nv = (*num32).div_ceil(4).max(1);
                    let _ = writeln!(h, "cbuffer TbRtC_{space}_{register} : register(b{register}, space{space}) {{ uint4 tb_rtc_{space}_{register}[{nv}]; }};");
                }
            }
            RootParamKind::Descriptor { kind: 2, register, space } => {
                if *space == XDXR_INTERNAL_SPACE {
                    continue;
                }
                if declared_b.insert((*space, *register)) {
                    let _ = writeln!(h, "cbuffer TbRtCb_{space}_{register} : register(b{register}, space{space}) {{ uint4 tb_rtcb_{space}_{register}[256]; }};");
                }
            }
            RootParamKind::Descriptor { kind: 3, register, space } => {
                if declared_t.insert((*space, *register)) {
                    let _ = writeln!(h, "ByteAddressBuffer tb_rtsrv_{space}_{register} : register(t{register}, space{space});");
                }
            }
            RootParamKind::Descriptor { kind: 4, register, space } => {
                if declared_u.insert((*space, *register)) {
                    let _ = writeln!(h, "RWByteAddressBuffer tb_rtuav_{space}_{register} : register(u{register}, space{space});");
                }
            }
            RootParamKind::Descriptor { .. } => {}
            RootParamKind::Table(ranges) => {
                for r in ranges {
                    let cnt = if r.num == u32::MAX { 1 } else { r.num.clamp(1, 64) };
                    let br = r.base_register;
                    match r.range_type {
                        0 if declared_t.insert((r.space, br)) => {
                            let _ = writeln!(h, "ByteAddressBuffer tb_rtt_{}_{br}[{cnt}] : register(t{br}, space{});", r.space, r.space);
                        }
                        1 if declared_u.insert((r.space, br)) => {
                            let _ = writeln!(h, "RWByteAddressBuffer tb_rtu_{}_{br}[{cnt}] : register(u{br}, space{});", r.space, r.space);
                        }
                        2 if r.space != XDXR_INTERNAL_SPACE && declared_b.insert((r.space, br)) => {
                            let _ = writeln!(h, "cbuffer TbRtTb_{}_{br} : register(b{br}, space{}) {{ uint4 tb_rttb_{}_{br}[64]; }};", r.space, r.space, r.space);
                        }
                        3 if declared_s.insert((r.space, br)) => {
                            let _ = writeln!(h, "SamplerState tb_rts_{}_{br}[{cnt}] : register(s{br}, space{});", r.space, r.space);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn opnd_r(o: &Opnd) -> String {
    match *o {
        Opnd::Sgpr(n) => format!("s[{}u]", (n as usize) & 127),
        Opnd::Vgpr(n) => format!("v[{}u]", (n as usize) & 255),
        Opnd::ConstInt(k) => format!("{:#x}u", k as u32),
        Opnd::ConstF32(f) => format!("{:#x}u", f.to_bits()),
        Opnd::Literal(u) => format!("{u:#x}u"),
        Opnd::VccLo => "vcc_lo".into(),
        Opnd::VccHi => "vcc_hi".into(),
        Opnd::ExecLo => "exec_lo".into(),
        Opnd::ExecHi => "exec_hi".into(),
        Opnd::M0 => "m0".into(),
        Opnd::Scc => "scc".into(),
        Opnd::Null | Opnd::Raw(_) => "0u".into(),
        Opnd::Temp(n) => format!("t_tmp{n}"),
    }
}

fn opnd_w(o: &Opnd, val: &str) -> String {
    match *o {
        Opnd::Sgpr(n) => format!("s[{}u] = {val};", (n as usize) & 127),
        Opnd::Vgpr(n) => format!("if (exec_lo & 1u) v[{}u] = {val};", (n as usize) & 255),
        Opnd::VccLo => format!("vcc_lo = {val};"),
        Opnd::VccHi => format!("vcc_hi = {val};"),
        Opnd::ExecLo => format!("exec_lo = {val};"),
        Opnd::ExecHi => format!("exec_hi = {val};"),
        Opnd::M0 => format!("m0 = {val};"),
        Opnd::Scc => format!("scc = ({val}) != 0u ? 1u : 0u;"),
        Opnd::Null => String::new(),
        Opnd::Temp(n) => format!("t_tmp{n} = {val};"),
        _ => String::new(),
    }
}

fn hi_opnd(o: &Opnd) -> Opnd {
    match *o {
        Opnd::Sgpr(n) => Opnd::Sgpr(n.saturating_add(1)),
        Opnd::Vgpr(n) => Opnd::Vgpr(n.saturating_add(1)),
        Opnd::VccLo => Opnd::VccHi,
        Opnd::ExecLo => Opnd::ExecHi,
        Opnd::ConstInt(k) => Opnd::ConstInt(if k < 0 { -1 } else { 0 }),
        _ => Opnd::Null,
    }
}

fn emit_insn(out: &mut String, ins: &Insn, tmp: &mut u32, pc_to_case: &BTreeMap<u32, u32>, base_off: u32) {
    let m = ins.mnemonic.as_str();
    let d0 = ins.dst.first().copied();
    let s = &ins.src;
    let r = |i: usize| s.get(i).map(opnd_r).unwrap_or_else(|| "0u".into());
    let f = |i: usize| format!("asfloat({})", r(i));

    match m {
        "s_nop" | "v_nop" | "s_waitcnt" | "s_waitcnt_vscnt" | "s_waitcnt_depctr" | "s_ttracedata_imm" | "s_clause" | "s_code_end" => {}
        "s_endpgm" => {
            let _ = writeln!(out, "                tb_cps_id = 0u; break;");
        }
        "s_setpc_b64" => {
            let _ = writeln!(out, "                tb_cps_id = (v[8u] != 0u) ? (v[8u] + 1u) : 0u; break;");
        }
        "s_swappc_b64" => {
            let src_lo = r(0);
            let _ = writeln!(out, "                tb_cps_ret = tb_cps_id; tb_cps_id = (({src_lo} >> 8u) != 0u) ? (({src_lo} >> 8u) + 1u) : 0u; break;");
        }
        "s_mov_b32" | "v_mov_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &r(0)));
            }
        }
        "s_mov_b64" => {
            if let Some(d) = d0 {
                let s0 = s.first().copied().unwrap_or(Opnd::Null);
                let _ = writeln!(out, "                {} {}", opnd_w(&d, &opnd_r(&s0)), opnd_w(&hi_opnd(&d), &opnd_r(&hi_opnd(&s0))));
            }
        }
        "s_add_u32" | "s_add_i32" | "v_add_nc_u32" | "v_add_co_u32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} + {})", r(0), r(1))));
            }
        }
        "v_add_co_ci_u32" | "s_addc_u32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} + {} + (vcc_lo & 1u))", r(0), r(1))));
            }
        }
        "s_sub_u32" | "s_sub_i32" | "v_sub_nc_u32" | "v_sub_co_u32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} - {})", r(0), r(1))));
            }
        }
        "v_subrev_nc_u32" | "v_subrev_co_u32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} - {})", r(1), r(0))));
            }
        }
        "s_and_b32" | "v_and_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} & {})", r(0), r(1))));
            }
        }
        "s_or_b32" | "v_or_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} | {})", r(0), r(1))));
            }
        }
        "v_or3_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} | {} | {})", r(0), r(1), r(2))));
            }
        }
        "s_xor_b32" | "v_xor_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} ^ {})", r(0), r(1))));
            }
        }
        "s_lshl_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} << ({} & 31u))", r(0), r(1))));
            }
        }
        "v_lshlrev_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} << ({} & 31u))", r(1), r(0))));
            }
        }
        "s_lshr_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} >> ({} & 31u))", r(0), r(1))));
            }
        }
        "v_lshrrev_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({} >> ({} & 31u))", r(1), r(0))));
            }
        }
        "s_lshl_b64" | "v_lshlrev_b64" => {
            if let Some(d) = d0 {
                let (sh, val_op) = if m == "s_lshl_b64" {
                    (r(1), s.first().copied().unwrap_or(Opnd::Null))
                } else {
                    (r(0), s.get(1).copied().unwrap_or(Opnd::Null))
                };
                let t = *tmp;
                *tmp += 1;
                let lo = opnd_r(&val_op);
                let hi = opnd_r(&hi_opnd(&val_op));
                let _ = writeln!(
                    out,
                    "                uint64_t t{t} = (((uint64_t){hi} << 32) | (uint64_t){lo}) << ({sh} & 63u); {} {}",
                    opnd_w(&d, &format!("(uint)t{t}")),
                    opnd_w(&hi_opnd(&d), &format!("(uint)(t{t} >> 32)"))
                );
            }
        }
        "s_lshr_b64" | "v_lshrrev_b64" => {
            if let Some(d) = d0 {
                let (sh, val_op) = if m == "s_lshr_b64" {
                    (r(1), s.first().copied().unwrap_or(Opnd::Null))
                } else {
                    (r(0), s.get(1).copied().unwrap_or(Opnd::Null))
                };
                let t = *tmp;
                *tmp += 1;
                let lo = opnd_r(&val_op);
                let hi = opnd_r(&hi_opnd(&val_op));
                let _ = writeln!(
                    out,
                    "                uint64_t t{t} = (((uint64_t){hi} << 32) | (uint64_t){lo}) >> ({sh} & 63u); {} {}",
                    opnd_w(&d, &format!("(uint)t{t}")),
                    opnd_w(&hi_opnd(&d), &format!("(uint)(t{t} >> 32)"))
                );
            }
        }
        "v_lshl_add_u32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("(({} << ({} & 31u)) + {})", r(0), r(1), r(2))));
            }
        }
        "v_lshl_or_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("(({} << ({} & 31u)) | {})", r(0), r(1), r(2))));
            }
        }
        "v_bfe_u32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("(({} >> ({} & 31u)) & ((1u << min({} & 31u, 31u)) - 1u))", r(0), r(1), r(2))));
            }
        }
        "s_bfe_u32" => {
            if let Some(d) = d0 {
                let t = *tmp;
                *tmp += 1;
                let _ = writeln!(
                    out,
                    "                uint t{t}w = ({} >> 16u) & 0x7fu; {}",
                    r(1),
                    opnd_w(&d, &format!("(t{t}w >= 32u ? ({} >> ({} & 31u)) : (({} >> ({} & 31u)) & ((1u << t{t}w) - 1u)))", r(0), r(1), r(0), r(1)))
                );
            }
        }
        "v_mul_f32" | "v_mul_legacy_f32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("asuint({} * {})", f(0), f(1))));
            }
        }
        "v_add_f32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("asuint({} + {})", f(0), f(1))));
            }
        }
        "v_sub_f32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("asuint({} - {})", f(0), f(1))));
            }
        }
        "v_fmac_f32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("asuint(mad({}, {}, asfloat({})))", f(0), f(1), opnd_r(&d))));
            }
        }
        "v_fma_f32" | "v_mad_f32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("asuint(mad({}, {}, {}))", f(0), f(1), f(2))));
            }
        }
        "v_rcp_f32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("asuint(1.0 / {})", f(0))));
            }
        }
        "v_min_f32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("asuint(min({}, {}))", f(0), f(1))));
            }
        }
        "v_max_f32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("asuint(max({}, {}))", f(0), f(1))));
            }
        }
        "v_cndmask_b32" => {
            if let Some(d) = d0 {
                let mask = if s.len() >= 3 { r(2) } else { "vcc_lo".into() };
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("((({mask}) & 1u) != 0u ? {} : {})", r(1), r(0))));
            }
        }
        "s_and_saveexec_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {} exec_lo &= {}; scc = exec_lo != 0u ? 1u : 0u;", opnd_w(&d, "exec_lo"), r(0));
            }
        }
        "v_readfirstlane_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("WaveReadLaneFirst({})", r(0))));
            }
        }
        "v_writelane_b32" => {
            if let Some(d) = d0 {
                let _ = writeln!(out, "                if (WaveGetLaneIndex() == ({} & 31u)) {}", r(1), opnd_w(&d, &r(0)));
            }
        }
        m if m.starts_with("v_cmp_") => {
            if let Some(d) = d0 {
                let (a, b) = (r(0), r(1));
                let cond = if m.contains("_eq_") {
                    format!("({a} == {b})")
                } else if m.contains("_ne_") {
                    format!("({a} != {b})")
                } else if m.contains("_lt_i32") {
                    format!("(asint({a}) < asint({b}))")
                } else if m.contains("_le_i32") {
                    format!("(asint({a}) <= asint({b}))")
                } else if m.contains("_gt_i32") {
                    format!("(asint({a}) > asint({b}))")
                } else if m.contains("_ge_i32") {
                    format!("(asint({a}) >= asint({b}))")
                } else if m.contains("_lt_u32") {
                    format!("({a} < {b})")
                } else if m.contains("_le_u32") {
                    format!("({a} <= {b})")
                } else if m.contains("_gt_u32") {
                    format!("({a} > {b})")
                } else if m.contains("_ge_u32") {
                    format!("({a} >= {b})")
                } else if m.contains("_lt_f32") {
                    format!("(asfloat({a}) < asfloat({b}))")
                } else if m.contains("_le_f32") {
                    format!("(asfloat({a}) <= asfloat({b}))")
                } else if m.contains("_gt_f32") {
                    format!("(asfloat({a}) > asfloat({b}))")
                } else if m.contains("_ge_f32") {
                    format!("(asfloat({a}) >= asfloat({b}))")
                } else {
                    "false".to_string()
                };
                let _ = writeln!(out, "                {}", opnd_w(&d, &format!("({cond} ? 1u : 0u)")));
            }
        }
        m if ins.enc == Enc::Flat => {
            let off = ins.aux.imm as i32;
            let v0 = match s.first() {
                Some(Opnd::Vgpr(v)) => *v,
                _ => 0,
            };
            let va_expr = match s.get(2) {
                Some(Opnd::Sgpr(sg)) => format!("tb_gva(s[{}u], s[{}u], v[{}u], {off})", sg, sg.saturating_add(1), v0),
                _ => format!("tb_gva(v[{}u], v[{}u], 0u, {off})", v0, v0.saturating_add(1)),
            };
            let t = *tmp;
            *tmp += 1;
            let _ = writeln!(out, "                uint2 g{t} = {va_expr};");
            let n = if m.ends_with("dwordx4") {
                4u8
            } else if m.ends_with("dwordx3") {
                3
            } else if m.ends_with("dwordx2") {
                2
            } else {
                1
            };
            if m.contains("_store_") {
                let dreg = match s.get(1) {
                    Some(Opnd::Vgpr(v)) => *v,
                    _ => 0,
                };
                for k in 0..n {
                    let _ = writeln!(
                        out,
                        "                if (exec_lo & 1u) tb_gmem_store(tb_gva(g{t}.x, g{t}.y, {}u, 0), v[{}u]);",
                        4 * k as u32,
                        (dreg as usize + k as usize) & 255
                    );
                }
            } else if let Some(Opnd::Vgpr(vd)) = d0 {
                for k in 0..n {
                    let _ = writeln!(
                        out,
                        "                if (exec_lo & 1u) v[{}u] = tb_gmem_load(tb_gva(g{t}.x, g{t}.y, {}u, 0), 4u);",
                        (vd as usize + k as usize) & 255,
                        4 * k as u32
                    );
                }
            }
        }
        m if m.starts_with("image_bvh") => {
            let wide = m == "image_bvh64_intersect_ray";
            let n = s.len();
            if n >= 3 {
                let sr = match s[n - 2] {
                    Opnd::Sgpr(sg) => sg as usize,
                    _ => 0,
                };
                let vbase = match s[0] {
                    Opnd::Vgpr(v) => v as usize,
                    _ => 0,
                };
                let vd = match d0 {
                    Some(Opnd::Vgpr(v)) => v as usize,
                    _ => 0,
                };
                let t = *tmp;
                *tmp += 1;
                let (node_expr, ro) = if wide {
                    (format!("(((uint64_t)v[{}u]) << 32) | (uint64_t)v[{}u]", (vbase + 1) & 255, vbase & 255), vbase + 2)
                } else {
                    (format!("(uint64_t)v[{}u]", vbase & 255), vbase + 1)
                };
                let _ = writeln!(
                    out,
                    "                uint4 b{t} = tb_bvh_intersect(uint4(s[{}u], s[{}u], s[{}u], s[{}u]), {node_expr}, asfloat(v[{}u]), asfloat(uint3(v[{}u], v[{}u], v[{}u])), asfloat(uint3(v[{}u], v[{}u], v[{}u])), asfloat(uint3(v[{}u], v[{}u], v[{}u])));",
                    sr & 127, (sr + 1) & 127, (sr + 2) & 127, (sr + 3) & 127,
                    ro & 255,
                    (ro + 1) & 255, (ro + 2) & 255, (ro + 3) & 255,
                    (ro + 4) & 255, (ro + 5) & 255, (ro + 6) & 255,
                    (ro + 7) & 255, (ro + 8) & 255, (ro + 9) & 255
                );
                let _ = writeln!(
                    out,
                    "                if (exec_lo & 1u) {{ v[{}u] = b{t}.x; v[{}u] = b{t}.y; v[{}u] = b{t}.z; v[{}u] = b{t}.w; }}",
                    vd & 255, (vd + 1) & 255, (vd + 2) & 255, (vd + 3) & 255
                );
            }
        }
        m if m.starts_with("buffer_store_dword") || m.starts_with("buffer_load_dword") => {
            let addr = r(0);
            if m.starts_with("buffer_store") {
                if let Some(Opnd::Vgpr(vd)) = d0 {
                    let _ = writeln!(out, "                if (exec_lo & 1u) tb_stack[(({addr} + {:#x}u) >> 2u) & 127u] = v[{}u];", ins.aux.imm as u32, vd);
                }
            } else if let Some(Opnd::Vgpr(vd)) = d0 {
                let _ = writeln!(out, "                if (exec_lo & 1u) v[{}u] = tb_stack[(({addr} + {:#x}u) >> 2u) & 127u];", vd, ins.aux.imm as u32);
            }
        }
        _ => {
            let _ = (pc_to_case, base_off);
            let _ = writeln!(out, "                // rt_cps passthrough: {m} (+{:#x})", ins.offset);
        }
    }
}

pub fn program_exports(prog: &StateObjectProgram, program_index: u32) -> Vec<(String, ShaderId)> {
    if prog.symbols.is_empty() {
        return Vec::new();
    }
    let slices = sorted_slices(prog);
    let main_sym = slices
        .iter()
        .find(|(s, _, _)| s.name == "main")
        .or_else(|| slices.first())
        .map(|(s, _, _)| cont_id(s))
        .unwrap_or(1);
    let mut exports = Vec::new();
    for exp in &prog.exports {
        let entry_cid = slices
            .iter()
            .find(|(s, _, _)| s.name == *exp || s.name.starts_with(&format!("{exp}.")))
            .map(|(s, _, _)| cont_id(s))
            .unwrap_or(main_sym);
        exports.push((exp.clone(), ShaderId::new(program_index, entry_cid, fnv1a_64(exp))));
    }
    exports
}

pub fn translate_state_program(prog: &StateObjectProgram, program_index: u32) -> Result<TranslatedRtProgram, String> {
    if prog.symbols.is_empty() {
        return Err("state object program has no continuation symbols".into());
    }
    let slices = sorted_slices(prog);
    let mut pc_to_case = BTreeMap::new();
    for &(sym, _, _) in &slices {
        pc_to_case.insert(sym.code_offset, cont_id(sym));
    }

    let mut h = String::with_capacity(8192);
    let _ = writeln!(
        h,
        "// Generated by tombox-xshader rt_cps from Xbox Series DXR state object ({} continuations).",
        slices.len()
    );
    h.push_str(&global_helpers());
    h.push_str(&bvh_global_helpers());
    let _ = writeln!(h, "cbuffer TbDxrDispatch : register(b0, space{XDXR_INTERNAL_SPACE}) {{ uint4 tb_dxr_d[3]; uint tb_dxr_d12; }};");
    let _ = writeln!(h, "cbuffer TbDxrTable : register(b1, space{XDXR_INTERNAL_SPACE}) {{ uint2 tb_dxr_t; }};");
    emit_root_decls(&mut h, &prog.root_sig);

    let main_sym = slices
        .iter()
        .find(|(s, _, _)| s.name == "main")
        .or_else(|| slices.first())
        .map(|(s, _, _)| cont_id(s))
        .unwrap_or(1);

    let mut total_insns = 0usize;
    let mut uses_bvh = false;
    let mut uses_global = false;
    let mut tmp = 0u32;

    let _ = writeln!(h, "[numthreads(8, 8, 1)]");
    let _ = writeln!(h, "void main(uint3 dtid : SV_DispatchThreadID, uint3 gtid : SV_GroupThreadID, uint3 gid : SV_GroupID) {{");
    let _ = writeln!(h, "    uint width = tb_dxr_d[2].z, height = tb_dxr_d[2].w, depth = max(tb_dxr_d12, 1u);");
    let _ = writeln!(h, "    if (dtid.x >= width || dtid.y >= height || dtid.z >= depth) return;");
    let _ = writeln!(h, "    uint s[128]; uint v[256]; uint tb_stack[128];");
    let _ = writeln!(h, "    [unroll] for (uint i = 0u; i < 128u; i++) {{ s[i] = 0u; tb_stack[i] = 0u; }}");
    let _ = writeln!(h, "    [unroll] for (uint i = 0u; i < 256u; i++) {{ v[i] = 0u; }}");
    let _ = writeln!(h, "    uint exec_lo = 1u, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u;");
    let _ = writeln!(h, "    v[0u] = gtid.x; v[1u] = gtid.y; v[2u] = gtid.z;");
    let _ = writeln!(h, "    s[4u] = gid.x; s[5u] = gid.y; s[6u] = gid.z;");
    let _ = writeln!(h, "    s[12u] = tb_dxr_d[0].x; s[13u] = tb_dxr_d[0].y; s[14u] = width; s[15u] = height; s[16u] = depth;");
    let _ = writeln!(h, "    s[19u] = tb_dxr_d[0].z; s[20u] = tb_dxr_d[0].w; s[21u] = tb_dxr_d[1].x; s[22u] = tb_dxr_d[1].y;");
    let _ = writeln!(h, "    s[23u] = tb_dxr_d[1].z; s[24u] = tb_dxr_d[1].w; s[25u] = tb_dxr_d[2].x; s[26u] = tb_dxr_d[2].y;");
    let _ = writeln!(h, "    uint tb_cps_id = tb_dxr_t.x != 0u ? tb_dxr_t.x : {main_sym}u;");
    let _ = writeln!(h, "    uint tb_cps_ret = 0u;");
    let _ = writeln!(h, "    for (uint tb_step = 0u; tb_step < 256u && tb_cps_id != 0u; tb_step++) {{");
    let _ = writeln!(h, "        switch (tb_cps_id) {{");

    for &(sym, start, end) in &slices {
        let cid = cont_id(sym);
        let insns = if start < end { disassemble(&prog.isa[start..end]) } else { Vec::new() };
        total_insns += insns.len();
        let _ = writeln!(h, "            case {cid}u: {{ // {} @ +{:#x} ({} insns)", sym.name, sym.code_offset, insns.len());
        let mut terminated = false;
        for ins in &insns {
            if ins.mnemonic.starts_with("image_bvh") {
                uses_bvh = true;
            }
            if ins.enc == Enc::Flat {
                uses_global = true;
            }
            emit_insn(&mut h, ins, &mut tmp, &pc_to_case, sym.code_offset);
            if matches!(ins.mnemonic.as_str(), "s_endpgm" | "s_setpc_b64" | "s_swappc_b64") {
                terminated = true;
                break;
            }
        }
        if !terminated {
            let _ = writeln!(h, "                tb_cps_id = 0u; break;");
        }
        let _ = writeln!(h, "            }}");
    }

    let _ = writeln!(h, "            default: tb_cps_id = 0u; break;");
    let _ = writeln!(h, "        }}");
    let _ = writeln!(h, "    }}");
    let _ = writeln!(h, "}}");

    let exports = program_exports(prog, program_index);

    Ok(TranslatedRtProgram {
        hlsl: h,
        entry: "main",
        profile: "cs_6_2",
        exports,
        continuations: slices.len(),
        instruction_count: total_insns,
        uses_bvh,
        uses_global,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::RootSig;
    use crate::state_obj::{parse_state_object, ContinuationSym, StateObjectProgram};

    #[test]
    fn translates_synthetic_cps_program_and_compiles_with_dxc() {
        let words: [u32; 5] = [
            0xdc30_8000, 0x057d_0001, // flat_load_dword v5, v[1:2]
            0xf19c_0f01, 0x0000_1401, // image_bvh64_intersect_ray
            0xbf81_0000,              // s_endpgm
        ];
        let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let prog = StateObjectProgram {
            exports: vec!["SyntheticRayGen".into(), "XDXR_TraceRay".into()],
            root_sig_dxbc: Vec::new(),
            root_sig: RootSig { version: 2, flags: 0, params: Vec::new(), num_static_samplers: 0, static_samplers: Vec::new() },
            pgm_rsrc: vec![0; 7],
            meta: vec![0; 5],
            code_size: isa.len() as u32,
            isa,
            symbols: vec![ContinuationSym { name: "SyntheticRayGen.ss_0".into(), code_offset: 0, flags: 0, scratch_bytes: 0, rsrc_info: 0 }],
        };
        let tr = translate_state_program(&prog, 0).expect("translate synthetic RT program");
        assert!(tr.uses_bvh);
        assert!(tr.uses_global);
        assert_eq!(tr.continuations, 1);
        assert_eq!(tr.exports.len(), 2);
        assert_eq!(tr.exports[0].0, "SyntheticRayGen");
        assert_eq!(tr.exports[0].1.continuation_id(), 1);
        assert!(tr.hlsl.contains("tb_bvh_intersect("), "{}", tr.hlsl);
        assert!(tr.hlsl.contains("tb_gmem_load("), "{}", tr.hlsl);

        #[cfg(windows)]
        if crate::dxc::available() {
            let dxil = crate::dxc::compile(&tr.hlsl, tr.profile).expect("DXC compile synthetic RT CPS shader");
            assert!(dxil.len() > 64);
        }
    }

    #[test]
    fn optional_translate_and_compile_real_state_object() {
        let Ok(path) = std::env::var("TOMBOX_TEST_STATEOBJ") else { return };
        let blob = std::fs::read(&path).expect("read stateobj");
        let so = parse_state_object(&blob).expect("parse stateobj");
        for (i, prog) in so.programs.iter().enumerate() {
            let tr = translate_state_program(prog, i as u32).expect("translate real stateobj program");
            assert!(tr.continuations >= 2, "program {i} continuations={}", tr.continuations);
            assert!(tr.instruction_count > 10, "program {i} insns={}", tr.instruction_count);
            #[cfg(windows)]
            if crate::dxc::available() {
                let dxil = crate::dxc::compile(&tr.hlsl, tr.profile).unwrap_or_else(|e| panic!("DXC failed on program #{i}: {e}\n{}", tr.hlsl));
                assert!(dxil.len() > 64);
            }
        }
    }
}
