use super::{r, Emitter};
use crate::gds_abi::{TB_GDS_HLSL_DECL, TB_GDS_TITLE_DWORDS};
use crate::rdna2::Insn;

pub(super) fn is_gds(ins: &Insn) -> bool {
    ins.raw.first().is_some_and(|w| (w >> 17) & 1 != 0) || ins.mnemonic.starts_with("ds_gws_") || ins.mnemonic == "ds_ordered_count"
}

pub(super) fn gds_header(body: &str) -> Option<&'static str> {
    body.contains("tb_gds.").then_some(TB_GDS_HLSL_DECL)
}

impl Emitter<'_> {
    pub(super) fn emit_gds(&mut self, ins: &Insn) -> Result<(), String> {
        let m = ins.mnemonic.clone();
        if m.starts_with("ds_gws_") {
            return Err(format!("{m}: global wave sync counts hardware waves across thread groups; no lowering in the one-lane-per-thread model"));
        }
        if m == "ds_ordered_count" {
            return Err("ds_ordered_count: dispatch-ordered GDS append is not lowered".into());
        }
        let off = ins.aux.imm as u32 & 0xffff;
        let limit = TB_GDS_TITLE_DWORDS * 4;
        let t = self.tmp;
        self.tmp += 1;
        let d = ins.dst.first().copied();
        let (a, v0, v1) = (ins.src[0], ins.src[1], ins.src[2]);
        let counter = matches!(m.as_str(), "ds_append" | "ds_consume");
        // GDS M0 = { base[31:16], size[15:0] }, both in bytes (RDNA2 ISA 3.7, 10.5). The
        // address relative to the base must lie below the size: out of range is a GDS address
        // range error (3.11), with the LDS rule (10.4.3 applies LDS text to GDS): writes are
        // discarded and reads return zero (3.6).
        let rel = if counter {
            format!("{off:#x}u")
        } else {
            self.touch(&a);
            format!("({}) + {off:#x}u", r(&a)?)
        };
        self.line(&format!(
            "uint t{t}r = {rel}; uint t{t}a = ((m0 >> 16) + t{t}r) & ~3u; bool t{t}k = (exec_lo & 1u) != 0u && t{t}r < (m0 & 0xffffu) && t{t}a < {limit}u; uint t{t} = 0u;"
        ));
        let rtn = m.contains("_rtn") || counter || m == "ds_read_b32";
        let op = m.replace("_rtn", "");
        self.touch(&v0);
        self.touch(&v1);
        let (x0, x1) = (r(&v0)?, r(&v1)?);
        let body = match op.as_str() {
            "ds_append" => format!("tb_gds.InterlockedAdd(t{t}a, 1u, t{t});"),
            "ds_consume" => format!("tb_gds.InterlockedAdd(t{t}a, 0xffffffffu, t{t});"),
            "ds_read_b32" => format!("t{t} = tb_gds.Load(t{t}a);"),
            "ds_write_b32" => format!("tb_gds.Store(t{t}a, {x0});"),
            "ds_add_u32" => format!("tb_gds.InterlockedAdd(t{t}a, {x0}, t{t});"),
            "ds_sub_u32" => format!("tb_gds.InterlockedAdd(t{t}a, 0u - ({x0}), t{t});"),
            "ds_min_u32" => format!("tb_gds.InterlockedMin(t{t}a, {x0}, t{t});"),
            "ds_max_u32" => format!("tb_gds.InterlockedMax(t{t}a, {x0}, t{t});"),
            "ds_min_i32" => format!("int t{t}s; tb_gds.InterlockedMin(t{t}a, asint({x0}), t{t}s); t{t} = asuint(t{t}s);"),
            "ds_max_i32" => format!("int t{t}s; tb_gds.InterlockedMax(t{t}a, asint({x0}), t{t}s); t{t} = asuint(t{t}s);"),
            "ds_and_b32" => format!("tb_gds.InterlockedAnd(t{t}a, {x0}, t{t});"),
            "ds_or_b32" => format!("tb_gds.InterlockedOr(t{t}a, {x0}, t{t});"),
            "ds_xor_b32" => format!("tb_gds.InterlockedXor(t{t}a, {x0}, t{t});"),
            "ds_wrxchg_b32" => format!("tb_gds.InterlockedExchange(t{t}a, {x0}, t{t});"),
            "ds_cmpst_b32" => {
                format!("tb_gds.InterlockedCompareExchange(t{t}a, {x0}, {x1}, t{t});")
            }
            "ds_rsub_u32" | "ds_inc_u32" | "ds_dec_u32" => {
                let x = &x0;
                let c = format!("t{t}c");
                let new = match op.as_str() {
                    "ds_rsub_u32" => format!("({x}) - {c}"),
                    "ds_inc_u32" => format!("(({c} >= ({x})) ? 0u : ({c} + 1u))"),
                    _ => format!("((({c} == 0u) || ({c} > ({x}))) ? ({x}) : ({c} - 1u))"),
                };
                format!(
                    "uint {c} = tb_gds.Load(t{t}a); [allow_uav_condition] for (uint t{t}i = 0u; t{t}i < 4096u; t{t}i++) {{ uint t{t}p; tb_gds.InterlockedCompareExchange(t{t}a, {c}, {new}, t{t}p); if (t{t}p == {c}) break; {c} = t{t}p; }} t{t} = {c};"
                )
            }
            other => return Err(format!("{other} with GDS=1 is not lowered")),
        };
        self.line(&format!("if (t{t}k) {{ {body} }}"));
        if rtn {
            let d = d.ok_or("gds: no vdst")?;
            self.wv(&d, &format!("t{t}"))?;
        }
        Ok(())
    }
}
