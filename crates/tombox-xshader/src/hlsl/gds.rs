use super::{mesh, r, Emitter};
use crate::gds_abi::{TB_GDS_HLSL_DECL, TB_GDS_TITLE_DWORDS};
use crate::rdna2::Insn;
use crate::Stage;
use std::fmt::Write as _;

pub(super) fn is_gds(ins: &Insn) -> bool {
    ins.raw.first().is_some_and(|w| (w >> 17) & 1 != 0) || ins.mnemonic.starts_with("ds_gws_") || ins.mnemonic == "ds_ordered_count"
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CounterMode {
    GroupShared,
    NativeWave,
}

// Each group of 32 guest lanes shares one EXEC mask and one pre-op value,
// even when the host schedules those lanes in several waves.
const SHARED_COUNTER: &str = r#"
groupshared uint tb_gds_mask[TB_GDS_WAVES];
groupshared uint tb_gds_previous[TB_GDS_WAVES];
uint tb_gds_counter(uint index, uint address, bool enabled, uint step) {
    uint wave = index / 32u, lane = index % 32u;
    if (lane == 0u) tb_gds_mask[wave] = 0u;
    GroupMemoryBarrierWithGroupSync();
    if (enabled) InterlockedOr(tb_gds_mask[wave], 1u << lane);
    GroupMemoryBarrierWithGroupSync();
    uint mask = tb_gds_mask[wave];
    if (enabled && lane == firstbitlow(mask)) {
        uint previous;
        tb_gds.InterlockedAdd(address, countbits(mask) * step, previous);
        tb_gds_previous[wave] = previous;
    }
    GroupMemoryBarrierWithGroupSync();
    return enabled ? tb_gds_previous[wave] : 0u;
}
"#;

const NATIVE_COUNTER: &str = r#"
uint tb_gds_counter(uint index, uint address, bool enabled, uint step) {
    uint previous = 0u;
    uint count = WaveActiveCountBits(enabled);
    if (enabled) {
        if (WaveIsFirstLane()) tb_gds.InterlockedAdd(address, count * step, previous);
        previous = WaveReadLaneFirst(previous);
    }
    return previous;
}
"#;

impl Emitter<'_> {
    /// Returns true when the compute entry must request a native wave32.
    pub(super) fn write_gds_header(&mut self, h: &mut String) -> Result<bool, String> {
        if self.gds_counter.is_none() && !self.body.contains("tb_gds.") {
            return Ok(false);
        }
        let _ = writeln!(h, "{TB_GDS_HLSL_DECL}");
        let Some(mode) = self.gds_counter else { return Ok(false) };
        // The LDS mirror takes all 32 KiB. A divergent counter cannot use a
        // barrier that every thread in the group must reach, either.
        if mode == CounterMode::NativeWave || self.uses_lds {
            h.push_str(NATIVE_COUNTER);
            self.uses_wave = true;
            if self.ex.stage == Stage::Compute {
                self.sm6_minor = self.sm6_minor.max(6);
                return Ok(true);
            }
        } else {
            let threads = self.ex.isa_preamble[1..].iter().map(|&n| n.max(1))
                .try_fold(1u32, u32::checked_mul).filter(|&n| n <= 1024)
                .ok_or("GDS counter: invalid compute thread-group size")?;
            let _ = writeln!(h, "#define TB_GDS_WAVES {}u", threads.div_ceil(32));
            h.push_str(SHARED_COUNTER);
        }
        Ok(false)
    }

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
        if counter && mesh::wave_size(&self.insns) == 64 {
            return Err(format!("{m}: GDS counter translation currently supports wave32 only"));
        }
        let rel = if counter {
            format!("{off:#x}u")
        } else {
            self.touch(&a);
            format!("({}) + {off:#x}u", r(&a)?)
        };
        self.line(&format!(
            "uint t{t}r = {rel}; uint t{t}a = ((m0 >> 16) + t{t}r) & ~3u; bool t{t}k = (exec_lo & 1u) != 0u && t{t}r < (m0 & 0xffffu) && t{t}a < {limit}u; uint t{t} = 0u;"
        ));
        if counter {
            let compute = self.ex.stage == Stage::Compute;
            if compute && self.brace_depth == 0 && self.loop_depth == 0 {
                self.gds_counter.get_or_insert(CounterMode::GroupShared);
            } else {
                self.gds_counter = Some(CounterMode::NativeWave);
            }
            let index = if compute { "tb_gi" } else { "0u" };
            let step = if m == "ds_append" { "1u" } else { "0xffffffffu" };
            self.line(&format!("t{t} = tb_gds_counter({index}, t{t}a, t{t}k, {step});"));
            return self.wv(&d.ok_or("gds: no vdst")?, &format!("t{t}"));
        }
        let rtn = m.contains("_rtn") || m == "ds_read_b32";
        let op = m.replace("_rtn", "");
        self.touch(&v0);
        self.touch(&v1);
        let (x0, x1) = (r(&v0)?, r(&v1)?);
        let body = match op.as_str() {
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
