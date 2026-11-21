//! Unit + integration tests. The malformed-input and truncation tests assert
//! the core safety invariant: the parser never panics, it returns `Err`.

use super::*;

#[test]
fn rejects_empty() {
    let err = PeFile::parse(&[]).unwrap_err();
    matches!(err, ParseError::OutOfBounds { .. });
}

#[test]
fn rejects_non_mz() {
    let buf = [0u8; 256];
    let err = PeFile::parse(&buf).unwrap_err();
    match err {
        ParseError::BadMagic { what, .. } => assert_eq!(what, "DOS header (MZ)"),
        other => panic!("expected BadMagic, got {other:?}"),
    }
}

#[test]
fn rejects_mz_without_pe() {
    let mut buf = vec![0u8; 256];
    buf[0] = b'M';
    buf[1] = b'Z';
    // e_lfanew points somewhere with no "PE\0\0".
    buf[0x3C] = 0x80;
    let err = PeFile::parse(&buf).unwrap_err();
    // Either out-of-bounds (if 0x80 is past our tiny buffer) or bad PE sig.
    assert!(matches!(
        err,
        ParseError::OutOfBounds { .. } | ParseError::BadMagic { .. }
    ));
}

/// Deterministic pseudo-random bytes must never cause a panic.
#[test]
fn no_panic_on_random_bytes() {
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as u8
    };
    for size in [0usize, 1, 2, 64, 256, 4096] {
        for _ in 0..200 {
            let buf: Vec<u8> = (0..size).map(|_| next()).collect();
            let _ = PeFile::parse(&buf); // must not panic
        }
    }
}

/// A "looks like a PE" buffer with an MZ and PE signature but random tails must
/// also never panic.
#[test]
fn no_panic_on_pe_shaped_garbage() {
    let mut state: u64 = 0xdead_beef_cafe_babe;
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 40) as u8
    };
    for _ in 0..2000 {
        let mut buf = vec![0u8; 1024];
        for b in buf.iter_mut() {
            *b = next();
        }
        buf[0] = b'M';
        buf[1] = b'Z';
        let e_lfanew = (next() as usize) % 900; // somewhere inside
        buf[0x3C] = (e_lfanew & 0xff) as u8;
        buf[0x3D] = ((e_lfanew >> 8) & 0xff) as u8;
        buf[e_lfanew] = b'P';
        buf[e_lfanew + 1] = b'E';
        buf[e_lfanew + 2] = 0;
        buf[e_lfanew + 3] = 0;
        let _ = PeFile::parse(&buf); // must not panic
    }
}

/// Systematic truncation of a real PE: parse every prefix length, assert no
/// panic. Skips gracefully if the sample file is unavailable.
#[test]
fn no_panic_on_truncated_real_pe() {
    let Some(data) = sample_pe() else {
        eprintln!("sample PE unavailable; skipping");
        return;
    };
    // Full parse must succeed.
    assert!(PeFile::parse(&data).is_ok());
    // Truncate at a spread of lengths (every length is O(n^2); sample instead).
    let step = (data.len() / 500).max(1);
    let mut len = 0;
    while len <= data.len() {
        let _ = PeFile::parse(&data[..len]); // must not panic
        len += step;
    }
}

/// Integration: a real x64 system DLL parses with the expected high-level facts.
#[test]
fn parses_system_dll() {
    let Some(data) = sample_pe() else {
        eprintln!("sample PE unavailable; skipping");
        return;
    };
    let pe = PeFile::parse(&data).expect("kernel32 should parse");
    assert_eq!(
        pe.coff.machine,
        constants::IMAGE_FILE_MACHINE_AMD64,
        "expected AMD64 machine"
    );
    assert_eq!(pe.opt.magic, 0x020B, "expected PE32+");
    assert!(pe.is_dll(), "kernel32 should be a DLL");
    let exports = pe.exports.as_ref().expect("kernel32 exports a lot");
    assert!(
        exports.named.iter().any(|(n, _)| n == "CreateFileW"),
        "kernel32 should export CreateFileW; got {} names",
        exports.named.len()
    );
}

/// Integration: a real x64 DLL carries base relocations dominated by DIR64 (10).
#[test]
fn relocations_are_dir64_on_x64() {
    let Some(data) = sample_pe() else {
        eprintln!("sample PE unavailable; skipping");
        return;
    };
    let pe = PeFile::parse(&data).expect("parse");
    // System DLLs are relocatable; if stripped, skip rather than fail.
    if pe.coff.characteristics & constants::IMAGE_FILE_RELOCS_STRIPPED != 0 {
        eprintln!("relocs stripped; skipping");
        return;
    }
    let rel = pe.relocations.as_ref().expect("x64 DLL should have relocations");
    assert!(rel.total_entries > 0, "expected at least one relocation entry");
    // On x64, pointer relocations are DIR64 (type 10). It should be the most common
    // non-ABSOLUTE type by a wide margin.
    let dir64 = rel.by_type.iter().find(|(t, _)| *t == 10).map(|(_, n)| *n).unwrap_or(0);
    assert!(
        dir64 > 0,
        "expected DIR64 relocations on x64; got types {:?}",
        rel.by_type
    );
}

/// relocation_entries() returns the detailed fixups and their count matches the
/// summary's non-ABSOLUTE total.
#[test]
fn relocation_entries_match_summary() {
    let Some(data) = sample_pe() else {
        eprintln!("sample PE unavailable; skipping");
        return;
    };
    let pe = PeFile::parse(&data).expect("parse");
    if pe.coff.characteristics & constants::IMAGE_FILE_RELOCS_STRIPPED != 0 {
        return;
    }
    let entries = pe.relocation_entries().expect("reloc entries");
    let summary = pe.relocations.as_ref().unwrap();
    let non_absolute: usize = summary
        .by_type
        .iter()
        .filter(|(t, _)| *t != 0)
        .map(|(_, n)| *n)
        .sum();
    assert_eq!(entries.len(), non_absolute);
    // On x64 the detailed entries should be overwhelmingly DIR64 (10).
    let dir64 = entries.iter().filter(|e| e.kind == 10).count();
    assert!(dir64 > 0 && dir64 == non_absolute, "expected all DIR64");
    // Every reloc RVA must be mappable to a file offset.
    for e in entries.iter().take(1000) {
        assert!(pe.rva_to_offset(e.rva).is_ok(), "reloc rva {:#x} unmappable", e.rva);
    }
}

/// Load a known-good x64 PE for integration tests. Prefer kernel32 (stable
/// exports), fall back to the test binary itself.
fn sample_pe() -> Option<Vec<u8>> {
    let candidates = [
        r"C:\Windows\System32\kernel32.dll",
        r"C:\Windows\System32\ntdll.dll",
    ];
    for c in candidates {
        if let Ok(d) = std::fs::read(c) {
            return Some(d);
        }
    }
    None
}

fn synthetic_pe(raw_offset: u32, raw_size: u32, virtual_size: u32, headers_size: u32) -> Vec<u8> {
    fn u16_at(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn u32_at(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    let mut bytes = vec![0; 0x200];
    u16_at(&mut bytes, 0, 0x5a4d);
    u32_at(&mut bytes, 0x3c, 0x80);
    u32_at(&mut bytes, 0x80, 0x4550);
    u16_at(&mut bytes, 0x84, constants::IMAGE_FILE_MACHINE_AMD64);
    u16_at(&mut bytes, 0x86, 1);
    u16_at(&mut bytes, 0x94, 0xf0);
    u16_at(&mut bytes, 0x98, 0x20b);
    u32_at(&mut bytes, 0x98 + 32, 0x1000);
    u32_at(&mut bytes, 0x98 + 36, 0x200);
    u32_at(&mut bytes, 0x98 + 56, 0x2000);
    u32_at(&mut bytes, 0x98 + 60, headers_size);
    let section = 0x188;
    bytes[section..section + 5].copy_from_slice(b".test");
    u32_at(&mut bytes, section + 8, virtual_size);
    u32_at(&mut bytes, section + 12, 0x1000);
    u32_at(&mut bytes, section + 16, raw_size);
    u32_at(&mut bytes, section + 20, raw_offset);
    bytes
}

#[test]
fn rva_mapping_rejects_section_offsets_outside_the_file() {
    for raw_offset in [0x200, 0xffff_ff00, u32::MAX] {
        let bytes = synthetic_pe(raw_offset, 0x200, 0x200, 0x200);
        let pe = PeFile::parse(&bytes).expect("malformed raw ranges remain inspectable");
        for rva in [0x1000, 0x1001, 0x10ff] {
            assert_eq!(
                pe.rva_to_offset(rva),
                Err(ParseError::UnmappableRva { rva }),
                "raw offset {raw_offset:#x}, RVA {rva:#x}"
            );
        }
    }
}

#[test]
fn rva_mapping_keeps_backed_bytes_of_a_truncated_section() {
    let bytes = synthetic_pe(0x1f0, 0x100, 0x200, 0x200);
    let pe = PeFile::parse(&bytes).unwrap();
    assert_eq!(pe.rva_to_offset(0x1000), Ok(0x1f0));
    assert_eq!(pe.rva_to_offset(0x100f), Ok(bytes.len() - 1));
    assert_eq!(
        pe.rva_to_offset(0x1010),
        Err(ParseError::UnmappableRva { rva: 0x1010 })
    );
}

#[test]
fn rva_mapping_rejects_headers_at_and_past_eof() {
    let bytes = synthetic_pe(0x1f0, 0x10, 0x10, 0x10000);
    let pe = PeFile::parse(&bytes).unwrap();
    assert_eq!(pe.rva_to_offset(0), Ok(0));
    assert_eq!(pe.rva_to_offset(0x1ff), Ok(0x1ff));
    for rva in [0x200, 0x300] {
        assert_eq!(pe.rva_to_offset(rva), Err(ParseError::UnmappableRva { rva }));
    }
}

#[test]
fn rva_mapping_rejects_virtual_only_section_bytes() {
    let bytes = synthetic_pe(0x1f0, 0x10, 0x200, 0x200);
    let pe = PeFile::parse(&bytes).unwrap();
    assert_eq!(pe.rva_to_offset(0x100f), Ok(0x1ff));
    for rva in [0x1010, 0x11ff, 0x1200] {
        assert_eq!(pe.rva_to_offset(rva), Err(ParseError::UnmappableRva { rva }));
    }
}

/// Build a minimal PE32+ with an export directory so a test can drive the export
/// parser. The 40-byte IMAGE_EXPORT_DIRECTORY is placed in the header region
/// (RVA 0x1b0) so it maps 1:1 to a file offset without needing a backed section.
fn pe_with_export_dir(
    number_of_functions: u32,
    number_of_names: u32,
    address_of_functions: u32,
    address_of_names: u32,
    address_of_name_ordinals: u32,
) -> Vec<u8> {
    fn u16_at(b: &mut [u8], o: usize, v: u16) {
        b[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn u32_at(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    let mut bytes = vec![0u8; 0x200];
    u16_at(&mut bytes, 0, 0x5a4d); // MZ
    u32_at(&mut bytes, 0x3c, 0x80); // e_lfanew
    u32_at(&mut bytes, 0x80, 0x4550); // PE\0\0
    u16_at(&mut bytes, 0x84, constants::IMAGE_FILE_MACHINE_AMD64);
    u16_at(&mut bytes, 0x86, 1); // NumberOfSections
    u16_at(&mut bytes, 0x94, 0xf0); // SizeOfOptionalHeader
    u16_at(&mut bytes, 0x98, 0x20b); // PE32+
    u32_at(&mut bytes, 0x98 + 32, 0x1000); // SectionAlignment
    u32_at(&mut bytes, 0x98 + 36, 0x200); // FileAlignment
    u32_at(&mut bytes, 0x98 + 56, 0x2000); // SizeOfImage
    u32_at(&mut bytes, 0x98 + 60, 0x200); // SizeOfHeaders
    u32_at(&mut bytes, 0x98 + 108, 16); // NumberOfRvaAndSizes

    // One small section at VA 0x1000 (keeps the high RVAs below unmapped).
    let section = 0x188;
    bytes[section..section + 5].copy_from_slice(b".test");
    u32_at(&mut bytes, section + 8, 0x10); // VirtualSize
    u32_at(&mut bytes, section + 12, 0x1000); // VirtualAddress
    u32_at(&mut bytes, section + 16, 0x10); // SizeOfRawData
    u32_at(&mut bytes, section + 20, 0x1f0); // PointerToRawData

    // Export data directory (index DIR_EXPORT = 0) -> RVA 0x1b0.
    let export_rva = 0x1b0u32;
    u32_at(&mut bytes, 0x108, export_rva); // DataDir[0].VirtualAddress
    u32_at(&mut bytes, 0x10c, 40); // DataDir[0].Size

    // The IMAGE_EXPORT_DIRECTORY at file offset 0x1b0.
    let e = export_rva as usize;
    u32_at(&mut bytes, e + 12, 0); // Name RVA
    u32_at(&mut bytes, e + 16, 1); // Base
    u32_at(&mut bytes, e + 20, number_of_functions);
    u32_at(&mut bytes, e + 24, number_of_names);
    u32_at(&mut bytes, e + 28, address_of_functions);
    u32_at(&mut bytes, e + 32, address_of_names);
    u32_at(&mut bytes, e + 36, address_of_name_ordinals);
    bytes
}

/// Regression: an export table whose AddressOfFunctions is near u32::MAX makes
/// `addr_funcs_rva + idx * 4` overflow in the unnamed-ordinal loop. With
/// overflow-checks on (set for every profile in Cargo.toml) the unchecked `+`
/// panicked; the parser must return a value instead.
#[test]
fn export_address_of_functions_near_u32_max_does_not_panic() {
    // NumberOfNames = 0 routes straight to the unnamed-ordinal loop.
    let bytes = pe_with_export_dir(2, 0, 0xffff_ffff, 0, 0);
    let pe = PeFile::parse(&bytes).expect("malformed export table stays inspectable");
    let exp = pe.exports.as_ref().expect("export directory recognised");
    assert_eq!(exp.number_of_functions, 2);
    // The overflowing RVA maps to nothing, so no ordinal should be collected.
    assert!(exp.unnamed_ordinals.is_empty());
}

/// Regression: in the named loop, a large name-ordinal index combined with an
/// AddressOfFunctions near u32::MAX makes `addr_funcs_rva + ord_index * 4`
/// overflow. Must not panic.
#[test]
fn export_named_func_rva_near_u32_max_does_not_panic() {
    // One name; its name-pointer array and ordinal array live in mapped header
    // bytes just below the export directory.
    let names_rva = 0x1a0u32;
    let ords_rva = 0x1a8u32;
    let mut bytes = pe_with_export_dir(2, 1, 0xffff_ffff, names_rva, ords_rva);
    // AddressOfNames[0] -> a name string RVA (0 = empty, still mappable).
    bytes[names_rva as usize..names_rva as usize + 4].copy_from_slice(&0u32.to_le_bytes());
    // AddressOfNameOrdinals[0] = 0xffff, so ord_index * 4 pushes the add over.
    bytes[ords_rva as usize..ords_rva as usize + 2].copy_from_slice(&0xffffu16.to_le_bytes());
    let pe = PeFile::parse(&bytes).expect("malformed export table stays inspectable");
    let exp = pe.exports.as_ref().expect("export directory recognised");
    assert_eq!(exp.named.len(), 1);
}
