use super::{r, Emitter};
use crate::rdna2::{Insn, Opnd};

pub(super) fn is_wide_lds(m: &str) -> bool {
    matches!(m, "ds_write_b64" | "ds_write2_b64" | "ds_write2st64_b64" | "ds_read_b64" | "ds_read2_b64" | "ds_read2st64_b64")
}

fn vg(o: &Opnd, k: u8) -> Result<Opnd, String> {
    match o {
        Opnd::Vgpr(n) => Ok(Opnd::Vgpr(n + k)),
        _ => Err("64-bit LDS access with a non-VGPR operand".into()),
    }
}

fn part(ins: &Insn, m: &str, op: u32, off: u32, src: Vec<Opnd>, dst: Vec<Opnd>) -> Insn {
    let mut p = ins.clone();
    p.mnemonic = m.to_string();
    p.op = op;
    p.aux.imm = off as i64;
    p.src = src;
    p.dst = dst;
    p
}

pub(super) fn split_wide_lds(ins: &Insn) -> Result<Vec<Insn>, String> {
    let m = ins.mnemonic.as_str();
    let off = ins.aux.imm as u32 & 0xffff;
    let (o0, o1) = (off & 0xff, (off >> 8) & 0xff);
    let scale = if m.contains("st64") { 512 } else { 8 };
    let bytes: Vec<u32> = match m {
        "ds_write_b64" | "ds_read_b64" => vec![off, off + 4],
        _ => vec![o0 * scale, o0 * scale + 4, o1 * scale, o1 * scale + 4],
    };
    if bytes.iter().any(|&b| b > 0xffff) {
        return Err(format!("{m}: offset {off:#x} reaches past the 16-bit DS offset"));
    }
    let addr = ins.src[0];
    if m.starts_with("ds_write") {
        let data: Vec<Opnd> = match m {
            "ds_write_b64" => vec![ins.src[1], vg(&ins.src[1], 1)?],
            _ => vec![ins.src[1], vg(&ins.src[1], 1)?, ins.src[2], vg(&ins.src[2], 1)?],
        };
        return Ok(bytes.iter().zip(data).map(|(&b, d)| part(ins, "ds_write_b32", 0x0d, b, vec![addr, d, Opnd::Vgpr(0)], vec![Opnd::Vgpr(0)])).collect());
    }
    let vd = match ins.dst.first() {
        Some(Opnd::Vgpr(v)) => *v,
        _ => return Err(format!("{m}: bad vdst")),
    };
    let mut order: Vec<usize> = (0..bytes.len()).collect();
    if let Opnd::Vgpr(a) = addr {
        if let Some(k) = order.iter().position(|&k| vd + k as u8 == a) {
            let x = order.remove(k);
            order.push(x);
        }
    }
    Ok(order
        .into_iter()
        .map(|k| part(ins, "ds_read_b32", 0x36, bytes[k], vec![addr, Opnd::Vgpr(0), Opnd::Vgpr(0)], vec![Opnd::Vgpr(vd + k as u8)]))
        .collect())
}

impl Emitter<'_> {
    pub(super) fn cs_threads(&self) -> (u32, u32, u32) {
        let p = self.ex.isa_preamble;
        let [x, y, z] = self.ex.compute_threads.unwrap_or([p[1].max(1), p[2].max(1), p[3].max(1)]);
        (x, y, z)
    }

    pub(super) fn xlane_header(&self) -> Result<Option<String>, String> {
        if !self.is_cs || !self.body.contains("tb_xl[") {
            return Ok(None);
        }
        if self.uses_lds {
            return Err("ds_permute/ds_bpermute in a compute shader that also uses LDS: the 32-lane exchange needs groupshared space beyond the 32 KiB LDS mirror".into());
        }
        let (x, y, z) = self.cs_threads();
        let n = x * y * z;
        Ok(Some(format!("#define TB_XL_GS 1\ngroupshared uint tb_xl[{n}];\ngroupshared uint tb_xm[{n}];\ngroupshared uint tb_xa[{n}];")))
    }

    pub(super) fn emit_ds_permute(&mut self, ins: &Insn) -> Result<(), String> {
        self.uses_wave = true;
        let d = *ins.dst.first().ok_or("permute: no vdst")?;
        let (addr, data) = (ins.src[0], ins.src[1]);
        self.touch(&addr);
        self.touch(&data);
        let off = ins.aux.imm as u32 & 0xffff;
        let back = ins.mnemonic == "ds_bpermute_b32";
        let t = self.tmp;
        self.tmp += 1;
        self.line(&format!("uint t{t}m = ((({}) + {off:#x}u) >> 2) & 31u; uint t{t}d = {}; uint t{t};", r(&addr)?, r(&data)?));
        let shared = self.is_cs && self.brace_depth == 0 && self.loop_depth == 0;
        if shared {
            let (x, y, z) = self.cs_threads();
            let n = x * y * z;
            self.line("#ifdef TB_XL_GS");
            self.line(&format!("{{ uint t{t}f = gtid.x + gtid.y * {x}u + gtid.z * {}u; uint t{t}g = t{t}f & ~31u; uint t{t}j = t{t}f & 31u;", x * y));
            self.line("GroupMemoryBarrierWithGroupSync();");
            self.line(&format!("tb_xl[t{t}f] = t{t}d; tb_xm[t{t}f] = t{t}m; tb_xa[t{t}f] = exec_lo & 1u;"));
            self.line("GroupMemoryBarrierWithGroupSync();");
            if back {
                self.line(&format!("uint t{t}i = t{t}g + t{t}m; t{t} = (t{t}i < {n}u && tb_xa[min(t{t}i, {}u)] != 0u) ? tb_xl[min(t{t}i, {}u)] : 0u; }}", n - 1, n - 1));
            } else {
                self.line(&format!(
                    "t{t} = 0u; for (uint t{t}k = 0u; t{t}k < 32u; t{t}k++) {{ uint t{t}i = min(t{t}g + t{t}k, {}u); if (t{t}g + t{t}k < {n}u && tb_xa[t{t}i] != 0u && tb_xm[t{t}i] == t{t}j) t{t} = tb_xl[t{t}i]; }} }}",
                    n - 1
                ));
            }
            self.line("#else");
        }
        self.line(&format!("{{ uint t{t}l = WaveGetLaneIndex(); uint t{t}g = t{t}l & ~31u;"));
        if back {
            self.line(&format!("uint t{t}v = WaveReadLaneAt(t{t}d, t{t}g + t{t}m); t{t} = (WaveReadLaneAt(exec_lo & 1u, t{t}g + t{t}m) != 0u) ? t{t}v : 0u; }}"));
        } else {
            self.line(&format!(
                "t{t} = 0u; for (uint t{t}k = 0u; t{t}k < 32u; t{t}k++) {{ uint t{t}x = WaveReadLaneAt(t{t}m, t{t}g + t{t}k); uint t{t}y = WaveReadLaneAt(t{t}d, t{t}g + t{t}k); bool t{t}a = WaveReadLaneAt(exec_lo & 1u, t{t}g + t{t}k) != 0u; if (t{t}a && t{t}x == (t{t}l & 31u)) t{t} = t{t}y; }} }}"
            ));
        }
        if shared {
            self.line("#endif");
        }
        self.wv(&d, &format!("t{t}"))
    }
}
