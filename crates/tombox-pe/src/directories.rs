//! Parsers for the data directories most relevant to understanding an Xbox
//! PE's dependencies and loader requirements: imports, delay imports, exports,
//! base relocations, and TLS. Each parser is defensive: it caps iteration,
//! tolerates unmappable RVAs, and records a note instead of panicking.

use crate::constants::*;
use crate::{PeFile, Result};

// Safety caps to bound work on hostile/corrupt inputs.
const MAX_DLLS: usize = 8192;
const MAX_FUNCS_PER_DLL: usize = 1 << 18;
const MAX_RELOC_BLOCKS: usize = 1 << 20;
const MAX_TLS_CALLBACKS: usize = 4096;
const MAX_NAME: usize = 1024;

#[derive(Debug, Clone)]
pub enum ImportEntry {
    ByName { hint: u16, name: String },
    ByOrdinal(u16),
}

#[derive(Debug, Clone)]
pub struct ImportedDll {
    pub name: String,
    pub original_first_thunk: u32,
    pub first_thunk: u32,
    pub functions: Vec<ImportEntry>,
    /// True if the import-lookup table was absent and we read the IAT instead.
    pub used_iat_fallback: bool,
}

#[derive(Debug, Clone)]
pub struct Imports {
    pub dlls: Vec<ImportedDll>,
}

#[derive(Debug, Clone)]
pub struct DelayImports {
    pub dlls: Vec<ImportedDll>,
}

#[derive(Debug, Clone)]
pub struct ExportTable {
    pub dll_name: String,
    pub ordinal_base: u32,
    pub number_of_functions: u32,
    pub number_of_names: u32,
    /// (name, ordinal) for named exports.
    pub named: Vec<(String, u32)>,
    /// Ordinals with an address but no name.
    pub unnamed_ordinals: Vec<u32>,
    pub forwarders: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct RelocSummary {
    pub total_blocks: usize,
    pub total_entries: usize,
    /// Count per relocation type code.
    pub by_type: Vec<(u16, usize)>,
}

#[derive(Debug, Clone)]
pub struct TlsInfo {
    pub start_address_of_raw_data: u64,
    pub end_address_of_raw_data: u64,
    pub address_of_index: u64,
    pub address_of_callbacks: u64,
    pub size_of_zero_fill: u32,
    pub characteristics: u32,
    pub callback_count: usize,
}

/// Read a NUL-terminated name located at an RVA.
fn cstr_rva(pe: &PeFile, rva: u32) -> Option<String> {
    let off = pe.rva_to_offset(rva).ok()?;
    pe.reader().cstr_at(off, MAX_NAME, "name@rva").ok()
}

fn thunk_table(pe: &PeFile, table_rva: u32, into: &mut Vec<ImportEntry>, notes: &mut Vec<String>) {
    let mut idx = 0usize;
    loop {
        if idx >= MAX_FUNCS_PER_DLL {
            notes.push(format!("import thunk table at rva {table_rva:#x} hit the {MAX_FUNCS_PER_DLL} cap"));
            break;
        }
        let Some(entry_rva) = table_rva.checked_add((idx as u32) * 8) else {
            notes.push(format!("import thunk table at rva {table_rva:#x} overflows the RVA space"));
            break;
        };
        let off = match pe.rva_to_offset(entry_rva) {
            Ok(o) => o,
            Err(_) => {
                notes.push(format!("import thunk at rva {entry_rva:#x} is unmappable"));
                break;
            }
        };
        let thunk = match pe.reader().u64_at(off, "import thunk") {
            Ok(v) => v,
            Err(_) => break,
        };
        if thunk == 0 {
            break;
        }
        const ORDINAL_FLAG: u64 = 1u64 << 63;
        if thunk & ORDINAL_FLAG != 0 {
            into.push(ImportEntry::ByOrdinal((thunk & 0xFFFF) as u16));
        } else {
            let hint_rva = (thunk & 0x7FFF_FFFF) as u32;
            let hint = pe
                .rva_to_offset(hint_rva)
                .ok()
                .and_then(|o| pe.reader().u16_at(o, "import hint").ok())
                .unwrap_or(0);
            let name = cstr_rva(pe, hint_rva.saturating_add(2)).unwrap_or_default();
            into.push(ImportEntry::ByName { hint, name });
        }
        idx += 1;
    }
}

pub fn parse_imports(pe: &PeFile, notes: &mut Vec<String>) -> Option<Imports> {
    let dir = pe.directory(DIR_IMPORT)?;
    if dir.virtual_address == 0 || dir.size == 0 {
        return None;
    }
    let mut dlls = Vec::new();
    let mut i = 0usize;
    loop {
        if i >= MAX_DLLS {
            notes.push(format!("import descriptor table hit the {MAX_DLLS} cap"));
            break;
        }
        let Some(desc_rva) = dir.virtual_address.checked_add((i as u32) * 20) else {
            notes.push("import descriptor table RVA overflowed the RVA space".into());
            break;
        };
        let off = match pe.rva_to_offset(desc_rva) {
            Ok(o) => o,
            Err(_) => {
                notes.push("import descriptor table became unmappable".into());
                break;
            }
        };
        let r = pe.reader();
        let oft = r.u32_at(off, "IMPORT_DESC.OriginalFirstThunk").ok()?;
        let name_rva = r.u32_at(off + 12, "IMPORT_DESC.Name").ok()?;
        let ft = r.u32_at(off + 16, "IMPORT_DESC.FirstThunk").ok()?;
        // All-zero descriptor terminates the array.
        if oft == 0 && name_rva == 0 && ft == 0 {
            break;
        }
        let name = cstr_rva(pe, name_rva).unwrap_or_else(|| "<unmappable name>".into());
        let (table, used_iat_fallback) = if oft != 0 { (oft, false) } else { (ft, true) };
        let mut functions = Vec::new();
        if table != 0 {
            thunk_table(pe, table, &mut functions, notes);
        }
        dlls.push(ImportedDll {
            name,
            original_first_thunk: oft,
            first_thunk: ft,
            functions,
            used_iat_fallback,
        });
        i += 1;
    }
    if dlls.is_empty() {
        None
    } else {
        Some(Imports { dlls })
    }
}

pub fn parse_delay_imports(pe: &PeFile, notes: &mut Vec<String>) -> Option<DelayImports> {
    let dir = pe.directory(DIR_DELAY_IMPORT)?;
    if dir.virtual_address == 0 || dir.size == 0 {
        return None;
    }
    // ImgDelayDescr (v2, RVA-based): Attributes, DllNameRVA, ModuleHandleRVA,
    // ImportAddressTableRVA, ImportNameTableRVA, BoundIATRVA, UnloadIATRVA, TimeStamp.
    let mut dlls = Vec::new();
    let mut i = 0usize;
    loop {
        if i >= MAX_DLLS {
            break;
        }
        let Some(desc_rva) = dir.virtual_address.checked_add((i as u32) * 32) else {
            break;
        };
        let off = pe.rva_to_offset(desc_rva).ok()?;
        let r = pe.reader();
        let attributes = r.u32_at(off, "DELAY.Attributes").ok()?;
        let name_rva = r.u32_at(off + 4, "DELAY.DllNameRVA").ok()?;
        let int_rva = r.u32_at(off + 16, "DELAY.ImportNameTableRVA").ok()?;
        if attributes == 0 && name_rva == 0 && int_rva == 0 {
            break;
        }
        if attributes & 1 == 0 {
            notes.push(
                "delay-import descriptor uses legacy VA-based addressing; skipping (not RVA-based)"
                    .into(),
            );
            i += 1;
            continue;
        }
        let name = cstr_rva(pe, name_rva).unwrap_or_else(|| "<unmappable>".into());
        let mut functions = Vec::new();
        if int_rva != 0 {
            thunk_table(pe, int_rva, &mut functions, notes);
        }
        dlls.push(ImportedDll {
            name,
            original_first_thunk: int_rva,
            first_thunk: 0,
            functions,
            used_iat_fallback: false,
        });
        i += 1;
    }
    if dlls.is_empty() {
        None
    } else {
        Some(DelayImports { dlls })
    }
}

pub fn parse_exports(pe: &PeFile, notes: &mut Vec<String>) -> Option<ExportTable> {
    let dir = pe.directory(DIR_EXPORT)?;
    if dir.virtual_address == 0 || dir.size == 0 {
        return None;
    }
    let off = pe.rva_to_offset(dir.virtual_address).ok()?;
    let r = pe.reader();
    let name_rva = r.u32_at(off + 12, "EXPORT.Name").ok()?;
    let ordinal_base = r.u32_at(off + 16, "EXPORT.Base").ok()?;
    let number_of_functions = r.u32_at(off + 20, "EXPORT.NumberOfFunctions").ok()?;
    let number_of_names = r.u32_at(off + 24, "EXPORT.NumberOfNames").ok()?;
    let addr_funcs_rva = r.u32_at(off + 28, "EXPORT.AddressOfFunctions").ok()?;
    let addr_names_rva = r.u32_at(off + 32, "EXPORT.AddressOfNames").ok()?;
    let addr_ords_rva = r.u32_at(off + 36, "EXPORT.AddressOfNameOrdinals").ok()?;

    let dll_name = cstr_rva(pe, name_rva).unwrap_or_default();

    let n_names = (number_of_names as usize).min(1 << 20);
    let n_funcs = (number_of_functions as usize).min(1 << 20);
    let mut named = Vec::new();
    let mut forwarders = Vec::new();

    // The export directory's own RVA range; an EAT entry pointing back into it
    // is a forwarder string rather than a function address.
    let exp_lo = dir.virtual_address;
    let exp_hi = dir.virtual_address.saturating_add(dir.size);

    // Track which function indices are named.
    let mut named_indices = vec![false; n_funcs];

    for k in 0..n_names {
        let Some(name_ptr_rva) = addr_names_rva.checked_add((k as u32) * 4) else { break };
        let name_ptr_off = match pe.rva_to_offset(name_ptr_rva) {
            Ok(o) => o,
            Err(_) => break,
        };
        let str_rva = match r.u32_at(name_ptr_off, "EXPORT name ptr") {
            Ok(v) => v,
            Err(_) => break,
        };
        let name = cstr_rva(pe, str_rva).unwrap_or_default();
        let Some(ord_ptr_rva) = addr_ords_rva.checked_add((k as u32) * 2) else { break };
        let ord_off = match pe.rva_to_offset(ord_ptr_rva) {
            Ok(o) => o,
            Err(_) => break,
        };
        let ord_index = r.u16_at(ord_off, "EXPORT ordinal").unwrap_or(0) as u32;
        let ordinal = ordinal_base.saturating_add(ord_index);
        if (ord_index as usize) < named_indices.len() {
            named_indices[ord_index as usize] = true;
        }
        if let Some(func_rva_ptr) = addr_funcs_rva.checked_add(ord_index * 4) {
            if let Ok(func_off) = pe.rva_to_offset(func_rva_ptr) {
                if let Ok(func_rva) = r.u32_at(func_off, "EXPORT func rva") {
                    if func_rva >= exp_lo && func_rva < exp_hi {
                        let target = cstr_rva(pe, func_rva).unwrap_or_default();
                        forwarders.push((name.clone(), target));
                    }
                }
            }
        }
        named.push((name, ordinal));
    }

    let mut unnamed_ordinals = Vec::new();
    for idx in 0..n_funcs {
        if named_indices.get(idx).copied().unwrap_or(false) {
            continue;
        }
        let Some(func_rva_ptr) = addr_funcs_rva.checked_add((idx as u32) * 4) else {
            continue;
        };
        if let Ok(func_off) = pe.rva_to_offset(func_rva_ptr) {
            if let Ok(func_rva) = r.u32_at(func_off, "EXPORT func rva") {
                if func_rva != 0 {
                    unnamed_ordinals.push(ordinal_base.saturating_add(idx as u32));
                }
            }
        }
    }

    if named.len() != n_names {
        notes.push(format!(
            "export table: parsed {} of {} names before stopping",
            named.len(),
            n_names
        ));
    }

    Some(ExportTable {
        dll_name,
        ordinal_base,
        number_of_functions,
        number_of_names,
        named,
        unnamed_ordinals,
        forwarders,
    })
}

fn relocation_bytes<'a>(pe: &PeFile<'a>, rva: u32, size: u32) -> Result<&'a [u8]> {
    let off = pe.rva_to_offset(rva)?;
    let available = match pe.sections.iter().find(|s| s.contains_rva(rva)) {
        Some(s) => s.size_of_raw_data - (rva - s.virtual_address),
        None => pe.opt.size_of_headers - rva,
    };
    if size > available {
        return Err(crate::ParseError::Invariant {
            what: "RELOC.BlockSize",
            detail: format!("block at rva {rva:#x} exceeds its file-backed range"),
        });
    }
    pe.reader().slice_at(off, size as usize, "RELOC block")
}

pub(crate) fn walk_relocations(
    pe: &PeFile,
    mut visit: impl FnMut(u32, &[u8]),
) -> Result<()> {
    let Some(dir) = pe.directory(DIR_BASERELOC) else {
        return Ok(());
    };
    if dir.virtual_address == 0 || dir.size == 0 {
        return Ok(());
    }
    let mut cur = dir.virtual_address;
    let end = cur.checked_add(dir.size).ok_or_else(|| crate::ParseError::Invariant {
        what: "RELOC directory",
        detail: "RVA range overflow".into(),
    })?;
    let mut blocks = 0usize;
    while cur < end {
        if blocks >= MAX_RELOC_BLOCKS {
            return Err(crate::ParseError::Invariant {
                what: "RELOC directory",
                detail: "relocation block count hit cap".into(),
            });
        }
        let remaining = end - cur;
        if remaining < 8 {
            return Err(crate::ParseError::Invariant {
                what: "RELOC header",
                detail: format!("incomplete header at rva {cur:#x}"),
            });
        }
        let header = crate::Reader::new(relocation_bytes(pe, cur, 8)?);
        let page_rva = header.u32_at(0, "RELOC.PageRVA")?;
        let block_size = header.u32_at(4, "RELOC.BlockSize")?;
        if block_size < 8 || block_size % 2 != 0 || block_size > remaining {
            return Err(crate::ParseError::Invariant {
                what: "RELOC.BlockSize",
                detail: format!("block at rva {cur:#x} has size {block_size}, remaining {remaining}"),
            });
        }
        let entries = &relocation_bytes(pe, cur, block_size)?[8..];
        for entry in entries.chunks_exact(2) {
            let entry = u16::from_le_bytes([entry[0], entry[1]]);
            if entry >> 12 != 0 && page_rva.checked_add(u32::from(entry & 0x0fff)).is_none() {
                return Err(crate::ParseError::Invariant {
                    what: "RELOC target RVA",
                    detail: format!("relocation target overflows at block rva {cur:#x}"),
                });
            }
        }
        visit(page_rva, entries);
        cur += block_size;
        blocks += 1;
    }
    Ok(())
}

pub fn parse_relocations(pe: &PeFile, notes: &mut Vec<String>) -> Option<RelocSummary> {
    let dir = pe.directory(DIR_BASERELOC)?;
    if dir.virtual_address == 0 || dir.size == 0 {
        return None;
    }
    let mut summary = RelocSummary::default();
    let mut type_counts: std::collections::BTreeMap<u16, usize> = std::collections::BTreeMap::new();
    if let Err(err) = walk_relocations(pe, |_, entries| {
        for entry in entries.chunks_exact(2) {
            let typ = u16::from_le_bytes([entry[0], entry[1]]) >> 12;
            *type_counts.entry(typ).or_insert(0) += 1;
            summary.total_entries += 1;
        }
        summary.total_blocks += 1;
    }) {
        notes.push(format!("relocation directory: {err}"));
    }
    summary.by_type = type_counts.into_iter().collect();
    Some(summary)
}

pub fn parse_tls(pe: &PeFile, notes: &mut Vec<String>) -> Option<TlsInfo> {
    let dir = pe.directory(DIR_TLS)?;
    if dir.virtual_address == 0 || dir.size == 0 {
        return None;
    }
    let off = pe.rva_to_offset(dir.virtual_address).ok()?;
    let r = pe.reader();
    let start = r.u64_at(off, "TLS.StartAddressOfRawData").ok()?;
    let end = r.u64_at(off + 8, "TLS.EndAddressOfRawData").ok()?;
    let index = r.u64_at(off + 16, "TLS.AddressOfIndex").ok()?;
    let callbacks = r.u64_at(off + 24, "TLS.AddressOfCallBacks").ok()?;
    let zero_fill = r.u32_at(off + 32, "TLS.SizeOfZeroFill").ok()?;
    let characteristics = r.u32_at(off + 36, "TLS.Characteristics").ok()?;

    // AddressOfCallBacks is an absolute VA pointing at a NULL-terminated array
    // of absolute VA callback pointers. Convert VA->RVA via ImageBase.
    let mut callback_count = 0usize;
    if callbacks != 0 {
        if let Some(cb_rva) = callbacks.checked_sub(pe.opt.image_base) {
            let mut idx = 0u32;
            while callback_count < MAX_TLS_CALLBACKS {
                let entry_rva = (cb_rva as u32).wrapping_add(idx * 8);
                let o = match pe.rva_to_offset(entry_rva) {
                    Ok(o) => o,
                    Err(_) => break,
                };
                match r.u64_at(o, "TLS callback ptr") {
                    Ok(0) | Err(_) => break,
                    Ok(_) => callback_count += 1,
                }
                idx += 1;
            }
        } else {
            notes.push("TLS AddressOfCallBacks below ImageBase; cannot convert to RVA".into());
        }
    }

    Some(TlsInfo {
        start_address_of_raw_data: start,
        end_address_of_raw_data: end,
        address_of_index: index,
        address_of_callbacks: callbacks,
        size_of_zero_fill: zero_fill,
        characteristics,
        callback_count,
    })
}

/// Convenience: the raw bytes of a data directory, if mappable (used by callers
/// that want to hash or hex-dump a directory for deeper manual analysis).
pub fn directory_bytes<'a>(pe: &PeFile<'a>, index: usize) -> Result<&'a [u8]> {
    let dir = pe
        .directory(index)
        .ok_or(crate::ParseError::Unsupported { what: format!("directory index {index} absent") })?;
    let off = pe.rva_to_offset(dir.virtual_address)?;
    pe.reader().slice_at(off, dir.size as usize, "directory bytes")
}
