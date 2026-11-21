//! `tombox-pe` — a bounds-checked parser for PE/COFF images, oriented toward
//! inspecting Xbox Series X|S PE32+ (x64) executables and their dependencies.
//!
//! Design goals (per CLAUDE.md):
//!  * Report *raw facts* and validate structural invariants.
//!  * Preserve unknown fields/bytes rather than discarding them.
//!  * Never panic on malformed input — every failure is an explicit `ParseError`.
//!  * Zero external dependencies, so the whole parse path is auditable.
//!
//! This parser is deliberately read-only and does not execute or map anything.

mod error;
mod reader;
pub mod constants;
pub mod directories;

pub use error::{ParseError, Result};
pub use reader::Reader;

use directories::{DelayImports, ExportTable, Imports, RelocSummary, TlsInfo};

/// Offsets/sizes of the DOS stub we care about.
const DOS_MAGIC: u16 = 0x5A4D; // 'MZ'
const PE_SIGNATURE: u32 = 0x0000_4550; // 'PE\0\0'
const OPT_MAGIC_PE32: u16 = 0x010B;
const OPT_MAGIC_PE32PLUS: u16 = 0x020B;
const OPT_MAGIC_ROM: u16 = 0x0107;

/// The COFF file header (a.k.a. IMAGE_FILE_HEADER), 20 bytes.
#[derive(Debug, Clone)]
pub struct CoffHeader {
    pub machine: u16,
    pub number_of_sections: u16,
    pub time_date_stamp: u32,
    pub pointer_to_symbol_table: u32,
    pub number_of_symbols: u32,
    pub size_of_optional_header: u16,
    pub characteristics: u16,
}

/// One entry in the data directory array (RVA + size).
#[derive(Debug, Clone, Copy, Default)]
pub struct DataDirectory {
    pub virtual_address: u32,
    pub size: u32,
}

/// The PE32+ optional header. We only model PE32+ fully; PE32 and ROM images
/// are detected and reported but flagged unsupported for deep parsing, because
/// the Xbox target is x64/PE32+ and we refuse to guess at a different layout.
#[derive(Debug, Clone)]
pub struct OptionalHeader {
    pub magic: u16,
    pub major_linker_version: u8,
    pub minor_linker_version: u8,
    pub size_of_code: u32,
    pub size_of_initialized_data: u32,
    pub size_of_uninitialized_data: u32,
    pub address_of_entry_point: u32,
    pub base_of_code: u32,
    pub image_base: u64,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub major_os_version: u16,
    pub minor_os_version: u16,
    pub major_image_version: u16,
    pub minor_image_version: u16,
    pub major_subsystem_version: u16,
    pub minor_subsystem_version: u16,
    pub win32_version_value: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub checksum: u32,
    pub subsystem: u16,
    pub dll_characteristics: u16,
    pub size_of_stack_reserve: u64,
    pub size_of_stack_commit: u64,
    pub size_of_heap_reserve: u64,
    pub size_of_heap_commit: u64,
    pub loader_flags: u32,
    pub number_of_rva_and_sizes: u32,
    pub data_directories: Vec<DataDirectory>,
}

/// A section header (IMAGE_SECTION_HEADER), 40 bytes.
#[derive(Debug, Clone)]
pub struct Section {
    /// The 8 raw name bytes, preserved exactly (may not be NUL-terminated).
    pub name_raw: [u8; 8],
    pub name: String,
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub size_of_raw_data: u32,
    pub pointer_to_raw_data: u32,
    pub pointer_to_relocations: u32,
    pub pointer_to_linenumbers: u32,
    pub number_of_relocations: u16,
    pub number_of_linenumbers: u16,
    pub characteristics: u32,
}

impl Section {
    /// Does this section's virtual range contain `rva`?
    fn contains_rva(&self, rva: u32) -> bool {
        // Use the larger of virtual_size / size_of_raw_data for containment so
        // we map addresses that land in the (file-backed) raw data even when
        // VirtualSize is rounded down oddly.
        let span = self.virtual_size.max(self.size_of_raw_data);
        let start = self.virtual_address;
        let end = start.saturating_add(span);
        rva >= start && rva < end
    }
}

/// Non-fatal structural observations collected during parsing. These are facts
/// about deviations from the common case, not errors — we record rather than
/// discard them.
#[derive(Debug, Clone, Default)]
pub struct Notes(pub Vec<String>);

impl Notes {
    fn push(&mut self, s: impl Into<String>) {
        self.0.push(s.into());
    }
}

/// A fully parsed PE image (headers + directories we understand).
#[derive(Debug)]
pub struct PeFile<'a> {
    data: &'a [u8],
    pub e_lfanew: u32,
    pub coff: CoffHeader,
    pub opt: OptionalHeader,
    pub sections: Vec<Section>,
    pub imports: Option<Imports>,
    pub delay_imports: Option<DelayImports>,
    pub exports: Option<ExportTable>,
    pub relocations: Option<RelocSummary>,
    pub tls: Option<TlsInfo>,
    pub notes: Notes,
}

impl<'a> PeFile<'a> {
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Map a relative virtual address to a file offset using the section table.
    /// Returns an error for addresses not backed by file data.
    pub fn rva_to_offset(&self, rva: u32) -> Result<usize> {
        // RVAs inside the header area (below the first section) map 1:1.
        for s in &self.sections {
            if s.contains_rva(rva) {
                let delta = rva - s.virtual_address;
                if delta >= s.size_of_raw_data {
                    // Falls in uninitialised (BSS-like) space: no file bytes.
                    return Err(ParseError::UnmappableRva { rva });
                }
                return (s.pointer_to_raw_data as usize)
                    .checked_add(delta as usize)
                    .filter(|&offset| offset < self.data.len())
                    .ok_or(ParseError::UnmappableRva { rva });
            }
        }
        // Header region fallback.
        if (rva as usize) < self.opt.size_of_headers as usize
            && (rva as usize) < self.data.len()
        {
            return Ok(rva as usize);
        }
        Err(ParseError::UnmappableRva { rva })
    }

    pub fn reader(&self) -> Reader<'a> {
        Reader::new(self.data)
    }

    /// Parse an image from a complete in-memory buffer.
    pub fn parse(data: &'a [u8]) -> Result<PeFile<'a>> {
        let r = Reader::new(data);
        let mut notes = Notes::default();

        // --- DOS header ---
        let dos_magic = r.u16_at(0, "DOS magic")?;
        if dos_magic != DOS_MAGIC {
            return Err(ParseError::BadMagic {
                what: "DOS header (MZ)",
                found: dos_magic as u64,
                expected: DOS_MAGIC as u64,
            });
        }
        let e_lfanew = r.u32_at(0x3C, "e_lfanew")?;

        // --- PE signature ---
        let pe_off = e_lfanew as usize;
        let sig = r.u32_at(pe_off, "PE signature")?;
        if sig != PE_SIGNATURE {
            return Err(ParseError::BadMagic {
                what: "PE signature",
                found: sig as u64,
                expected: PE_SIGNATURE as u64,
            });
        }

        // --- COFF header ---
        let coff_off = pe_off + 4;
        let coff = CoffHeader {
            machine: r.u16_at(coff_off, "COFF.Machine")?,
            number_of_sections: r.u16_at(coff_off + 2, "COFF.NumberOfSections")?,
            time_date_stamp: r.u32_at(coff_off + 4, "COFF.TimeDateStamp")?,
            pointer_to_symbol_table: r.u32_at(coff_off + 8, "COFF.PointerToSymbolTable")?,
            number_of_symbols: r.u32_at(coff_off + 12, "COFF.NumberOfSymbols")?,
            size_of_optional_header: r.u16_at(coff_off + 16, "COFF.SizeOfOptionalHeader")?,
            characteristics: r.u16_at(coff_off + 18, "COFF.Characteristics")?,
        };

        // --- Optional header ---
        let opt_off = coff_off + 20;
        if coff.size_of_optional_header < 2 {
            return Err(ParseError::Invariant {
                what: "SizeOfOptionalHeader",
                detail: format!("too small ({})", coff.size_of_optional_header),
            });
        }
        let magic = r.u16_at(opt_off, "OptionalHeader.Magic")?;
        match magic {
            OPT_MAGIC_PE32PLUS => {}
            OPT_MAGIC_PE32 => {
                return Err(ParseError::Unsupported {
                    what: "PE32 (32-bit) optional header; Xbox target is PE32+/x64. \
                           Refusing to parse with the wrong layout."
                        .into(),
                })
            }
            OPT_MAGIC_ROM => {
                return Err(ParseError::Unsupported {
                    what: "ROM image optional header".into(),
                })
            }
            other => {
                return Err(ParseError::BadMagic {
                    what: "OptionalHeader.Magic",
                    found: other as u64,
                    expected: OPT_MAGIC_PE32PLUS as u64,
                })
            }
        }

        let number_of_rva_and_sizes = r.u32_at(opt_off + 108, "NumberOfRvaAndSizes")?;
        // Clamp directory count to a sane maximum and to what the header claims
        // it can hold, so a bogus huge value cannot cause a giant allocation.
        let dir_count = number_of_rva_and_sizes.min(64) as usize;
        if number_of_rva_and_sizes > 16 {
            notes.push(format!(
                "NumberOfRvaAndSizes = {number_of_rva_and_sizes} (standard is 16); parsing first {dir_count}"
            ));
        }
        let mut data_directories = Vec::with_capacity(dir_count);
        for i in 0..dir_count {
            let base = opt_off + 112 + i * 8;
            data_directories.push(DataDirectory {
                virtual_address: r.u32_at(base, "DataDirectory.VirtualAddress")?,
                size: r.u32_at(base + 4, "DataDirectory.Size")?,
            });
        }

        let opt = OptionalHeader {
            magic,
            major_linker_version: r.u8_at(opt_off + 2, "MajorLinkerVersion")?,
            minor_linker_version: r.u8_at(opt_off + 3, "MinorLinkerVersion")?,
            size_of_code: r.u32_at(opt_off + 4, "SizeOfCode")?,
            size_of_initialized_data: r.u32_at(opt_off + 8, "SizeOfInitializedData")?,
            size_of_uninitialized_data: r.u32_at(opt_off + 12, "SizeOfUninitializedData")?,
            address_of_entry_point: r.u32_at(opt_off + 16, "AddressOfEntryPoint")?,
            base_of_code: r.u32_at(opt_off + 20, "BaseOfCode")?,
            image_base: r.u64_at(opt_off + 24, "ImageBase")?,
            section_alignment: r.u32_at(opt_off + 32, "SectionAlignment")?,
            file_alignment: r.u32_at(opt_off + 36, "FileAlignment")?,
            major_os_version: r.u16_at(opt_off + 40, "MajorOperatingSystemVersion")?,
            minor_os_version: r.u16_at(opt_off + 42, "MinorOperatingSystemVersion")?,
            major_image_version: r.u16_at(opt_off + 44, "MajorImageVersion")?,
            minor_image_version: r.u16_at(opt_off + 46, "MinorImageVersion")?,
            major_subsystem_version: r.u16_at(opt_off + 48, "MajorSubsystemVersion")?,
            minor_subsystem_version: r.u16_at(opt_off + 50, "MinorSubsystemVersion")?,
            win32_version_value: r.u32_at(opt_off + 52, "Win32VersionValue")?,
            size_of_image: r.u32_at(opt_off + 56, "SizeOfImage")?,
            size_of_headers: r.u32_at(opt_off + 60, "SizeOfHeaders")?,
            checksum: r.u32_at(opt_off + 64, "CheckSum")?,
            subsystem: r.u16_at(opt_off + 68, "Subsystem")?,
            dll_characteristics: r.u16_at(opt_off + 70, "DllCharacteristics")?,
            size_of_stack_reserve: r.u64_at(opt_off + 72, "SizeOfStackReserve")?,
            size_of_stack_commit: r.u64_at(opt_off + 80, "SizeOfStackCommit")?,
            size_of_heap_reserve: r.u64_at(opt_off + 88, "SizeOfHeapReserve")?,
            size_of_heap_commit: r.u64_at(opt_off + 96, "SizeOfHeapCommit")?,
            loader_flags: r.u32_at(opt_off + 104, "LoaderFlags")?,
            number_of_rva_and_sizes,
            data_directories,
        };

        // --- Section table ---
        let sec_off = opt_off + coff.size_of_optional_header as usize;
        let n_sections = coff.number_of_sections as usize;
        if n_sections > 96 {
            // PE spec practical ceiling; guard against absurd counts.
            notes.push(format!(
                "NumberOfSections = {n_sections} exceeds the usual ceiling of 96"
            ));
        }
        let mut sections = Vec::with_capacity(n_sections.min(4096));
        for i in 0..n_sections {
            let b = sec_off + i * 40;
            let name_bytes = r.slice_at(b, 8, "Section.Name")?;
            let mut name_raw = [0u8; 8];
            name_raw.copy_from_slice(name_bytes);
            let name = {
                let end = name_raw.iter().position(|&c| c == 0).unwrap_or(8);
                String::from_utf8_lossy(&name_raw[..end]).into_owned()
            };
            sections.push(Section {
                name_raw,
                name,
                virtual_size: r.u32_at(b + 8, "Section.VirtualSize")?,
                virtual_address: r.u32_at(b + 12, "Section.VirtualAddress")?,
                size_of_raw_data: r.u32_at(b + 16, "Section.SizeOfRawData")?,
                pointer_to_raw_data: r.u32_at(b + 20, "Section.PointerToRawData")?,
                pointer_to_relocations: r.u32_at(b + 24, "Section.PointerToRelocations")?,
                pointer_to_linenumbers: r.u32_at(b + 28, "Section.PointerToLinenumbers")?,
                number_of_relocations: r.u16_at(b + 32, "Section.NumberOfRelocations")?,
                number_of_linenumbers: r.u16_at(b + 34, "Section.NumberOfLinenumbers")?,
                characteristics: r.u32_at(b + 36, "Section.Characteristics")?,
            });
        }

        // Build a partial image so RVA mapping is available to directory parsers.
        let mut pe = PeFile {
            data,
            e_lfanew,
            coff,
            opt,
            sections,
            imports: None,
            delay_imports: None,
            exports: None,
            relocations: None,
            tls: None,
            notes,
        };

        pe.validate_invariants();

        // --- Directories (best-effort; each records a note on failure) ---
        // Collect into a separate vec so we don't alias `&pe` and `&mut pe.notes`.
        let mut dir_notes: Vec<String> = Vec::new();
        pe.imports = directories::parse_imports(&pe, &mut dir_notes);
        pe.delay_imports = directories::parse_delay_imports(&pe, &mut dir_notes);
        pe.exports = directories::parse_exports(&pe, &mut dir_notes);
        pe.relocations = directories::parse_relocations(&pe, &mut dir_notes);
        pe.tls = directories::parse_tls(&pe, &mut dir_notes);
        pe.notes.0.extend(dir_notes);

        Ok(pe)
    }

    fn validate_invariants(&mut self) {
        let opt = &self.opt;
        if opt.file_alignment != 0 && !opt.file_alignment.is_power_of_two() {
            self.notes
                .push(format!("FileAlignment {} is not a power of two", opt.file_alignment));
        }
        if opt.section_alignment != 0 && opt.section_alignment < opt.file_alignment {
            self.notes.push(format!(
                "SectionAlignment ({}) < FileAlignment ({})",
                opt.section_alignment, opt.file_alignment
            ));
        }
        if opt.address_of_entry_point == 0
            && (self.coff.characteristics & constants::IMAGE_FILE_DLL) == 0
        {
            self.notes
                .push("AddressOfEntryPoint is 0 for a non-DLL image");
        }
        // Section raw ranges should lie within the file.
        let file_len = self.data.len() as u64;
        for s in &self.sections {
            let end = s.pointer_to_raw_data as u64 + s.size_of_raw_data as u64;
            if s.size_of_raw_data != 0 && end > file_len {
                self.notes.push(format!(
                    "section '{}' raw data [{:#x}, {:#x}) extends past EOF ({:#x})",
                    s.name, s.pointer_to_raw_data, end, file_len
                ));
            }
        }
    }

    pub fn is_dll(&self) -> bool {
        self.coff.characteristics & constants::IMAGE_FILE_DLL != 0
    }

    pub fn directory(&self, index: usize) -> Option<DataDirectory> {
        self.opt.data_directories.get(index).copied()
    }

    /// Collect every base-relocation entry (rva + type). Unlike the summary in
    /// `relocations`, this returns the individual fixups a loader must apply.
    /// `ABSOLUTE` (type 0) padding entries are skipped.
    pub fn relocation_entries(&self) -> Result<Vec<RelocEntry>> {
        let mut out = Vec::new();
        let Some(dir) = self.directory(constants::DIR_BASERELOC) else {
            return Ok(out);
        };
        if dir.virtual_address == 0 || dir.size == 0 {
            return Ok(out);
        }
        let r = self.reader();
        let mut cur = dir.virtual_address;
        let end = dir.virtual_address.saturating_add(dir.size);
        while cur < end {
            let off = self.rva_to_offset(cur)?;
            let page_rva = r.u32_at(off, "RELOC.PageRVA")?;
            let block_size = r.u32_at(off + 4, "RELOC.BlockSize")?;
            if block_size < 8 {
                return Err(ParseError::Invariant {
                    what: "RELOC.BlockSize",
                    detail: format!("block at rva {cur:#x} has size {block_size}"),
                });
            }
            let n = ((block_size - 8) / 2) as usize;
            for e in 0..n {
                let entry = r.u16_at(off + 8 + e * 2, "RELOC entry")?;
                let kind = entry >> 12;
                let offset = (entry & 0x0FFF) as u32;
                if kind == 0 {
                    continue; // ABSOLUTE = padding
                }
                out.push(RelocEntry {
                    rva: page_rva.saturating_add(offset),
                    kind,
                });
            }
            cur = cur.saturating_add(block_size);
        }
        Ok(out)
    }
}

/// A single base-relocation fixup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelocEntry {
    pub rva: u32,
    pub kind: u16,
}

#[cfg(test)]
mod tests;
