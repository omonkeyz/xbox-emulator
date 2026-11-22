//! Shader binding metadata from the Xbox `PS\0\x09` pipeline cache.
//!
//! The cache is stored as 16-byte-aligned zlib streams. Its header gives the
//! size of the pipeline, index, and metadata sections preceding the native code.
//! The shader HASH payload links the cache metadata to a DXBC. A hash can occur
//! in several metadata records, so consumers must resolve bindings in context.

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};

use miniz_oxide::inflate::{
    core::{
        decompress,
        inflate_flags::{TINFL_FLAG_PARSE_ZLIB_HEADER, TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF},
        DecompressorOxide,
    },
    TINFLStatus,
};

use crate::Dxbc;

const HEADER_SIZE: usize = 0x80;
const GRAPHICS_PSO_SIZE: usize = 0x70;
const COMPUTE_PSO_SIZE: usize = 0x18;
const INDEX_ENTRY_SIZE: usize = 8;
const USER_DATA_TAG: u32 = 0x41;
const USER_DATA_SIZE: usize = 32;
const HASH_TAG: u32 = 0x43;
const HASH_TRAILER_SIZE: usize = 32;
const ZLIB_BLOCK_SIZE: usize = 64 * 1024;
const MAX_COMPRESSED_BLOCK: usize = 128 * 1024;
const MAX_METADATA_PREFIX: usize = 128 * 1024 * 1024;

pub type PsoError = io::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserDataKind {
    Sampler,
    ConstantBuffer,
    ShaderResource,
    UnorderedAccess,
    Other(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserDataBinding {
    pub kind: UserDataKind,
    pub byte_offset: u32,
    pub register: u32,
    pub raw_fields: [u32; 4],
}

impl UserDataBinding {
    pub fn static_sampler_words(&self) -> Option<[u32; 4]> {
        if self.kind != UserDataKind::Sampler || self.byte_offset != 0
            || self.raw_fields[0] != 0 || self.descriptor_dwords() != 4 || self.user_sgpr() != 0 {
            return None;
        }
        match self.raw_fields[1] >> 16 {
            6 => Some([0x92, 0x00c0_0000, 0x1450_0000, 0]),
            11 => Some([0x1b6, 0x00c0_0000, 0x1850_0000, 0]),
            _ => None,
        }
    }

    pub fn descriptor_dwords(&self) -> u8 {
        self.raw_fields[1] as u8
    }

    pub fn user_sgpr(&self) -> u8 {
        (self.raw_fields[1] >> 8) as u8
    }

    pub fn register_space(&self) -> u32 {
        self.raw_fields[1] >> 16
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingLayout {
    pub metadata_index: u32,
    pub entries: Vec<UserDataBinding>,
}

#[derive(Debug)]
pub struct PsoCache {
    layouts: HashMap<[u8; 16], Vec<BindingLayout>>,
}

impl PsoCache {
    /// Read through the metadata section, starting at the first zlib stream.
    pub fn read(
        archive: &mut (impl Read + Seek),
        archive_offset: u64,
    ) -> Result<Self, PsoError> {
        if archive_offset % 16 != 0 {
            return Err(invalid("PSO cache archive offset must be 16-byte aligned"));
        }

        let mut pos = archive_offset;
        let mut data = Vec::new();
        let mut input = Vec::with_capacity(MAX_COMPRESSED_BLOCK);
        let mut output = vec![0; ZLIB_BLOCK_SIZE + 1];
        let mut decoder = DecompressorOxide::new();
        loop {
            archive.seek(SeekFrom::Start(pos))?;
            input.clear();
            (&mut *archive).take(MAX_COMPRESSED_BLOCK as u64).read_to_end(&mut input)?;
            decoder.init();
            let (status, consumed, produced) = decompress(
                &mut decoder,
                &input,
                &mut output,
                0,
                TINFL_FLAG_PARSE_ZLIB_HEADER | TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
            );
            if status != TINFLStatus::Done || !(1..=ZLIB_BLOCK_SIZE).contains(&produced) {
                return Err(invalid(format!("invalid PSO cache zlib stream at {pos:#x}")));
            }
            if data.len() + produced > MAX_METADATA_PREFIX {
                return Err(invalid("PSO cache metadata prefix exceeds its size limit"));
            }
            data.extend_from_slice(&output[..produced]);
            if data.len() >= HEADER_SIZE && data.len() >= Sections::from_header(&data)?.metadata_end {
                return Self::parse(&data);
            }
            pos = pos.checked_add(consumed as u64 + 15)
                .map(|next| next & !15)
                .ok_or_else(|| invalid("PSO cache archive offset overflow"))?;
        }
    }

    /// Parse the decompressed header, indices, and metadata.
    pub fn parse(decoded: &[u8]) -> Result<Self, PsoError> {
        let sections = Sections::from_header(decoded)?;
        let metadata = decoded.get(sections.metadata_start..sections.metadata_end)
            .ok_or_else(|| invalid("truncated PSO cache metadata section"))?;
        let index = &decoded[sections.metadata_index_start..sections.shader_index_start];
        let mut layouts: HashMap<[u8; 16], Vec<BindingLayout>> = HashMap::new();
        for (id, item) in index.chunks_exact(INDEX_ENTRY_SIZE).enumerate() {
            let start = word(item, 0) as usize;
            let size = word(item, 4) as usize;
            let record = metadata.get(start..).and_then(|tail| tail.get(..size))
                .ok_or_else(|| invalid(format!("metadata index {id} points outside the section")))?;
            #[cfg(test)]
            if std::env::var("TOMBOX_PSO_RECORD").ok().as_deref() == Some(&id.to_string()) {
                println!("Metadata {id} words={:x?}", record.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect::<Vec<_>>());
            }
            if let Some((shader_hash, entries)) = parse_metadata_record(record)? {
                layouts.entry(shader_hash).or_default().push(BindingLayout {
                    metadata_index: id as u32,
                    entries,
                });
            }
        }
        Ok(Self { layouts })
    }

    pub fn bindings(&self, shader_hash: &[u8; 16]) -> &[BindingLayout] {
        self.layouts.get(shader_hash).map_or(&[], Vec::as_slice)
    }
}

struct Sections {
    metadata_index_start: usize,
    shader_index_start: usize,
    metadata_start: usize,
    metadata_end: usize,
}

impl Sections {
    fn from_header(data: &[u8]) -> Result<Self, PsoError> {
        if data.len() < HEADER_SIZE || &data[..4] != b"PS\0\x09" {
            return Err(invalid("expected a PS09 cache header"));
        }
        let graphics_size = section_size(data, 0x50, GRAPHICS_PSO_SIZE)?;
        let compute_size = section_size(data, 0x54, COMPUTE_PSO_SIZE)?;
        let metadata_index_size = section_size(data, 0x58, INDEX_ENTRY_SIZE)?;
        let shader_index_size = section_size(data, 0x5c, INDEX_ENTRY_SIZE)?;
        let metadata_size = section_size(data, 0x60, 1)?;
        let metadata_index_start = HEADER_SIZE + graphics_size + compute_size;
        let shader_index_start = metadata_index_start + metadata_index_size;
        let metadata_start = shader_index_start + shader_index_size;
        let metadata_end = metadata_start + metadata_size;
        if metadata_end > MAX_METADATA_PREFIX {
            return Err(invalid("PSO cache metadata prefix exceeds its size limit"));
        }
        Ok(Self { metadata_index_start, shader_index_start, metadata_start, metadata_end })
    }
}

fn section_size(header: &[u8], offset: usize, stride: usize) -> Result<usize, PsoError> {
    (word(header, offset) as usize).checked_mul(stride)
        .filter(|&len| len <= MAX_METADATA_PREFIX)
        .ok_or_else(|| invalid("invalid PSO cache section size"))
}

fn parse_metadata_record(record: &[u8]) -> Result<Option<([u8; 16], Vec<UserDataBinding>)>, PsoError> {
    if record.len() < HASH_TRAILER_SIZE {
        return Ok(None);
    }
    let payload_end = record.len() - HASH_TRAILER_SIZE;
    let trailer = &record[payload_end..];
    if word(trailer, 0) != HASH_TAG {
        return Ok(None);
    }
    let entries_start = match word(record, 0) {
        0x2 => 16,
        0x403 => 24,
        _ => return Ok(None),
    };
    let Some(payload) = record.get(entries_start..payload_end) else {
        return Err(invalid("truncated PSO cache metadata record"));
    };
    let hash: [u8; 16] = trailer[4..20].try_into().unwrap();
    let mut entries = Vec::new();
    for item in payload.chunks(USER_DATA_SIZE) {
        if item.len() < 4 || word(item, 0) != USER_DATA_TAG {
            break;
        }
        if item.len() != USER_DATA_SIZE {
            return Err(invalid("truncated PSO cache USER_DATA record"));
        }
        let kind = match word(item, 4) {
            0x300 => UserDataKind::ShaderResource,
            0x302 => UserDataKind::ConstantBuffer,
            0x303 => UserDataKind::UnorderedAccess,
            0x401 => UserDataKind::Sampler,
            other => UserDataKind::Other(other),
        };
        let byte_offset = word(item, 8).checked_mul(4)
            .ok_or_else(|| invalid("PSO cache USER_DATA offset overflow"))?;
        entries.push(UserDataBinding {
            kind,
            byte_offset,
            register: word(item, 12),
            raw_fields: std::array::from_fn(|i| word(item, 16 + i * 4)),
        });
    }
    Ok(Some((hash, entries)))
}

/// Extract the HASH payload used by PSO metadata.
pub fn shader_hash(dxbc: &[u8]) -> Option<[u8; 16]> {
    let container = Dxbc::parse(dxbc).ok()?;
    let hash = container.chunk(b"HASH")?;
    if hash.size != 20 {
        return None;
    }
    dxbc.get(hash.offset + 4..hash.offset + hash.size)?.try_into().ok()
}

pub fn shader_feature_flags(dxbc: &[u8]) -> Option<u64> {
    let container = Dxbc::parse(dxbc).ok()?;
    let features = container.chunk(b"SFI0")?;
    if features.size != 8 {
        return None;
    }
    Some(u64::from_le_bytes(dxbc.get(features.offset..features.offset + 8)?.try_into().ok()?))
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn invalid(message: impl Into<String>) -> PsoError {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use miniz_oxide::deflate::compress_to_vec_zlib;

    use super::*;

    #[test]
    fn shader_feature_flags_validate_chunk_size() {
        let mut bytes = vec![0; 52];
        bytes[..4].copy_from_slice(b"DXBC");
        bytes[24..28].copy_from_slice(&52u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&1u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&36u32.to_le_bytes());
        bytes[36..40].copy_from_slice(b"SFI0");
        bytes[40..44].copy_from_slice(&8u32.to_le_bytes());
        bytes[44..52].copy_from_slice(&0x0204_0004u64.to_le_bytes());
        assert_eq!(shader_feature_flags(&bytes), Some(0x0204_0004));
        assert_eq!(shader_feature_flags(&bytes[..51]), None);
        bytes[40..44].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(shader_feature_flags(&bytes), None);
    }

    #[test]
    fn static_sampler_words_require_verified_forms() {
        let mut binding = UserDataBinding {
            kind: UserDataKind::Sampler, byte_offset: 0, register: 3,
            raw_fields: [0, 0x60004, u32::MAX, 0],
        };
        assert_eq!(binding.static_sampler_words(), Some([0x92, 0xc00000, 0x14500000, 0]));
        binding.raw_fields[1] = 0xb0004;
        assert_eq!(binding.static_sampler_words(), Some([0x1b6, 0xc00000, 0x18500000, 0]));
        binding.raw_fields[1] = 0xc0004;
        assert_eq!(binding.static_sampler_words(), None);
        binding.raw_fields[1] = 0x60104;
        assert_eq!(binding.static_sampler_words(), None);
        binding.raw_fields[1] = 0x60004;
        binding.kind = UserDataKind::ShaderResource;
        assert_eq!(binding.static_sampler_words(), None);
    }

    fn shader_record(stage_marker: u32, hash: [u8; 16], entries: &[[u32; 7]]) -> Vec<u8> {
        let binding_start = if stage_marker == 2 { 16 } else { 24 };
        let mut record = vec![0; binding_start];
        record[0..4].copy_from_slice(&stage_marker.to_le_bytes());
        for entry in entries {
            push_word(&mut record, USER_DATA_TAG);
            for &value in entry {
                push_word(&mut record, value);
            }
        }
        push_word(&mut record, HASH_TAG);
        record.extend_from_slice(&hash);
        record.extend_from_slice(&[0; 12]);
        record
    }

    fn decoded_cache(graphics_count: u32, records: &[&[u8]]) -> Vec<u8> {
        let mut data = vec![0; HEADER_SIZE + graphics_count as usize * GRAPHICS_PSO_SIZE];
        data[0..4].copy_from_slice(b"PS\0\x09");
        data[0x50..0x54].copy_from_slice(&graphics_count.to_le_bytes());
        data[0x58..0x5c].copy_from_slice(&(records.len() as u32).to_le_bytes());
        let mut offset = 0;
        for record in records {
            push_word(&mut data, offset);
            push_word(&mut data, record.len() as u32);
            offset += record.len() as u32;
        }
        data[0x60..0x64].copy_from_slice(&offset.to_le_bytes());
        for record in records {
            data.extend_from_slice(record);
        }
        data
    }

    fn push_word(data: &mut Vec<u8>, word: u32) {
        data.extend_from_slice(&word.to_le_bytes());
    }

    #[test]
    fn reads_multiple_aligned_streams_and_preserves_ambiguous_bindings() {
        let hash = [13; 16];
        let graphics = shader_record(2, hash, &[
            [0x300, 0x18, 320, 0x50, 0x0001_0904, u32::MAX, 0],
            [0x302, 0x78, 38, 0, 0x0001_0904, u32::MAX, 0],
        ]);
        let compute = shader_record(0x403, hash, &[
            [0x303, 0, 23, 4, 0x0000_0408, u32::MAX, 0],
            [0x401, 0, 3, 0, 0x000b_0004, u32::MAX, 0],
        ]);
        let decoded = decoded_cache(600, &[&graphics, &[0; 40], &compute]);
        let mut archive = vec![0; 16];
        for block in decoded.chunks(ZLIB_BLOCK_SIZE) {
            archive.extend(compress_to_vec_zlib(block, 5));
            archive.resize(archive.len().next_multiple_of(16), 0);
        }

        let parsed = PsoCache::read(&mut Cursor::new(archive), 16).unwrap();
        let [first, second] = parsed.bindings(&hash) else {
            panic!("expected two matching layouts");
        };
        assert_eq!(first.metadata_index, 0);
        assert_eq!(second.metadata_index, 2);
        let srv = first.entries[0];
        assert_eq!(srv.kind, UserDataKind::ShaderResource);
        assert_eq!(srv.byte_offset, 0x60);
        assert_eq!(srv.register, 320);
        assert_eq!(srv.register_space(), 1);
        assert_eq!(srv.user_sgpr(), 9);
        assert_eq!(srv.descriptor_dwords(), 4);
        assert_eq!(first.entries[1].kind, UserDataKind::ConstantBuffer);
        assert_eq!(second.entries[0].kind, UserDataKind::UnorderedAccess);
        assert_eq!(second.entries[1].kind, UserDataKind::Sampler);
        assert!(parsed.bindings(&[0; 16]).is_empty());
    }

    #[test]
    fn rejects_invalid_index_and_truncated_metadata() {
        let record = shader_record(2, [1; 16], &[]);
        let mut decoded = decoded_cache(0, &[&record]);
        assert!(PsoCache::parse(&decoded[..decoded.len() - 1]).is_err());
        decoded[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(PsoCache::parse(&decoded).is_err());
    }

}
