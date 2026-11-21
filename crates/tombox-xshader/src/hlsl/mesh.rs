use super::{f, r, semantic, sig_type, Emitter, XYZW};
use crate::meta::MeshInfo;
use crate::rdna2::{Insn, Opnd};
use crate::Stage;
use std::fmt::Write as _;

pub const GS_ALLOC_REQ: u32 = 9;
pub const EXP_PRIM: u32 = 20;
pub const MS_LDS_DWORDS: usize = 7167;
pub const AS_ABI_UNKNOWN: &str = "amplification shader: the console AS (xgs12_10 0xde6f0) gets s[0:1] = user-data block, s2 = mesh-args ring >> 8, s3 = payload ring >> 8 and gid.x offset by COMPUTE_START_X, and leaves through global memory stores into those rings; the translator has no global-store path and the per-entry ring addressing and the mesh shader's payload register are not decoded";

pub fn wave_size(insns: &[Insn]) -> u32 {
    let w64 = insns.iter().any(|i| {
        i.mnemonic == "v_mbcnt_hi_u32_b32"
            || i.mnemonic.ends_with("saveexec_b64")
            || (i.mnemonic.ends_with("_b64") && matches!(i.dst.first(), Some(Opnd::ExecLo)))
    });
    if w64 { 64 } else { 32 }
}

fn all_lanes(o: &Opnd) -> bool {
    matches!(o, Opnd::ConstInt(-1)) || matches!(o, Opnd::Literal(0xffff_ffff))
}

impl<'a> Emitter<'a> {
    pub(super) fn is_ms(&self) -> bool {
        self.ex.stage == Stage::Mesh
    }

    pub(super) fn mesh_info(&self) -> Result<MeshInfo, String> {
        self.ex
            .gs_info
            .and_then(|g| g.mesh)
            .filter(|m| m.stage == 13)
            .ok_or_else(|| "mesh shader without PSV0 mesh runtime info".to_string())
    }

    fn ms_threads(&self) -> Result<u32, String> {
        let m = self.mesh_info()?;
        let n = m.num_threads.iter().map(|&x| x.max(1)).product::<u32>();
        if m.num_threads[0] == 0 || n > 128 {
            return Err(format!("mesh shader thread-group size {:?} outside 1..128", m.num_threads));
        }
        Ok(n)
    }

    pub(super) fn mesh_mbcnt(&self, ins: &Insn, b: String) -> Result<String, String> {
        if !all_lanes(&ins.src[0]) {
            return Err(format!("{} with a mask other than all lanes in a mesh shader at {:#x}", ins.mnemonic, ins.offset));
        }
        let w = wave_size(&self.insns);
        Ok(match (ins.mnemonic.as_str(), w) {
            ("v_mbcnt_lo_u32_b32", 64) => format!("(min(gi & 63u, 32u) + {b})"),
            ("v_mbcnt_lo_u32_b32", _) => format!("((gi & 31u) + {b})"),
            ("v_mbcnt_hi_u32_b32", 64) => format!("(max(gi & 63u, 32u) - 32u + {b})"),
            _ => b,
        })
    }

    // v5 is the merged ES-GS "vertex id" VGPR (after the five GS VGPRs; Mesa radv_shader_args.c
    // declare_ms_input_vgprs L327-337 and L865-886 at Mesa ef594386). Two readings of it exist for
    // a GFX10.3 mesh launch, and this translator keeps the first one:
    // - Target (taken from the GTA VI mesh corpus layout): every lane holds its own vertex id,
    //   gid.x * threads + thread index, as if the driver drew threads vertices per workgroup. Then
    //   v_readfirstlane_b32 sX, v5 gives gid.x * threads + wave base.
    // - Mesa RADV and AMD LLPC (legacy FAST_LAUNCH=1): the driver draws one input vertex per
    //   workgroup (radv_cmd_buffer.c L12436 count = x * y * z, L4187 fast launch 1 below GFX11;
    //   radv_shader_info.c L767-790 hw_max_esverts = max_gsprims = 1), so v5 is the flat
    //   workgroup index. ac_nir_lower_ngg_mesh.c L603-612 says it is only filled for lane 0 of
    //   wave 0 and broadcasts it through LDS; radv_shader_info.c L789 says every lane gets it;
    //   LLPC lgc/lowering/MeshTaskShader.cpp L2398-2400 (40cb8d9) reads VGPR5 as the flat
    //   workgroup id, "incremented by 1 for each subgroup", and also broadcasts it via LDS.
    //   Leonida's console notes use this value.
    // Both are statements about how the driver programs the GE, not a fixed hardware register
    // definition, and the console driver's programming is unknown. What settles it is how GTA VI
    // consumes v_readfirstlane_b32 sX, v5: a shift right by log2(threads) (or a divide by threads)
    // supports the target reading; direct use as a group or meshlet index, a left shift, or an LDS
    // broadcast from wave 0 supports the workgroup-index reading. s3[7:0]/[15:8] below follow the
    // same choice and would become 1/1 in wave 0 and 0 elsewhere under the other reading.
    pub(super) fn mesh_readfirstlane(&self, src: &Opnd) -> Result<Option<String>, String> {
        if !self.is_ms() || !matches!(src, Opnd::Vgpr(5)) {
            return Ok(None);
        }
        let written = self.insns[..self.cur_idx.min(self.insns.len())].iter().any(|i| i.dst.iter().any(|d| matches!(d, Opnd::Vgpr(5))));
        if written {
            return Ok(None);
        }
        let w = wave_size(&self.insns);
        Ok(Some(format!("(gid.x * {}u + (gi & {:#x}u))", self.ms_threads()?, !(w - 1))))
    }

    pub(super) fn mesh_sendmsg(&mut self, ins: &Insn) -> Result<(), String> {
        let msg = ins.aux.imm as u32 & 0xf;
        if msg != GS_ALLOC_REQ {
            return Err(format!("s_sendmsg message {msg} in a mesh shader unsupported"));
        }
        self.line("if (exec_lo & 1u) tb_mscnt = m0;");
        Ok(())
    }

    pub(super) fn mesh_barrier(&mut self) -> Result<(), String> {
        if self.brace_depth == 0 && self.loop_depth == 0 {
            self.lds_w_pending = false;
            self.lds_r_pending = false;
            self.line("GroupMemoryBarrierWithGroupSync();");
            return Ok(());
        }
        use crate::rdna2::Enc;
        let writes = self.insns.iter().any(|i| {
            i.enc == Enc::Ds || (matches!(i.enc, Enc::Mubuf | Enc::Mtbuf | Enc::Flat | Enc::Mimg) && (i.mnemonic.contains("store") || i.mnemonic.contains("atomic") || i.mnemonic.contains('<')))
        });
        if writes {
            return Err("s_barrier on a divergent path of a mesh shader that writes LDS or memory".into());
        }
        Ok(())
    }

    fn distance_slots(&self) -> Vec<(usize, usize)> {
        let osig = &self.ex.output_sig;
        let mut out = Vec::new();
        for sv in [2u32, 3] {
            let mut els: Vec<usize> = (0..osig.len()).filter(|&i| osig[i].sysval == sv).collect();
            els.sort_by_key(|&i| (osig[i].register, osig[i].mask.trailing_zeros()));
            for ei in els {
                for n in 0..osig[ei].mask.count_ones() as usize {
                    out.push((ei, n));
                }
            }
        }
        out
    }

    pub(super) fn mesh_exp(&mut self, ins: &Insn) -> Result<bool, String> {
        let target = ins.aux.imm as u32;
        let en = ins.aux.mask;
        if target == EXP_PRIM {
            if en & 1 != 0 {
                let v = r(&ins.src[0])?;
                self.line(&format!("if (exec_lo & 1u) tb_prim = {v};"));
            }
            return Ok(true);
        }
        if (21..32).contains(&target) {
            return Err(format!("mesh export target {target} unsupported"));
        }
        if !(13..=15).contains(&target) {
            return Ok(false);
        }
        if self.ex.output_sig.iter().any(|e| matches!(e.sysval, 4 | 5)) {
            return Err("mesh shader with a layer/viewport output: position-export misc vector unsupported".into());
        }
        let slots = self.distance_slots();
        let k = (target - 13) as usize;
        for c in 0..4usize {
            if (en >> c) & 1 == 0 {
                continue;
            }
            let Some(&(ei, n)) = slots.get(4 * k + c) else {
                return Err(format!("position export {target}.{} has no clip/cull distance element", XYZW[c]));
            };
            let one = self.ex.output_sig[ei].mask.count_ones() == 1;
            let field = if one { format!("o.o{ei}") } else { format!("o.o{ei}.{}", XYZW[n]) };
            let v = f(&ins.src[c])?;
            self.line(&format!("if (exec_lo & 1u) {field} = {v};"));
        }
        Ok(true)
    }

    pub(super) fn finish_mesh(&mut self, mut h: String) -> Result<String, String> {
        let m = self.mesh_info()?;
        let n = self.ms_threads()?;
        if m.prim_vectors != 0 {
            return Err("mesh shader with per-primitive outputs unsupported".into());
        }
        let pl = if m.payload_bytes != 0 { Some(self.amp_ms_payload(m.payload_bytes)?) } else { None };
        let (topo, idx_ty, nidx) = match m.topology {
            1 => ("line", "uint2", 2usize),
            2 => ("triangle", "uint3", 3usize),
            t => return Err(format!("mesh output topology {t} unsupported")),
        };
        let (mv, mp) = (m.max_vertices as u32, m.max_primitives as u32);
        if mv == 0 || mp == 0 {
            return Err(format!("mesh shader with max vertices {mv} / primitives {mp}"));
        }
        if mv > n || mp > n {
            return Err(format!("mesh shader exports {mv} vertices / {mp} primitives from {n} threads (multi-row export)"));
        }
        if self.ex.output_sig.is_empty() {
            return Err("mesh shader without vertex outputs".into());
        }
        if !self.insns.iter().any(|i| i.mnemonic == "s_sendmsg" && i.aux.imm as u32 & 0xf == GS_ALLOC_REQ) {
            return Err("mesh shader without s_sendmsg GS_ALLOC_REQ: no output allocation".into());
        }
        let lds_limit = MS_LDS_DWORDS * 4;
        if m.groupshared_bytes as usize > lds_limit {
            return Err(format!("mesh shader uses {} bytes of groupshared memory (PC limit {lds_limit})", m.groupshared_bytes));
        }
        if let Some(o) = self.insns.iter().filter(|i| i.enc == crate::rdna2::Enc::Ds).map(|i| i.aux.imm.max(0) as usize + 16).max() {
            if o > lds_limit {
                return Err(format!("mesh shader LDS offset {o} beyond the PC groupshared limit {lds_limit}"));
            }
        }
        if self.uses_lds {
            let decl = "groupshared uint tb_lds[8192];";
            if !h.contains(decl) {
                return Err("mesh shader LDS declaration not found".into());
            }
            h = h.replace(decl, &format!("groupshared uint tb_lds[{MS_LDS_DWORDS}];"));
        }
        if let Some(p) = &pl {
            h.insert_str(0, "#define TB_AMP_GMEM 1
");
            h.push_str(p);
        }
        let _ = writeln!(h, "groupshared uint tb_mscnt;");
        let _ = writeln!(h, "struct SOut {{");
        for (ei, e) in self.ex.output_sig.iter().enumerate() {
            let (ty, _) = sig_type(e);
            if e.sysval == 1 {
                let _ = writeln!(h, "    precise {ty} o{ei} : {};", semantic(e));
            } else {
                let _ = writeln!(h, "    {ty} o{ei} : {};", semantic(e));
            }
        }
        let _ = writeln!(h, "}};");
        let _ = writeln!(h, "struct SPrim {{ bool cull : SV_CullPrimitive; }};");
        let [tx, ty_, tz] = m.num_threads.map(|x| x.max(1));
        let _ = writeln!(h, "[numthreads({tx}, {ty_}, {tz})]");
        let _ = writeln!(h, "[outputtopology(\"{topo}\")]");
        let _ = writeln!(
            h,
            "void main(uint gi : SV_GroupIndex, uint3 gid : SV_GroupID{}, out vertices SOut verts[{mv}], out indices {idx_ty} tris[{mp}], out primitives SPrim prims[{mp}]) {{",
            if pl.is_some() { ", in payload TbPayload tb_pl" } else { "" }
        );
        let _ = writeln!(h, "    uint exec_lo = 1u, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;");
        for reg in ["v5", "s3"] {
            self.regs.insert(reg.to_string());
        }
        if pl.is_some() {
            self.amp_ms_regs();
        }
        let _ = writeln!(h, "{}", self.reg_decls());
        let w = wave_size(&self.insns);
        let _ = writeln!(h, "    SOut o = (SOut)0; uint tb_prim = 0u;");
        let _ = writeln!(h, "    if (gi == 0u) tb_mscnt = 0u;");
        let _ = writeln!(h, "    GroupMemoryBarrierWithGroupSync();");
        // s3 = merged_wave_info, RDNA2 ISA §3.12.2 "SGPR Initialization", Geometry Shader (ES+GS) SGPR 3:
        // { TGsize[31:28], WaveInSubgroup[27:24], GSWaveID[23:16], GSPrimCount[15:8], ESVertCount[7:0] }.
        // Mesa ef594386 reads the same fields: ac_nir_lower_intrinsics_to_args.c L99 subgroup id =
        // [27:24] and L127 num_subgroups = [31:28] for NGG; ac_nir_lower_ngg.c L157/L163 vertex and
        // primitive presence from [7:0]/[15:8], L178 prim count = [15:8]; L409 gs_wave_id = [23:16]
        // (legacy GS ring addressing). LLPC MeshTaskShader.cpp L2189-2193 takes the mesh wave id from
        // [27:24]. The subgroup is the API workgroup here (finish_mesh refuses max vertices or
        // primitives above the thread count), so TGsize = ceil(threads / wave). The per-wave vertex and
        // primitive counts are the wave's live threads, matching the per-lane v5 reading (see
        // mesh_readfirstlane). GSWaveID is a running GS wave counter used only for legacy GS rings and
        // the ring messages are refused in mesh shaders, so it stays 0.
        let tg = n.div_ceil(w);
        let _ = writeln!(h, "    v5 = gid.x * {n}u + gi;");
        let _ = writeln!(h, "    s3 = {:#x}u | ((gi / {w}u) << 24) | (min({w}u, {n}u - (gi & {:#x}u)) * 0x101u);", tg << 28, !(w - 1));
        if pl.is_some() {
            h.push_str(&self.amp_ms_prologue(m.payload_bytes));
        }
        h.push_str(&self.body);
        let _ = writeln!(h, "    GroupMemoryBarrierWithGroupSync();");
        // GS_ALLOC_REQ payload: RDNA2 ISA §12.5.1 "Send Message", GS alloc req: "M0[9:0] = number of vertices,
        // M0[22:12] = number of primitives". Mesa ac_nir_prerast_utils.c L873-878 and LLPC
        // MeshTaskShader.cpp L1778 describe vertices as M0[10:0]; bit 10 only matters above 1023
        // vertices, beyond any mesh limit, so the ISA width is used.
        let _ = writeln!(h, "    uint tb_nv = tb_mscnt & 0x3ffu, tb_np = (tb_mscnt >> 12) & 0x7ffu;");
        let _ = writeln!(h, "    SetMeshOutputCounts(tb_nv, tb_np);");
        let _ = writeln!(h, "    if (gi < tb_nv) verts[gi] = o;");
        let idx: Vec<&str> = ["tb_prim & 0x1ffu", "(tb_prim >> 10) & 0x1ffu", "(tb_prim >> 20) & 0x1ffu"][..nidx].to_vec();
        let _ = writeln!(h, "    if (gi < tb_np) {{ tris[gi] = {idx_ty}({}); prims[gi].cull = (tb_prim >> 31) != 0u; }}", idx.join(", "));
        h.push_str("}\n");
        Ok(h)
    }
}
