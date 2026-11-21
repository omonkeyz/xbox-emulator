use compat_common::S_OK;
use crate::layout::E_INVALIDARG;

pub const OBJ: usize = 0x14d0;
pub const SURF: [usize; 3] = [0x70, 0xb0, 0xf8];
pub const PTRS: [usize; 10] = [0x88, 0x90, 0xa0, 0xc8, 0xd0, 0xe0, 0x110, 0x118, 0x128, 0x13c0];

pub const FMT: [[u32; 3]; 121] = [[0,4,0],[14,7,0],[14,7,0],[14,4,0],[14,5,0],[13,7,0],[13,7,0],[13,4,0],[13,5,0],[12,7,0],[12,7,0],[12,0,0],[12,4,0],[12,1,0],[12,5,0],[11,7,0],[11,7,0],[11,4,0],[11,5,0],[22,7,0],[22,7,0],[22,7,0],[22,4,1],[9,0,0],[9,0,0],[9,4,0],[6,7,0],[10,0,0],[10,0,0],[10,9,0],[10,4,0],[10,1,0],[10,5,0],[5,7,0],[5,7,0],[5,0,0],[5,4,0],[5,1,0],[5,5,0],[4,7,0],[4,7,0],[4,7,0],[4,4,0],[4,5,0],[20,0,0],[20,0,0],[20,0,0],[20,4,1],[3,0,0],[3,0,0],[3,4,0],[3,1,0],[3,5,0],[2,7,0],[2,7,0],[2,0,0],[2,0,0],[2,4,0],[2,1,0],[2,5,0],[1,0,0],[1,0,0],[1,4,0],[1,1,0],[1,5,0],[1,0,3],[0,0,0],[34,7,0],[32,0,0],[33,0,0],[35,0,0],[35,0,0],[35,9,0],[36,0,0],[36,0,0],[36,9,0],[37,0,0],[37,0,0],[37,9,0],[38,0,0],[38,0,0],[38,1,0],[39,0,0],[39,0,0],[39,1,0],[16,0,2],[17,0,1],[10,0,1],[10,0,1],[9,0,0],[10,0,1],[10,9,1],[10,0,1],[10,9,1],[40,0,0],[40,0,0],[40,1,0],[41,0,0],[41,0,0],[41,9,0],[10,0,0],[9,0,0],[12,0,0],[1,0,0],[2,0,0],[0,0,0],[0,0,0],[33,0,0],[0,0,0],[0,0,0],[0,0,0],[1,0,0],[1,0,0],[1,0,0],[3,0,0],[19,0,1],[0,0,0],[31,7,0],[2,0,0],[2,0,0],[1,4,1]];
pub const LOG2: [u8; 64] = [0,0,1,1,2,2,2,2,2,2,2,3,3,2,4,0,1,1,1,1,2,2,3,0,3,3,3,4,3,0,2,2,2,2,2,3,4,4,3,4,4,4,0,0,0,0,0,0,0,0,1,1,2,2,2,3,3,0,1,0,0,0,0,2];

pub fn comp_count(idx: u32) -> Option<u32> {
    Some(match idx {
        1 | 2 | 4 | 24..=29 | 35..=41 | 44..=56 | 59 | 60 => 1,
        3 | 5 | 11 | 20 | 21 | 32 | 33 | 57 => 2,
        6 | 7 | 13 | 16 | 58 => 3,
        8 | 9 | 10 | 12 | 14 | 17 | 18 | 19 | 30 | 31 | 34 | 63 => 4,
        _ => return None,
    })
}

pub fn safe_format(fmt: u32) -> bool {
    fmt >= 1 && fmt <= 0x78 && comp_count(FMT[fmt as usize][0]).is_some()
}

pub struct Dcc {
    pub m: Vec<u8>,
    pub b: [[u8; 256]; 3],
}

impl Dcc {
    pub fn r8(&self, o: usize) -> u8 { self.m[o] }
    pub fn r16(&self, o: usize) -> u16 { u16::from_le_bytes([self.m[o], self.m[o + 1]]) }
    pub fn r32(&self, o: usize) -> u32 { u32::from_le_bytes(self.m[o..o + 4].try_into().unwrap()) }
    pub fn r64(&self, o: usize) -> u64 { u64::from_le_bytes(self.m[o..o + 8].try_into().unwrap()) }
    pub fn w8(&mut self, o: usize, v: u8) { self.m[o] = v; }
    pub fn w16(&mut self, o: usize, v: u16) { self.m[o..o + 2].copy_from_slice(&v.to_le_bytes()); }
    pub fn w32(&mut self, o: usize, v: u32) { self.m[o..o + 4].copy_from_slice(&v.to_le_bytes()); }
    pub fn w64(&mut self, o: usize, v: u64) { self.m[o..o + 8].copy_from_slice(&v.to_le_bytes()); }
    pub fn i32(&self, o: usize) -> i32 { self.r32(o) as i32 }

    pub fn new() -> Dcc {
        let mut d = Dcc { m: vec![0u8; OBJ], b: [[0u8; 256]; 3] };
        d.w32(0, 7);
        d.w32(4, 4);
        d.w32(8, 4);
        d.w32(0xc, 3);
        d.w32(0x14, 1);
        d.w32(0x20, 0x100);
        d.w32(0x24, 0x20);
        d.w64(0x28, 0x100);
        d.w32(0x30, 1);
        d.w32(0x40, 0x1010000);
        d.w16(0x44, 0x101);
        d.w16(0x68, 0x101);
        for s in SURF {
            d.w32(s, 1);
            d.w32(s + 4, 0x100);
            d.w64(s + 8, 1);
            d.w64(s + 0x28, 0x100);
        }
        d
    }

    pub fn init(&mut self, fmt: u32, sw: u32, dim: u32, p0: u32, p1: u32) -> Result<(), i32> {
        self.w32(0x5c, 0xf);
        if p0 == 1 {
            self.w32(0x2c, 2);
        } else if p0 == 0 {
            self.w32(0x2c, 1);
        }
        self.w32(0x28, 0x40u32.wrapping_shl(p0));
        self.w32(0x24, 0x20);
        self.w32(0x4c, p1);
        if sw == 0 {
            self.w32(0, 0x13);
        } else {
            self.w32(0, 0xf + (sw & 3));
        }
        let [idx, w4, w7] = FMT[fmt as usize];
        let bpe = 1u32 << LOG2[(idx & 63) as usize];
        let r11 = (w7.wrapping_sub(1) & !2) == 0;
        self.w32(4, bpe);
        self.w32(0xc, match w4 { 1 | 3 | 5 => 2, 7 => 0, _ => 3 });
        self.w32(0x20, match bpe { 1 => 0x40, 2 => 0x80, _ => 0x100 });
        let c = comp_count(idx).ok_or(E_INVALIDARG)?;
        self.w32(8, c);
        let v48 = match idx {
            6 => if r11 { 8 } else { 1 },
            7 => if r11 { 9 } else { 2 },
            8 => if r11 { 0xa } else { 3 },
            9 => if r11 { 0xb } else { 4 },
            16 => 0xd,
            17 => 0xf,
            18 => 0xe,
            19 => 0xc,
            30 => 6,
            31 => 5,
            34 => 7,
            _ => {
                self.w8(0x10, (w7 < 2) as u8);
                0
            }
        };
        self.w32(0x48, v48);
        self.w32(0x14, match dim { 3 => 1, 4 => 2, _ => 0 });
        self.w32(0x60, 0xf);
        self.w32(0x58, 0xff);
        Ok(())
    }
}

pub fn args_ok(fmt: u32, sw: u32) -> bool {
    (fmt as i32) > 0 && (fmt as i32) <= 0x78 && (sw as i32) <= 0x1f
}

pub fn decode(count: u32, src: &[u8], meta: &[u8], dst: &mut [u8], fmt: u32, sw: u32, dim: u32, p0: u32, p1: u32) -> i32 {
    let mut d = Dcc::new();
    if let Err(e) = d.init(fmt, sw, dim, p0, p1) {
        return e;
    }
    for i in 0..count as usize {
        d.b[1].copy_from_slice(&src[i * 256..i * 256 + 256]);
        d.w32(0x58, meta[i] as u32);
        if let Err(e) = dec::run(&mut d) {
            return e;
        }
        dst[i * 256..i * 256 + 256].copy_from_slice(&d.b[0]);
    }
    S_OK
}

pub unsafe fn decode_export(count: u32, src: *const u8, meta: *const u8, dst: *mut u8, fmt: u32, sw: u32, dim: u32, params: *const u32) -> i32 {
    if params.is_null() || src.is_null() || dst.is_null() || meta.is_null() || fmt == 0 || (fmt as i32) > 0x78 || (sw as i32) > 0x1f {
        return E_INVALIDARG;
    }
    if (fmt as i32) < 0 {
        compat_common::record(crate::M, "XGDecodeDCC", &format!("format {:#x} is negative; the console indexes its format table out of bounds here, native rejects", fmt));
        return E_INVALIDARG;
    }
    let (p0, p1) = (*params, *params.add(1));
    let mut d = Dcc::new();
    if let Err(e) = d.init(fmt, sw, dim, p0, p1) {
        return e;
    }
    for i in 0..count as usize {
        core::ptr::copy_nonoverlapping(src.add(i * 256), d.b[1].as_mut_ptr(), 256);
        let m = *meta.add(i);
        d.w32(0x58, m as u32);
        if let Err(e) = dec::run(&mut d) {
            compat_common::record(crate::M, "XGDecodeDCC", &format!("block {} meta {:#04x} format {} swizzle {}: console output for this block is undefined (integer-divide fault in its color stage, or bytes taken from uninitialised stack/buffer memory), native rejects", i, m, fmt, sw));
            return e;
        }
        core::ptr::copy_nonoverlapping(d.b[0].as_ptr(), dst.add(i * 256), 256);
    }
    S_OK
}

pub fn encode(count: u32, src: &[u8], dst: &mut [u8], meta: &mut [u8], mut sizes: Option<&mut [u32]>, fmt: u32, sw: u32, dim: u32, p0: u32, p1: u32) -> i32 {
    let mut d = Dcc::new();
    if let Err(e) = d.init(fmt, sw, dim, p0, p1) {
        return e;
    }
    for i in 0..count as usize {
        d.b[0].copy_from_slice(&src[i * 256..i * 256 + 256]);
        if let Err(e) = enc::run(&mut d) {
            return e;
        }
        let mask = d.r32(0x5c);
        let Some(segs) = bits::seg_info_checked(d.r32(0x58)) else {
            return E_INVALIDARG;
        };
        let mut acc = 0u32;
        for k in 0..4 {
            if (mask >> k) & 1 != 0 {
                acc |= segs[4 + k];
            }
        }
        let size = acc.count_ones() << 5;
        if let Some(s) = sizes.as_deref_mut() {
            s[i] = size;
        }
        dst[i * 256..i * 256 + 256].copy_from_slice(&d.b[1]);
        meta[i] = d.r8(0x58);
    }
    S_OK
}

pub mod bits;
pub mod px;
#[cfg(test)]
mod oracle_px;
#[cfg(test)]
mod oracle_dec;
#[cfg(test)]
mod oracle_enc;
pub mod dec;
pub mod enc;
