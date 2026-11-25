use super::{sreg_opnd, Emitter, Res, Sym, BINDLESS};
use crate::rdna2::{Enc, Insn, Opnd};
use std::collections::HashMap;

const FIELDS: [(u32, u32, u32, &str); 19] = [
    (0, 0, 31, "BASE_ADDRESS"),
    (1, 0, 7, "BASE_ADDRESS_HI"),
    (1, 8, 19, "MIN_LOD"),
    (1, 20, 28, "FORMAT"),
    (1, 29, 29, "reserved"),
    (3, 0, 11, "DST_SEL"),
    (3, 20, 24, "SW_MODE"),
    (3, 25, 27, "BC_SWIZZLE"),
    (4, 0, 12, "DEPTH"),
    (4, 13, 13, "PITCH_MSB"),
    (4, 14, 15, "reserved"),
    (4, 29, 31, "reserved"),
    (5, 8, 19, "MIN_LOD_WARN"),
    (5, 20, 22, "PERF_MOD"),
    (5, 23, 23, "CORNER_SAMPLES"),
    (5, 24, 31, "LOD_HDW_CNT_EN/PRT_DEFAULT/BIG_PAGE"),
    (6, 0, 23, "counter and compression state"),
    (6, 24, 31, "META_DATA_ADDRESS_LO"),
    (7, 0, 31, "META_DATA_ADDRESS"),
];

fn field(lo: u32, w: u32) -> u32 {
    if w == 0 || lo >= 32 {
        return 0;
    }
    let w = w.min(32 - lo);
    (((1u64 << w) - 1) << lo) as u32
}

fn konst(o: Option<&Opnd>) -> Option<u32> {
    match o {
        Some(Opnd::ConstInt(v)) => Some(*v as u32),
        Some(Opnd::Literal(v)) => Some(*v),
        _ => None,
    }
}

fn used_bits(ins: &Insn, pos: usize) -> u32 {
    let c = |i: usize| konst(ins.src.get(i));
    let bits = match ins.mnemonic.as_str() {
        "s_bfe_u32" | "s_bfe_i32" if pos == 0 => c(1).map(|f| field(f & 31, (f >> 16) & 0x7f)),
        "v_bfe_u32" | "v_bfe_i32" if pos == 0 => c(1).zip(c(2)).map(|(o, w)| field(o & 31, w & 31)),
        "s_and_b32" | "v_and_b32" if pos < 2 => c(1 - pos),
        "s_lshr_b32" if pos == 0 => c(1).map(|s| !0u32 << (s & 31)),
        "v_lshrrev_b32" if pos == 1 => c(0).map(|s| !0u32 << (s & 31)),
        _ => None,
    };
    bits.unwrap_or(!0)
}

fn known_bits(k: u32, dim: u32) -> u32 {
    match k {
        1 => 0xc000_0000,
        2 => 0xffff_ffff,
        3 => 0xf00f_f000,
        4 if dim == 3 => 0,
        4 => 0x1fff_1fff,
        5 => 0x0000_00ff,
        _ => 0,
    }
}

fn img_type(dim: u32) -> u32 {
    [8, 9, 10, 11, 12, 13, 14][dim.min(6) as usize]
}

impl<'a> Emitter<'a> {
    fn desc_words_on(&self) -> bool {
        self.durango.is_none() && !crate::gcn::is_durango(&self.ex.fssm)
    }

    pub(super) fn note_desc_reads(&mut self, ins: &Insn, sym: &HashMap<u16, Sym>) {
        if !self.desc_words_on() || matches!(ins.enc, Enc::Smem | Enc::Mubuf | Enc::Mtbuf | Enc::Mimg) {
            return;
        }
        if matches!(ins.mnemonic.as_str(), "s_mov_b32" | "s_mov_b64" | "s_cmov_b32" | "s_cmov_b64") {
            return;
        }
        let wide = ins.mnemonic.ends_with("_b64") || ins.mnemonic.ends_with("_u64") || ins.mnemonic.ends_with("_i64");
        let dsts: Vec<u8> = ins.dst.iter().filter_map(|o| if let Opnd::Sgpr(n) = o { Some(*n) } else { None }).collect();
        for (pos, o) in ins.src.iter().enumerate() {
            let Opnd::Sgpr(base) = o else { continue };
            for h in 0..if wide { 2u8 } else { 1 } {
                let reg = base.wrapping_add(h);
                if dsts.iter().any(|&d| d == reg || (wide && d.wrapping_add(1) == reg)) {
                    continue;
                }
                let Some(Sym::Desc { table, off, k }) = sym.get(&(reg as u16)).copied() else { continue };
                if off & BINDLESS != 0 {
                    continue;
                }
                let mask = if wide { !0 } else { used_bits(ins, pos) };
                let e = self.desc_reads.entry((table, off)).or_default().entry(k).or_insert((0, ins.offset));
                e.0 |= mask;
            }
        }
    }

    pub(super) fn note_img_desc(&mut self, table: u8, off: u32, res: Res) {
        if off & BINDLESS == 0 {
            self.img_desc.insert((table, off), res);
        }
    }

    pub(super) fn emit_desc_words(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        let Some(&(d, id)) = self.dsel_loads.get(&idx) else { return Ok(()) };
        let key = ((id >> 24) as u8, id & 0x00ff_ffff);
        let Some(reads) = self.desc_reads.get(&key).cloned() else { return Ok(()) };
        let n = 1u32 << ins.op;
        let reads: Vec<(u32, u32, usize)> = reads.into_iter().filter(|(k, _)| *k < n).map(|(k, (m, at))| (k, m, at)).collect();
        if reads.is_empty() {
            return Ok(());
        }
        let (slot, dim, uav) = match self.img_desc.get(&key).copied() {
            Some(Res::Tex(s, dm)) => (s, dm, false),
            Some(Res::Uav(s, dm)) => (s, dm, true),
            _ => return Ok(()),
        };
        for &(k, m, at) in &reads {
            let bad = m & !known_bits(k, dim);
            if bad != 0 {
                let names: Vec<&str> = FIELDS.iter().filter(|f| f.0 == k && field(f.1, f.2 - f.1 + 1) & bad != 0).map(|f| f.3).collect();
                return Err(format!(
                    "instruction at {at:#x} reads bits {bad:#010x} of image descriptor dword {k} (table s{} offset {:#x}), which the PC view does not carry ({}); only the size, level, layer and type fields are rebuilt",
                    key.0,
                    key.1,
                    names.join(", ")
                ));
            }
        }
        let t = self.tmp;
        self.tmp += 1;
        let obj = if uav { format!("uav{slot}") } else { format!("tex{slot}") };
        self.line(&format!("uint t{t}w = 1u, t{t}h = 1u, t{t}d = 1u, t{t}l = 1u;"));
        let q = match (dim, uav) {
            (0, false) => format!("{obj}.GetDimensions(0u, t{t}w, t{t}l);"),
            (1, false) | (3, false) => format!("{obj}.GetDimensions(0u, t{t}w, t{t}h, t{t}l);"),
            (2, false) | (5, false) => format!("{obj}.GetDimensions(0u, t{t}w, t{t}h, t{t}d, t{t}l);"),
            (4, false) => format!("{obj}.GetDimensions(0u, t{t}w, t{t}d, t{t}l);"),
            (6, false) => format!("{obj}.GetDimensions(t{t}w, t{t}h, t{t}l);"),
            (0, true) => format!("{obj}.GetDimensions(t{t}w);"),
            (1, true) => format!("{obj}.GetDimensions(t{t}w, t{t}h);"),
            (2, true) | (5, true) => format!("{obj}.GetDimensions(t{t}w, t{t}h, t{t}d);"),
            (4, true) => format!("{obj}.GetDimensions(t{t}w, t{t}d);"),
            _ => return Err(format!("image descriptor words of a dim {dim} image read by ALU are not rebuilt")),
        };
        self.line(&q);
        let last = if dim == 6 { format!("firstbithigh(t{t}l)") } else { format!("(t{t}l - 1u)") };
        let depth = if matches!(dim, 2 | 4 | 5) { format!("(t{t}d - 1u)") } else { "0u".to_string() };
        let pitch = if dim == 2 && uav { 1u32 } else { 0 };
        for (k, _, _) in reads {
            let e = match k {
                1 => format!("(((t{t}w - 1u) & 3u) << 30)"),
                2 => format!("(((t{t}w - 1u) >> 2) | ((t{t}h - 1u) << 14) | 0x80000000u)"),
                3 => format!("(({last} << 16) | {:#x}u)", img_type(dim) << 28),
                4 => format!("({depth} & 0x1fffu)"),
                5 => format!("(({last} << 4) | {pitch}u)"),
                _ => continue,
            };
            let dst = sreg_opnd(d as u8 + k as u8);
            self.touch(&dst);
            self.ws(&dst, &e)?;
        }
        Ok(())
    }
}
