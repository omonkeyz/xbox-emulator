//! rpf-inspect <file.rpf>: print the header fields and region map of a RAGE package file.
use std::process::ExitCode;
use tombox_rpf::{inspect_prefix, Header, RegionKind, RPF7_OPEN};

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: rpf-inspect <file.rpf>");
        return ExitCode::from(2);
    };
    // Packages run to tens of gigabytes: only the first mebibyte is read and mapped.
    const PREFIX: u64 = 1 << 20;
    let (data, total) = match std::fs::File::open(&path).and_then(|f| {
        use std::io::Read;
        let total = f.metadata()?.len();
        let mut d = Vec::new();
        f.take(PREFIX).read_to_end(&mut d)?;
        Ok((d, total))
    }) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let r = match inspect_prefix(&data, total) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("{path}: RPF{}, {total} bytes{}", r.version, if total > PREFIX { " (regions below cover the first 1 MiB only)" } else { "" });
    match &r.header {
        Header::V7 { entry_count, names_length, encryption } => {
            let enc = if *encryption == RPF7_OPEN { "OPEN (table is plain)".to_string() } else { format!("{encryption:#010x} (table is encrypted)") };
            println!("  entries {entry_count}, names {names_length} bytes, encryption {enc}");
        }
        Header::V8 { entry_count, names_length, decryption_tag, platform, entries } => {
            let enc = if *decryption_tag == 0xff { "0xff (table is plain)".to_string() } else { format!("{decryption_tag:#x} (table is encrypted)") };
            println!("  entries {entry_count}, names {names_length} bytes, platform {platform:#x}, decryption tag {enc}");
            if *decryption_tag == 0xff && entries.len() < *entry_count as usize {
                println!("  showing {} of {entry_count} entries from the inspected prefix", entries.len());
            }
            for e in entries {
                println!("    {:08X} ext {:#04x}{} offset {:#x} size {:#x} compressor {} key {:#04x}",
                    e.name_hash, e.ext_id, if e.resource { " resource" } else { "" }, e.offset, e.size_on_disk, e.compressor, e.key_id);
            }
        }
        Header::V9 { words } => {
            println!("  header (0x40 bytes, plain); field meanings are not established:");
            for (i, w) in words.iter().enumerate().skip(1) {
                println!("    +{:#04x}  {w:#010x}  {w}", i * 4);
            }
        }
    }
    println!("  regions:");
    for g in &r.regions {
        let kind = match g.kind {
            RegionKind::Header => "header",
            RegionKind::Padding => "zero padding",
            RegionKind::HighEntropy => "high entropy (encrypted or compressed)",
            RegionKind::Plain => "plain data",
        };
        println!("    {:#08x}..{:#08x}  {:>8} bytes  entropy {:.2}  {kind}", g.start, g.end, g.end - g.start, g.entropy);
    }
    if r.regions.iter().skip(1).all(|g| g.kind != RegionKind::Plain) {
        println!("  nothing after the header is readable as plain data: the table of contents and the contents are encrypted.");
    }
    ExitCode::SUCCESS
}
