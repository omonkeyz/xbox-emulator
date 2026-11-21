//! disasm — disassemble a range of a PE by RVA and resolve direct call/branch
//! targets to UE symbol names (from the decompressed `-Symbols.bin`).
//!
//! Usage: disasm <pe> <rva_hex> [len] [--sym <symfile>] [--follow]

use iced_x86::{Decoder, DecoderOptions, FlowControl, Formatter, Instruction, IntelFormatter, OpKind};
use std::collections::VecDeque;

#[derive(Debug)]
struct Sym {
    rva: u32,
    size: u32,
    name: String,
}

fn read_u32(b: &[u8], o: usize) -> Option<u32> {
    let end = o.checked_add(4)?;
    let s = b.get(o..end)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn parse_syms(b: &[u8]) -> Vec<Sym> {
    let Some(count) = read_u32(b, 0x2c).map(|v| v as usize) else {
        return Vec::new();
    };
    let rec_base = 0x30usize;
    let Some(rec_end) = count.checked_mul(12).and_then(|n| rec_base.checked_add(n)) else {
        return Vec::new();
    };
    if rec_end > b.len() {
        return Vec::new();
    }
    let mut syms: Vec<Sym> = Vec::with_capacity(count);
    for i in 0..count {
        let o = rec_base + i * 12;
        let (Some(rva), Some(size)) = (read_u32(b, o), read_u32(b, o + 4)) else {
            return syms;
        };
        syms.push(Sym { rva, size, name: String::new() });
    }
    // strings: skip repeated count u32, then count length-prefixed strings
    let mut so = rec_end;
    if read_u32(b, so).is_none() {
        return syms;
    }
    so += 4;
    for s in syms.iter_mut() {
        let Some(len) = read_u32(b, so).map(|v| v as usize) else {
            break;
        };
        so += 4;
        if len == 0 {
            break;
        }
        let Some(end) = so.checked_add(len) else {
            break;
        };
        if end > b.len() {
            break;
        }
        s.name = String::from_utf8_lossy(&b[so..end - 1]).into_owned();
        so = end;
    }
    syms.sort_by_key(|s| s.rva);
    syms
}

fn load_syms(path: &str) -> Vec<Sym> {
    let b = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("warning: cannot read symbols {path}: {e}");
            return Vec::new();
        }
    };
    parse_syms(&b)
}

fn resolve<'a>(syms: &'a [Sym], rva: u32) -> Option<(String, u32)> {
    if syms.is_empty() {
        return None;
    }
    let idx = match syms.binary_search_by(|s| s.rva.cmp(&rva)) {
        Ok(i) => i,
        Err(0) => return None,
        Err(i) => i - 1,
    };
    let s = &syms[idx];
    let off = rva - s.rva;
    let span = if s.size > 0 { s.size } else { 0x40 };
    if off < span || off < 0x800 {
        Some((s.name.clone(), off))
    } else {
        None
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: disasm <pe> <rva_hex> [len] [--sym <symfile>] [--follow]");
        std::process::exit(2);
    }
    let pe_path = &args[1];
    let start_rva = u32::from_str_radix(args[2].trim_start_matches("0x"), 16).expect("rva hex");
    let mut len = 0x200usize;
    let mut symfile = None;
    let mut follow = false;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--sym" => { i += 1; symfile = args.get(i).cloned(); }
            "--follow" => follow = true,
            s => { if let Ok(n) = usize::from_str_radix(s.trim_start_matches("0x"), 16) { len = n; } }
        }
        i += 1;
    }

    let data = std::fs::read(pe_path).expect("read pe");
    let pe = tombox_pe::PeFile::parse(&data).expect("parse pe");
    let image_base = pe.opt.image_base;
    let syms = symfile.as_deref().map(load_syms).unwrap_or_default();
    if !syms.is_empty() {
        eprintln!("[loaded {} symbols]", syms.len());
    }

    // Disassemble one range starting at rva; with --follow, also queue direct
    // call targets (one level) so we can see the functions WinMain invokes.
    let mut queue: VecDeque<(u32, usize)> = VecDeque::new();
    queue.push_back((start_rva, len));
    let mut done = std::collections::HashSet::new();

    while let Some((rva, length)) = queue.pop_front() {
        if !done.insert(rva) {
            continue;
        }
        let off = match pe.rva_to_offset(rva) {
            Ok(o) => o,
            Err(e) => { eprintln!("rva {rva:#x}: {e}"); continue; }
        };
        let end = (off + length).min(data.len());
        let bytes = &data[off..end];
        let name = resolve(&syms, rva).map(|(n, o)| format!("{n}+{o:#x}")).unwrap_or_default();
        println!("\n==== {:#x} (rva {:#x}) {} ====", image_base + rva as u64, rva, name);

        let mut decoder = Decoder::with_ip(64, bytes, image_base + rva as u64, DecoderOptions::NONE);
        let mut fmt = IntelFormatter::new();
        let mut instr = Instruction::default();
        let mut text = String::new();
        while decoder.can_decode() {
            decoder.decode_out(&mut instr);
            text.clear();
            fmt.format(&instr, &mut text);
            let va = instr.ip();
            let irva = (va - image_base) as u32;

            // Resolve a direct call/branch target to a symbol.
            let mut annot = String::new();
            let fc = instr.flow_control();
            if matches!(fc, FlowControl::Call | FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch)
                && instr.op0_kind() == OpKind::NearBranch64
            {
                let tgt = instr.near_branch_target();
                let trva = (tgt - image_base) as u32;
                if let Some((n, o)) = resolve(&syms, trva) {
                    annot = format!("   ; -> {n}{}", if o == 0 { String::new() } else { format!("+{o:#x}") });
                }
                if follow && fc == FlowControl::Call && !done.contains(&trva) {
                    queue.push_back((trva, 0x120));
                }
            }
            println!("  {:#011x} {}{}", irva, text, annot);
            if instr.flow_control() == FlowControl::Return {
                // keep going a little past the first ret to catch tails
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym_file(records: &[(u32, u32)], names: &[&str]) -> Vec<u8> {
        assert_eq!(records.len(), names.len());
        let count = records.len() as u32;
        let mut b = vec![0u8; 0x30];
        b[0x2c..0x30].copy_from_slice(&count.to_le_bytes());
        for (rva, size) in records {
            b.extend_from_slice(&rva.to_le_bytes());
            b.extend_from_slice(&size.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        b.extend_from_slice(&count.to_le_bytes());
        for name in names {
            let mut s = name.as_bytes().to_vec();
            s.push(0);
            b.extend_from_slice(&(s.len() as u32).to_le_bytes());
            b.extend_from_slice(&s);
        }
        b
    }

    #[test]
    fn parses_and_sorts_symbols() {
        let b = sym_file(&[(0x2000, 0x20), (0x1000, 0x10)], &["second", "first"]);
        let v = parse_syms(&b);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].rva, v[0].size, v[0].name.as_str()), (0x1000, 0x10, "first"));
        assert_eq!((v[1].rva, v[1].size, v[1].name.as_str()), (0x2000, 0x20, "second"));
    }

    #[test]
    fn short_header_is_empty() {
        assert!(parse_syms(&[1, 2, 3, 4]).is_empty());
        assert!(parse_syms(&[0u8; 0x2c]).is_empty());
    }

    #[test]
    fn records_past_eof_are_empty() {
        let mut b = vec![0u8; 0x30];
        b[0x2c..0x30].copy_from_slice(&2u32.to_le_bytes());
        assert!(parse_syms(&b).is_empty());
    }

    #[test]
    fn huge_count_is_empty() {
        let mut b = vec![0u8; 0x30];
        b[0x2c..0x30].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_syms(&b).is_empty());
    }

    #[test]
    fn bad_string_lengths_keep_records_without_names() {
        // rec_end = 0x30 + 12 = 0x3c; repeated count at 0x3c; first
        // length-prefixed string at 0x40.
        let str_len = 0x30 + 12 + 4;
        for bad in [0u32, u32::MAX] {
            let mut b = sym_file(&[(0x1000, 0x10)], &["name"]);
            b[str_len..str_len + 4].copy_from_slice(&bad.to_le_bytes());
            let v = parse_syms(&b);
            assert_eq!(v.len(), 1, "bad length {bad:#x}");
            assert_eq!(v[0].rva, 0x1000);
            assert_eq!(v[0].name, "");
        }
    }
}
