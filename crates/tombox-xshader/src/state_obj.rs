
use crate::meta::{parse_rts0, RootSig};
use crate::Dxbc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationSym {
    pub name: String,
    pub code_offset: u32,
    pub flags: u32,
    pub scratch_bytes: u32,
    pub rsrc_info: u32,
}

#[derive(Debug, Clone)]
pub struct StateObjectProgram {
    pub exports: Vec<String>,
    pub root_sig_dxbc: Vec<u8>,
    pub root_sig: RootSig,
    pub pgm_rsrc: Vec<u32>,
    pub meta: Vec<u32>,
    pub isa: Vec<u8>,
    pub code_size: u32,
    pub symbols: Vec<ContinuationSym>,
}

#[derive(Debug, Clone)]
pub struct StateObjectBlob {
    pub version: u32,
    pub header_words: [u32; 8],
    pub programs: Vec<StateObjectProgram>,
}

fn rd32(b: &[u8], off: usize) -> Result<u32, String> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("state object truncated at {off:#x} (len {:#x})", b.len()))
}

fn scan_utf16_exports(slice: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= slice.len() {
        let len = u32::from_le_bytes([slice[i], slice[i + 1], slice[i + 2], slice[i + 3]]) as usize;
        if (8..=512).contains(&len) && len % 2 == 0 && i + 4 + len <= slice.len() {
            let bytes = &slice[i + 4..i + 4 + len];
            let valid = bytes.chunks_exact(2).all(|c| c[1] == 0 && (0x20..0x7f).contains(&c[0]));
            if valid {
                let s: String = bytes.chunks_exact(2).map(|c| c[0] as char).collect();
                if !out.contains(&s) {
                    out.push(s);
                }
                i += 4 + len;
                continue;
            }
        }
        i += 1;
    }
    out
}

pub fn parse_state_object(blob: &[u8]) -> Result<StateObjectBlob, String> {
    if blob.len() < 32 {
        return Err(format!("state object blob too small ({} bytes)", blob.len()));
    }
    let mut header_words = [0u32; 8];
    for (i, w) in header_words.iter_mut().enumerate() {
        *w = rd32(blob, i * 4)?;
    }
    let version = header_words[0];

    let mut dxbc_offsets = Vec::new();
    let mut pos = 0x20usize;
    while pos + 32 <= blob.len() {
        if &blob[pos..pos + 4] == b"DXBC" {
            let total = rd32(blob, pos + 24)? as usize;
            let nchunks = rd32(blob, pos + 28)?;
            if total >= 32 && nchunks >= 1 && nchunks <= 16 && pos + total <= blob.len() {
                if let Ok(dxbc) = Dxbc::parse(&blob[pos..pos + total]) {
                    if dxbc.chunk(b"RTS0").is_some() {
                        dxbc_offsets.push((pos, total));
                        pos += total;
                        continue;
                    }
                }
            }
        }
        pos += 1;
    }
    if dxbc_offsets.is_empty() {
        return Err("state object blob contains no embedded DXBC/RTS0 programs".into());
    }

    let mut programs = Vec::with_capacity(dxbc_offsets.len());
    let mut prev_end = 0x20usize;

    for &(doff, dsz) in &dxbc_offsets {
        let exports = scan_utf16_exports(&blob[prev_end..doff]);
        let dxbc_bytes = &blob[doff..doff + dsz];
        let dxbc = Dxbc::parse(dxbc_bytes)?;
        let rts = dxbc.chunk(b"RTS0").ok_or("missing RTS0 chunk")?;
        let root_sig = parse_rts0(&dxbc_bytes[rts.offset..rts.offset + rts.size])?;

        let post = doff + dsz;
        let hdr_sz = rd32(blob, post)? as usize;
        if hdr_sz % 4 != 0 || hdr_sz > 256 || post + 4 + hdr_sz > blob.len() {
            return Err(format!("invalid shader header size {hdr_sz:#x} at {post:#x}"));
        }
        let mut pgm_rsrc = Vec::with_capacity(hdr_sz / 4);
        for k in 0..hdr_sz / 4 {
            pgm_rsrc.push(rd32(blob, post + 4 + k * 4)?);
        }

        let meta_off = post + 4 + hdr_sz;
        let meta_sz = rd32(blob, meta_off)? as usize;
        if meta_sz % 4 != 0 || meta_sz > 256 || meta_off + 4 + meta_sz + 4 > blob.len() {
            return Err(format!("invalid shader metadata size {meta_sz:#x} at {meta_off:#x}"));
        }
        let mut meta = Vec::with_capacity(meta_sz / 4);
        for k in 0..meta_sz / 4 {
            meta.push(rd32(blob, meta_off + 4 + k * 4)?);
        }

        let code_len_off = meta_off + 4 + meta_sz;
        let code_len_field = rd32(blob, code_len_off)? as usize;
        if code_len_field < 0x14 {
            return Err(format!("implausible code length {code_len_field:#x} at {code_len_off:#x}"));
        }
        let code_start = code_len_off + 4;
        let padded_isa_len = code_len_field - 0x14;
        let trailer_off = code_start
            .checked_add(padded_isa_len)
            .ok_or("code length overflow")?;
        if trailer_off + 0x58 > blob.len() {
            return Err(format!("trailer at {trailer_off:#x} exceeds blob len {:#x}", blob.len()));
        }

        let n_syms = rd32(blob, trailer_off + 0x50)? as usize;
        let code_size = rd32(blob, trailer_off + 0x54)?;
        let sym_base = trailer_off + 0x58;
        if n_syms > 1024 || sym_base + n_syms * 24 > blob.len() {
            return Err(format!("invalid continuation symbol count {n_syms} at {trailer_off:#x}"));
        }

        let isa = blob[code_start..trailer_off].to_vec();

        let mut symbols = Vec::with_capacity(n_syms);
        let mut max_str_end = sym_base + n_syms * 24;
        for s in 0..n_syms {
            let so = sym_base + s * 24;
            let name_off = rd32(blob, so)? as usize;
            let _pad0 = rd32(blob, so + 4)?;
            let code_offset = rd32(blob, so + 8)?;
            let flags = rd32(blob, so + 12)?;
            let scratch_bytes = rd32(blob, so + 16)?;
            let rsrc_info = rd32(blob, so + 20)?;
            let np = sym_base + name_off;
            if np >= blob.len() {
                return Err(format!("symbol {s} name offset {name_off:#x} out of bounds"));
            }
            let ne = blob[np..].iter().position(|&c| c == 0).ok_or("unterminated symbol name")?;
            let name = String::from_utf8_lossy(&blob[np..np + ne]).into_owned();
            max_str_end = max_str_end.max((np + ne + 1 + 3) & !3);
            symbols.push(ContinuationSym { name, code_offset, flags, scratch_bytes, rsrc_info });
        }

        prev_end = max_str_end.min(blob.len());
        programs.push(StateObjectProgram {
            exports,
            root_sig_dxbc: dxbc_bytes.to_vec(),
            root_sig,
            pgm_rsrc,
            meta,
            isa,
            code_size,
            symbols,
        });
    }

    Ok(StateObjectBlob { version, header_words, programs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdna2::{disassemble, opnd_str};

    #[test]
    fn rejects_truncated_or_empty_blob() {
        assert!(parse_state_object(&[]).is_err());
        assert!(parse_state_object(&[0u8; 64]).is_err());
    }

    #[test]
    fn parses_synthetic_state_object_blob() {
        let rts0: Vec<u8> = [
            1u32, 1, 24, 0, 24, 0x0100_0000, // ver=1, npar=1, poff=24, nsamp=0, soff=24, flags
            1, 0, 36,                         // param[0]: CONST32, vis=ALL, offset=36
            0, 214_743_647, 2,               // b0, space, 2 dwords
        ].iter().flat_map(|w| w.to_le_bytes()).collect();
        let dxbc_total = (32 + 4 + 8 + rts0.len()) as u32;
        let mut dxbc = Vec::new();
        dxbc.extend_from_slice(b"DXBC");
        dxbc.extend_from_slice(&[0u8; 16]);
        dxbc.extend_from_slice(&1u32.to_le_bytes());
        dxbc.extend_from_slice(&dxbc_total.to_le_bytes());
        dxbc.extend_from_slice(&1u32.to_le_bytes());
        dxbc.extend_from_slice(&36u32.to_le_bytes());
        dxbc.extend_from_slice(b"RTS0");
        dxbc.extend_from_slice(&(rts0.len() as u32).to_le_bytes());
        dxbc.extend_from_slice(&rts0);

        let mut blob = vec![0u8; 0x20];
        blob[0..4].copy_from_slice(&2u32.to_le_bytes());
        let exp: Vec<u8> = "TestRG".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        blob.extend_from_slice(&(exp.len() as u32).to_le_bytes());
        blob.extend_from_slice(&exp);
        blob.extend_from_slice(&dxbc);
        blob.extend_from_slice(&28u32.to_le_bytes());
        blob.extend_from_slice(&[0u8; 28]);
        blob.extend_from_slice(&20u32.to_le_bytes());
        blob.extend_from_slice(&0x1234_5678u32.to_le_bytes());
        blob.extend_from_slice(&[0u8; 16]);
        let mut isa = vec![0u8; 20];
        isa.extend_from_slice(&0xbf81_0000u32.to_le_bytes());
        blob.extend_from_slice(&((isa.len() + 0x14) as u32).to_le_bytes());
        blob.extend_from_slice(&isa);
        blob.extend_from_slice(&[0u8; 0x50]);
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&4u32.to_le_bytes());
        for w in [24u32, 0, 0, 0x40001, 0, 0x18e1b] {
            blob.extend_from_slice(&w.to_le_bytes());
        }
        blob.extend_from_slice(b"main\0");

        let parsed = parse_state_object(&blob).expect("parse synthetic state object");
        assert_eq!(parsed.version, 2);
        assert_eq!(parsed.programs.len(), 1);
        let p = &parsed.programs[0];
        assert_eq!(p.exports, vec!["TestRG"]);
        assert_eq!(p.root_sig.flags, 0x0100_0000);
        assert_eq!(p.meta[0], 0x1234_5678);
        assert_eq!(p.symbols.len(), 1);
        assert_eq!(p.symbols[0].name, "main");
        assert_eq!(p.symbols[0].code_offset, 0);
    }

    #[test]
    fn optional_dump_state_object_disassembly() {
        let Ok(path) = std::env::var("TOMBOX_TEST_STATEOBJ") else { return };
        let blob = std::fs::read(&path).expect("read stateobj");
        let so = parse_state_object(&blob).expect("parse stateobj");
        eprintln!("StateObject v{} programs={}", so.version, so.programs.len());
        for (pi, prog) in so.programs.iter().enumerate() {
            eprintln!(
                "\n=== Program #{pi} exports={:?} rs_flags={:#x} params={} isa_len={:#x} code_size={:#x} ===",
                prog.exports, prog.root_sig.flags, prog.root_sig.params.len(), prog.isa.len(), prog.code_size
            );
            let base_shift = prog.isa.len().saturating_sub(prog.code_size as usize);
            eprintln!("  base_shift={base_shift:#x} prefix={:02x?}", &prog.isa[..base_shift.min(32)]);
            for sym in &prog.symbols {
                eprintln!("  SYM {:?} off={:#x} flags={:#x} scratch={:#x} rsrc={:#x}", sym.name, sym.code_offset, sym.flags, sym.scratch_bytes, sym.rsrc_info);
            }
            if pi == 3 || pi == 1 {
                for sym in &prog.symbols {
                    let start = base_shift + sym.code_offset as usize;
                    if start >= prog.isa.len() { continue; }
                    eprintln!("  --- Disasm {:?} @ isa+{start:#x} (code+{:#x}) ---", sym.name, sym.code_offset);
                    let insns = disassemble(&prog.isa[start..]);
                    for ins in insns.iter().take(120) {
                        let dsts: Vec<String> = ins.dst.iter().map(opnd_str).collect();
                        let srcs: Vec<String> = ins.src.iter().map(opnd_str).collect();
                        eprintln!("    +{:04x}: {:<24} {} {}", ins.offset, ins.mnemonic, dsts.join(", "), srcs.join(", "));
                        if ins.mnemonic == "s_endpgm" || ins.mnemonic == "s_setpc_b64" || ins.mnemonic == "s_swappc_b64" {
                            if ins.mnemonic == "s_endpgm" { break; }
                        }
                    }
                }
            }
        }
    }
}
