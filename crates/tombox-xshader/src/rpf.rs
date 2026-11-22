use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};

use miniz_oxide::inflate::{
    core::{decompress, inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF, DecompressorOxide},
    TINFLStatus,
};

use crate::{u32_at, Dxbc, Stage};

const RPF_HEADER_SIZE: usize = 0x40;
const NAME_LIMIT: usize = 255;
const RECORD_HEADER_SIZE: usize = 17;
const RECORD_TRAILER_SIZE: usize = 12;
const COMPRESSED_LIMIT: usize = 16 * 1024 * 1024;
const DXBC_LIMIT: usize = 64 * 1024 * 1024;

pub struct ShaderRecord<'a> {
    pub offset: u64,
    pub name: &'a str,
    pub stage: Stage,
    pub shader_model: (u32, u32),
    pub compressed_size: usize,
    pub dxbc: &'a [u8],
}

pub fn scan(
    mut archive: impl Read + Seek,
    mut visit: impl FnMut(ShaderRecord<'_>) -> io::Result<()>,
) -> io::Result<()> {
    let end = archive.seek(SeekFrom::End(0))?;
    archive.rewind()?;
    let mut archive = BufReader::with_capacity(256 * 1024, archive);
    let mut header = [0; RPF_HEADER_SIZE];
    archive.read_exact(&mut header)?;
    if &header[..4] != b"9FPR" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected an RPF9 archive"));
    }

    let mut position = RPF_HEADER_SIZE as u64;
    let mut name = Vec::with_capacity(NAME_LIMIT + 1);
    let mut compressed = Vec::new();
    let mut dxbc = Vec::new();
    let mut inflater = DecompressorOxide::new();
    while let Some(name_start) = next_name(&mut archive, &mut position, &mut name)? {
        if end.saturating_sub(position) < RECORD_HEADER_SIZE as u64 {
            break;
        }
        let mut header = [0; RECORD_HEADER_SIZE];
        archive.read_exact(&mut header)?;
        let payload_offset = position + RECORD_HEADER_SIZE as u64;
        let Some((packed, unpacked)) = record_sizes(&header, end - payload_offset) else {
            archive.seek_relative(-(RECORD_HEADER_SIZE as i64))?;
            continue;
        };

        compressed.resize(packed, 0);
        archive.read_exact(&mut compressed)?;
        dxbc.resize(unpacked, 0);
        inflater.init();
        let (status, consumed, produced) = decompress(
            &mut inflater,
            &compressed,
            &mut dxbc,
            0,
            TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
        );
        let trailer = packed - consumed;
        let valid_trailer = trailer == 0 || (name_start == 1 && trailer == RECORD_TRAILER_SIZE);
        if status == TINFLStatus::Done && produced == unpacked && valid_trailer {
            if let Some((stage, shader_model)) = xbox_program(&dxbc) {
                let name = &name[name_start..];
                visit(ShaderRecord {
                    offset: position - name.len() as u64 - 1,
                    name: std::str::from_utf8(name).expect("ASCII name"),
                    stage,
                    shader_model,
                    compressed_size: consumed,
                    dxbc: &dxbc,
                })?;
                position = payload_offset + packed as u64;
                continue;
            }
        }
        archive.seek_relative(-((RECORD_HEADER_SIZE + packed) as i64))?;
    }
    Ok(())
}

fn next_name(
    archive: &mut impl BufRead,
    position: &mut u64,
    name: &mut Vec<u8>,
) -> io::Result<Option<usize>> {
    name.clear();
    loop {
        let bytes = archive.fill_buf()?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let mut consumed = 0;
        let mut found = None;
        for &byte in bytes {
            consumed += 1;
            if byte == 0 {
                if (4..=NAME_LIMIT).contains(&name.len()) {
                    let length_prefixed = usize::from(name[0]) == name.len();
                    let start = usize::from(length_prefixed);
                    let text = &name[start..];
                    if text.len() >= 4 && text.iter().all(|byte| (b' '..=b'~').contains(byte)) {
                        found = Some(start);
                    }
                }
                if found.is_some() {
                    break;
                }
                name.clear();
            } else if name.len() <= NAME_LIMIT {
                name.push(byte);
            }
        }
        archive.consume(consumed);
        *position += consumed as u64;
        if found.is_some() {
            return Ok(found);
        }
    }
}

fn record_sizes(header: &[u8; RECORD_HEADER_SIZE], remaining: u64) -> Option<(usize, usize)> {
    let packed = u32_at(header, 1).ok()? as usize;
    let repeated = u32_at(header, 9).ok()? as usize;
    let unpacked = u32_at(header, 13).ok()? as usize;
    if packed == repeated
        && (1..=COMPRESSED_LIMIT).contains(&packed)
        && packed as u64 <= remaining
        && (32..=DXBC_LIMIT).contains(&unpacked)
    {
        Some((packed, unpacked))
    } else {
        None
    }
}

fn xbox_program(dxbc: &[u8]) -> Option<(Stage, (u32, u32))> {
    let container = Dxbc::parse(dxbc).ok()?;
    if container.total_size as usize != dxbc.len() {
        return None;
    }
    let program = container.chunk(b"DXIL")?;
    let body = &dxbc[program.offset..program.offset + program.size];
    if body.get(8..12)? != b"DXIL" || body.get(24..28)? != b"XDAT" {
        return None;
    }
    let version = u32_at(body, 0).ok()?;
    Some((Stage::from_program_version(version), ((version >> 4) & 0xf, version & 0xf)))
}
