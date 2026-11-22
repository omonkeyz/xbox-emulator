//! Inspector for RAGE package files (`.rpf`).
//!
//! What is established and what is not:
//!
//! * **RPF7** (documented by the modding community for an earlier RAGE title): a 16-byte
//!   header `magic "RPF7" (stored little-endian, so the bytes read "7FPR"), entry count,
//!   names length, encryption tag`, followed by 16-byte entries and the name table. The tag
//!   is `OPEN` (plain), or one of two values meaning the table is encrypted.
//! * **RPF8** (layout as read by the public-domain `rpf-archive-rs` crate, not checked here
//!   against a real file): `magic, entry count, names length, u16 decryption tag, u16
//!   platform`, a 256-byte signature, then 24-byte entries from offset 272. A tag of 0xFF
//!   means the table is plain; anything else means it is encrypted. Plain entries are decoded.
//! * **RPF9**: observed in exactly one file (8,880 bytes). Its first 0x40 bytes are plain;
//!   everything after is either high-entropy data or zero padding. The header fields are
//!   reported by offset. Names given to them below are guesses and are marked as such.
//!
//! This crate reads structure only. Package contents are encrypted with keys held by the
//! title, and recovering or applying those keys is outside this project's scope: the title
//! decrypts its own packages when it runs.
//!
//! Input is untrusted: every read is bounds-checked and nothing is allocated from sizes found
//! in the file.

/// A contiguous part of the file with one character.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
    pub kind: RegionKind,
    /// Shannon entropy in bits per byte (0 for an empty region).
    pub entropy: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Header,
    /// All zero bytes.
    Padding,
    /// Entropy of 7.2 bits/byte or more over the region: encrypted or compressed.
    HighEntropy,
    /// Anything else: plain structured data.
    Plain,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Header {
    V7 { entry_count: u32, names_length: u32, encryption: u32 },
    V8 { entry_count: u32, names_length: u32, decryption_tag: u16, platform: u16, entries: Vec<Entry8> },
    /// The sixteen little-endian dwords of the 0x40-byte header, verbatim.
    V9 { words: [u32; 16] },
}

/// One RPF8 table entry (only decoded when the table is plain).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry8 {
    pub name_hash: u32,
    /// 0xFF = not encrypted.
    pub key_id: u8,
    /// 0xFE = directory.
    pub ext_id: u8,
    pub resource: bool,
    pub offset: u64,
    pub size_on_disk: u64,
    /// 0 = stored.
    pub compressor: u8,
    pub last_qword: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rpf {
    pub version: u8,
    pub size: usize,
    pub header: Header,
    pub regions: Vec<Region>,
}

pub const RPF7_OPEN: u32 = 0x4e45_504f; // "OPEN"

fn u32_at(b: &[u8], o: usize) -> Result<u32, String> {
    let s = b.get(o..o + 4).ok_or_else(|| format!("truncated: need 4 bytes at {o:#x}, file is {:#x}", b.len()))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn entropy(b: &[u8]) -> f64 {
    if b.is_empty() {
        return 0.0;
    }
    let mut c = [0usize; 256];
    for &x in b {
        c[x as usize] += 1;
    }
    let n = b.len() as f64;
    let e: f64 = c.iter().filter(|&&v| v > 0).map(|&v| { let p = v as f64 / n; -p * p.log2() }).sum();
    e.max(0.0)
}

/// Split `b[from..]` into zero padding and non-zero runs. A zero run counts as padding only
/// from 64 bytes up, so zeros inside data do not split it.
fn map_regions(b: &[u8], from: usize) -> Vec<Region> {
    const MIN_PAD: usize = 64;
    let mut out = Vec::new();
    let mut i = from;
    let mut run_start = from;
    while i < b.len() {
        if b[i] == 0 {
            let z = i;
            while i < b.len() && b[i] == 0 {
                i += 1;
            }
            if i - z >= MIN_PAD {
                if z > run_start {
                    out.push((run_start, z, false));
                }
                out.push((z, i, true));
                run_start = i;
            }
        } else {
            i += 1;
        }
    }
    if b.len() > run_start {
        out.push((run_start, b.len(), false));
    }
    out.into_iter()
        .map(|(start, end, pad)| {
            let e = entropy(&b[start..end]);
            // A short region cannot reach 8 bits/byte: scale the bar by what its length allows.
            let ceiling = ((end - start) as f64).log2().min(8.0);
            let kind = if pad { RegionKind::Padding } else if e >= 0.9 * ceiling { RegionKind::HighEntropy } else { RegionKind::Plain };
            Region { start, end, kind, entropy: e }
        })
        .collect()
}

/// Parse the header and map the rest of the file.
pub fn inspect(b: &[u8]) -> Result<Rpf, String> {
    inspect_prefix(b, b.len() as u64)
}

/// Inspect a leading slice of an archive with a known total size.
/// Only complete plain RPF8 entries within the slice are returned.
pub fn inspect_prefix(b: &[u8], file_size: u64) -> Result<Rpf, String> {
    if b.len() as u64 > file_size {
        return Err("inspected bytes exceed the archive size".into());
    }
    let magic = u32_at(b, 0)?;
    if magic >> 8 != 0x0052_5046 {
        return Err(format!("not an RPF file: magic {magic:#010x} (expected \"RPF\" + version digit)"));
    }
    let digit = (magic & 0xff) as u8;
    let (header, header_len) = match digit {
        b'7' => (Header::V7 { entry_count: u32_at(b, 4)?, names_length: u32_at(b, 8)?, encryption: u32_at(b, 12)? }, 16),
        b'8' => {
            let (entry_count, tag) = (u32_at(b, 4)?, u32_at(b, 12)?);
            let mut entries = Vec::new();
            if tag as u16 == 0xff {
                const TABLE_START: usize = 272;
                const ENTRY_SIZE: usize = 24;
                let table_end = TABLE_START as u64 + u64::from(entry_count) * ENTRY_SIZE as u64;
                if entry_count != 0 && table_end > file_size {
                    return Err(format!("truncated: RPF8 table ends at {table_end:#x}, file size is {file_size:#x}"));
                }
                let available = b.get(TABLE_START..).unwrap_or_default();
                for e in available.chunks_exact(ENTRY_SIZE).take(entry_count as usize) {
                    let q = |k: usize| u64::from_le_bytes(e[k..k + 8].try_into().unwrap());
                    let (q0, q1) = (q(0), q(8));
                    entries.push(Entry8 {
                        name_hash: q0 as u32,
                        key_id: (q0 >> 40) as u8,
                        ext_id: (q0 >> 48) as u8,
                        resource: (q0 >> 56) & 1 != 0,
                        size_on_disk: (q1 & 0xfff_ffff) << 4,
                        offset: ((q1 >> 28) & 0x7fff_ffff) << 4,
                        compressor: (q1 >> 59) as u8,
                        last_qword: q(16),
                    });
                }
            }
            (Header::V8 { entry_count, names_length: u32_at(b, 8)?, decryption_tag: tag as u16, platform: (tag >> 16) as u16, entries }, 272.min(b.len()))
        }
        b'9' => {
            let mut words = [0u32; 16];
            for (i, w) in words.iter_mut().enumerate() {
                *w = u32_at(b, i * 4)?;
            }
            (Header::V9 { words }, 0x40)
        }
        d if d.is_ascii_digit() => return Err(format!("RPF version {} is not supported (known: 7, 8, 9)", d as char)),
        d => return Err(format!("not an RPF file: version byte {d:#04x}")),
    };
    let mut regions = vec![Region { start: 0, end: header_len, kind: RegionKind::Header, entropy: entropy(&b[..header_len]) }];
    regions.extend(map_regions(b, header_len));
    Ok(Rpf { version: digit - b'0', size: b.len(), header, regions })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, mut s: u32) -> Vec<u8> {
        (0..n).map(|_| { s ^= s << 13; s ^= s >> 17; s ^= s << 5; (s >> 8) as u8 }).collect()
    }

    #[test]
    fn rpf7_open_header() {
        let mut b = Vec::new();
        for w in [0x5250_4637u32, 3, 40, RPF7_OPEN] {
            b.extend_from_slice(&w.to_le_bytes());
        }
        b.extend_from_slice(&[1u8; 48]);
        let r = inspect(&b).unwrap();
        assert_eq!(r.version, 7);
        assert_eq!(r.header, Header::V7 { entry_count: 3, names_length: 40, encryption: RPF7_OPEN });
        assert_eq!(r.regions[1].kind, RegionKind::Plain);
    }

    #[test]
    fn rpf9_regions_are_mapped() {
        let mut b = vec![0u8; 0x40];
        b[..4].copy_from_slice(&0x5250_4639u32.to_le_bytes());
        b[4..8].copy_from_slice(&0x360u32.to_le_bytes());
        b.extend(noise(0x510, 7));
        b.resize(0x1000, 0);
        b.extend(noise(0x12b0, 99));
        let r = inspect(&b).unwrap();
        assert_eq!(r.version, 9);
        let kinds: Vec<_> = r.regions.iter().map(|x| (x.start, x.end, x.kind)).collect();
        assert_eq!(kinds, vec![
            (0, 0x40, RegionKind::Header),
            (0x40, 0x550, RegionKind::HighEntropy),
            (0x550, 0x1000, RegionKind::Padding),
            (0x1000, 0x22b0, RegionKind::HighEntropy),
        ]);
        match r.header {
            Header::V9 { words } => assert_eq!(words[1], 0x360),
            _ => panic!(),
        }
    }

    #[test]
    fn rpf8_plain_table_is_decoded_and_encrypted_one_is_not() {
        let mut b = vec![0u8; 272];
        b[..4].copy_from_slice(&0x5250_4638u32.to_le_bytes());
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        b[12..14].copy_from_slice(&0xffu16.to_le_bytes());
        let q0: u64 = 0xdead_beef | (0xff << 40) | (0x07 << 48) | (1 << 56);
        let q1: u64 = (0x30 >> 4) | ((0x200u64 >> 4) << 28) | (2 << 59);
        for q in [q0, q1, 0x1234] {
            b.extend_from_slice(&q.to_le_bytes());
        }
        match inspect(&b).unwrap().header {
            Header::V8 { entries, decryption_tag: 0xff, .. } => {
                assert_eq!(entries, vec![Entry8 { name_hash: 0xdead_beef, key_id: 0xff, ext_id: 7, resource: true, offset: 0x200, size_on_disk: 0x30, compressor: 2, last_qword: 0x1234 }]);
            }
            h => panic!("{h:?}"),
        }
        b[12] = 0xc0; // encrypted table: entries are not interpreted
        assert!(matches!(inspect(&b).unwrap().header, Header::V8 { ref entries, .. } if entries.is_empty()));
        b[12] = 0xff;
        assert!(inspect(&b[..280]).unwrap_err().contains("truncated"));
    }

    #[test]
    fn rpf8_prefix_skips_partial_entries_but_rejects_truncated_files() {
        let mut prefix = vec![0u8; 272 + 24 + 8];
        prefix[..4].copy_from_slice(b"8FPR");
        prefix[4..8].copy_from_slice(&2u32.to_le_bytes());
        prefix[12..14].copy_from_slice(&0xffu16.to_le_bytes());
        let file_size = 272 + 2 * 24;
        let rpf = inspect_prefix(&prefix, file_size).unwrap();
        assert!(matches!(rpf.header, Header::V8 { entry_count: 2, ref entries, .. } if entries.len() == 1));
        assert!(inspect_prefix(&prefix, file_size - 1).unwrap_err().contains("truncated"));
        assert!(inspect(&prefix).unwrap_err().contains("truncated"));
    }

    #[test]
    fn rejects_other_files_and_truncation() {
        assert!(inspect(b"PK\x03\x04aaaaaaaaaaaa").is_err());
        assert!(inspect(b"9F").is_err());
        assert!(inspect(b"9FPR\x00\x00").unwrap_err().contains("truncated"));
        assert!(inspect(b"5FPRxxxxxxxxxxxxxxxx").unwrap_err().contains("not supported"));
        // every prefix of a valid file either parses or fails cleanly
        let mut b = vec![0u8; 0x40];
        b[..4].copy_from_slice(&0x5250_4639u32.to_le_bytes());
        b.extend(noise(300, 3));
        for n in 0..b.len() {
            let _ = inspect(&b[..n]);
        }
    }
}
