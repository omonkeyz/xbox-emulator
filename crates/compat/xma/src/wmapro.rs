use crate::bits::{BitWriter, Bits, Vlc};
use crate::mdct::Imdct;
use crate::tables::*;
use std::sync::OnceLock;

const MAX_CHANNELS: usize = 8;
const MAX_SUBFRAMES: usize = 32;
const MAX_BANDS: usize = 29;
const MAX_FRAMESIZE: usize = 32768;
const BLOCK_MIN_BITS: u32 = 6;
const BLOCK_MAX_BITS: u32 = 13;
const BLOCK_MAX_SIZE: usize = 1 << BLOCK_MAX_BITS;
const BLOCK_SIZES: usize = (BLOCK_MAX_BITS - BLOCK_MIN_BITS + 1) as usize;
const VEC1_SIZE: i32 = 101;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    WmaPro,
    Xma2,
}

struct Tables {
    sf: Vlc,
    sf_rl: Vlc,
    coef: [Vlc; 2],
    vec4: Vlc,
    vec2: Vlc,
    vec1: Vlc,
    sin64: [f32; 33],
    windows: Vec<Vec<f32>>,
}

fn pairs(t: &[[u8; 2]]) -> (Vec<u8>, Vec<i32>) {
    (t.iter().map(|r| r[1]).collect(), t.iter().map(|r| r[0] as i32).collect())
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let (l, s) = pairs(&SCALE_TABLE);
        let sf = Vlc::from_lengths(&l, &s, -60);
        let (l, s) = pairs(&SCALE_RL_TABLE);
        let sf_rl = Vlc::from_lengths(&l, &s, 0);
        let c0 = Vlc::from_lengths(&COEF0_LENS, &COEF0_SYMS.iter().map(|&v| v as i32).collect::<Vec<_>>(), 0);
        let (l, s) = pairs(&COEF1_TABLE);
        let c1 = Vlc::from_lengths(&l, &s, 0);
        let vec4 = Vlc::from_lengths(&VEC4_LENS, &VEC4_SYMS.iter().map(|&v| v as i32).collect::<Vec<_>>(), -1);
        let (l, s) = pairs(&VEC2_TABLE);
        let vec2 = Vlc::from_lengths(&l, &s, -1);
        let (l, s) = pairs(&VEC1_TABLE);
        let vec1 = Vlc::from_lengths(&l, &s, 0);
        let mut sin64 = [0f32; 33];
        for (i, v) in sin64.iter_mut().enumerate() {
            *v = (i as f64 * std::f64::consts::PI / 64.0).sin() as f32;
        }
        let windows = (0..=BLOCK_MAX_BITS)
            .map(|b| {
                let n = 1usize << b;
                (0..n).map(|i| ((i as f64 + 0.5) * (std::f64::consts::PI / (2.0 * n as f64))).sin() as f32).collect()
            })
            .collect();
        Tables { sf, sf_rl, coef: [c0, c1], vec4, vec2, vec1, sin64, windows }
    })
}

fn default_decorrelation(n: usize) -> &'static [f32] {
    let off = [0usize, 0, 1, 5, 14, 30, 55][n];
    &DEFAULT_DECORRELATION_MATRICES[off..off + n * n]
}

const QUANT_RANGE: i32 = 2048;

fn quant_scale(exp: i32) -> f32 {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    if (-QUANT_RANGE..QUANT_RANGE).contains(&exp) {
        let t = TABLE.get_or_init(|| (-QUANT_RANGE..QUANT_RANGE).map(|e| 10f64.powf(e as f64 / 20.0) as f32).collect());
        t[(exp + QUANT_RANGE) as usize]
    } else {
        10f64.powf(exp as f64 / 20.0) as f32
    }
}

fn log2(v: u32) -> u32 {
    if v == 0 {
        0
    } else {
        31 - v.leading_zeros()
    }
}

#[derive(Clone)]
struct Channel {
    prev_block_len: i32,
    transmit_coefs: bool,
    num_subframes: usize,
    subframe_len: [u16; MAX_SUBFRAMES],
    cur_subframe: usize,
    decoded_samples: i32,
    grouped: bool,
    quant_step: i32,
    reuse_sf: bool,
    scale_factor_step: i32,
    max_scale_factor: i32,
    saved_scale_factors: [[i32; MAX_BANDS]; 2],
    scale_factor_idx: usize,
    sf_buf: usize,
    table_idx: usize,
    coeffs: usize,
    num_vec_coeffs: usize,
    out: Vec<f32>,
}

impl Channel {
    fn new() -> Channel {
        Channel {
            prev_block_len: 0,
            transmit_coefs: false,
            num_subframes: 0,
            subframe_len: [0; MAX_SUBFRAMES],
            cur_subframe: 0,
            decoded_samples: 0,
            grouped: false,
            quant_step: 0,
            reuse_sf: false,
            scale_factor_step: 0,
            max_scale_factor: 0,
            saved_scale_factors: [[0; MAX_BANDS]; 2],
            scale_factor_idx: 0,
            sf_buf: 0,
            table_idx: 0,
            coeffs: 0,
            num_vec_coeffs: 0,
            out: vec![0.0; BLOCK_MAX_SIZE + BLOCK_MAX_SIZE / 2],
        }
    }
}

#[derive(Clone)]
struct ChGroup {
    num_channels: usize,
    transform: bool,
    transform_band: [bool; MAX_BANDS],
    matrix: [f32; MAX_CHANNELS * MAX_CHANNELS],
    channels: Vec<usize>,
}

impl ChGroup {
    fn new() -> ChGroup {
        ChGroup { num_channels: 0, transform: false, transform_band: [false; MAX_BANDS], matrix: [0.0; 64], channels: Vec::new() }
    }
}

pub struct Config {
    pub codec: Codec,
    pub sample_rate: u32,
    pub channels: usize,
    pub block_align: usize,
    pub decode_flags: u32,
    pub bits_per_sample: u32,
}

pub struct WmaPro {
    codec: Codec,
    sample_rate: u32,
    nb_channels: usize,
    block_align: usize,
    pb: BitWriter,
    tx: Vec<Imdct>,
    tmp: Vec<f32>,
    len_prefix: bool,
    drc: bool,
    bits_per_sample: u32,
    pub samples_per_frame: usize,
    pub trim_start: u32,
    pub trim_end: u32,
    log2_frame_size: u32,
    lfe_channel: i32,
    max_num_subframes: usize,
    subframe_len_bits: u32,
    max_subframe_len_bit: bool,
    min_samples_per_subframe: usize,
    num_sfb: [usize; BLOCK_SIZES],
    sfb_offsets: [[i32; MAX_BANDS]; BLOCK_SIZES],
    sf_offsets: Box<[[[usize; MAX_BANDS]; BLOCK_SIZES]]>,
    subwoofer_cutoffs: [usize; BLOCK_SIZES],
    pgb: Bits,
    next_packet_start: usize,
    packet_offset: u32,
    packet_sequence_number: u32,
    num_saved_bits: i64,
    frame_offset: i64,
    packet_loss: bool,
    pub packet_done: bool,
    gb: Bits,
    buf_bit_size: i64,
    skip_frame: bool,
    parsed_all_subframes: bool,
    pub skip_packets: u32,
    subframe_len: usize,
    channels_for_cur_subframe: usize,
    channel_indexes: [usize; MAX_CHANNELS],
    num_bands: usize,
    transmit_num_vec_coeffs: bool,
    table_idx: usize,
    esc_len: u32,
    num_chgroups: usize,
    chgroup: Vec<ChGroup>,
    channel: Vec<Channel>,
    pub frame_num: u32,
}

#[derive(Debug)]
pub enum InitError {
    Invalid(&'static str),
}

fn get_rate(codec: Codec, sr: u32) -> u32 {
    if codec != Codec::WmaPro {
        if sr > 44100 {
            48000
        } else if sr > 32000 {
            44100
        } else if sr > 24000 {
            32000
        } else {
            24000
        }
    } else {
        sr
    }
}

pub fn frame_len_bits(sample_rate: u32, version: u32, decode_flags: u32) -> u32 {
    let mut b = if sample_rate <= 16000 {
        9
    } else if sample_rate <= 22050 || (sample_rate <= 32000 && version == 1) {
        10
    } else if sample_rate <= 48000 || version < 3 {
        11
    } else if sample_rate <= 96000 {
        12
    } else {
        13
    };
    if version == 3 {
        match decode_flags & 6 {
            2 => b += 1,
            4 => b -= 1,
            6 => b -= 2,
            _ => {}
        }
    }
    b
}

impl WmaPro {
    pub fn new(cfg: &Config) -> Result<WmaPro, InitError> {
        let _ = tables();
        if cfg.block_align == 0 {
            return Err(InitError::Invalid("block_align"));
        }
        if cfg.channels == 0 || cfg.channels > MAX_CHANNELS || (cfg.codec == Codec::Xma2 && cfg.channels > 2) {
            return Err(InitError::Invalid("channels"));
        }
        if cfg.bits_per_sample == 0 || cfg.bits_per_sample > 32 {
            return Err(InitError::Invalid("bits_per_sample"));
        }
        let log2_frame_size = log2(cfg.block_align as u32) + 4;
        if log2_frame_size > 25 {
            return Err(InitError::Invalid("block_align large"));
        }
        let decode_flags = cfg.decode_flags;
        let samples_per_frame = if cfg.codec == Codec::WmaPro {
            let b = frame_len_bits(cfg.sample_rate, 3, decode_flags);
            if b > BLOCK_MAX_BITS {
                return Err(InitError::Invalid("14-bit blocks"));
            }
            1usize << b
        } else {
            512
        };
        let log2_max_num_subframes = (decode_flags & 0x38) >> 3;
        let max_num_subframes = 1usize << log2_max_num_subframes;
        let max_subframe_len_bit = max_num_subframes == 16 || max_num_subframes == 4;
        let subframe_len_bits = log2(log2_max_num_subframes) + 1;
        let num_possible = log2_max_num_subframes as usize + 1;
        let min_samples_per_subframe = samples_per_frame / max_num_subframes;
        if max_num_subframes > MAX_SUBFRAMES {
            return Err(InitError::Invalid("subframes"));
        }
        if min_samples_per_subframe < (1 << BLOCK_MIN_BITS) {
            return Err(InitError::Invalid("min subframe"));
        }
        if num_possible > BLOCK_SIZES {
            return Err(InitError::Invalid("block sizes"));
        }
        let mut s = WmaPro {
            codec: cfg.codec,
            sample_rate: cfg.sample_rate,
            nb_channels: cfg.channels,
            block_align: cfg.block_align,
            pb: BitWriter::default(),
            tx: Vec::new(),
            tmp: vec![0.0; BLOCK_MAX_SIZE],
            len_prefix: decode_flags & 0x40 != 0,
            drc: decode_flags & 0x80 != 0,
            bits_per_sample: cfg.bits_per_sample,
            samples_per_frame,
            trim_start: 0,
            trim_end: 0,
            log2_frame_size,
            lfe_channel: -1,
            max_num_subframes,
            subframe_len_bits,
            max_subframe_len_bit,
            min_samples_per_subframe,
            num_sfb: [0; BLOCK_SIZES],
            sfb_offsets: [[0; MAX_BANDS]; BLOCK_SIZES],
            sf_offsets: vec![[[0; MAX_BANDS]; BLOCK_SIZES]; BLOCK_SIZES].into_boxed_slice(),
            subwoofer_cutoffs: [0; BLOCK_SIZES],
            pgb: Bits::default(),
            next_packet_start: 0,
            packet_offset: 0,
            packet_sequence_number: 0,
            num_saved_bits: 0,
            frame_offset: 0,
            packet_loss: true,
            packet_done: false,
            gb: Bits::default(),
            buf_bit_size: 0,
            skip_frame: true,
            parsed_all_subframes: false,
            skip_packets: 0,
            subframe_len: 0,
            channels_for_cur_subframe: 0,
            channel_indexes: [0; MAX_CHANNELS],
            num_bands: 0,
            transmit_num_vec_coeffs: false,
            table_idx: 0,
            esc_len: 0,
            num_chgroups: 0,
            chgroup: vec![ChGroup::new(); MAX_CHANNELS],
            channel: vec![Channel::new(); MAX_CHANNELS],
            frame_num: 0,
        };
        for c in 0..s.nb_channels {
            s.channel[c].prev_block_len = samples_per_frame as i32;
        }
        let rate = get_rate(cfg.codec, cfg.sample_rate) as i64;
        for i in 0..num_possible {
            let subframe_len = (samples_per_frame >> i) as i32;
            let mut band = 1usize;
            s.sfb_offsets[i][0] = 0;
            let mut x = 0;
            while x < MAX_BANDS - 1 && s.sfb_offsets[i][band - 1] < subframe_len {
                let mut offset = ((subframe_len as i64 * 2 * CRITICAL_FREQ[x] as i64) / rate + 2) as i32;
                offset &= !3;
                if offset > s.sfb_offsets[i][band - 1] {
                    s.sfb_offsets[i][band] = offset;
                    band += 1;
                }
                if offset >= subframe_len {
                    break;
                }
                x += 1;
            }
            s.sfb_offsets[i][band - 1] = subframe_len;
            s.num_sfb[i] = band - 1;
            if s.num_sfb[i] == 0 {
                return Err(InitError::Invalid("num_sfb"));
            }
        }
        for i in 0..num_possible {
            for b in 0..s.num_sfb[i] {
                let offset = ((s.sfb_offsets[i][b] + s.sfb_offsets[i][b + 1] - 1) << i) >> 1;
                for x in 0..num_possible {
                    let mut v = 0;
                    while (s.sfb_offsets[x][v + 1] << x) < offset {
                        v += 1;
                        if v + 1 >= MAX_BANDS {
                            return Err(InitError::Invalid("sf_offsets"));
                        }
                    }
                    s.sf_offsets[i][x][b] = v;
                }
            }
        }
        for i in 0..BLOCK_SIZES {
            let len = 1usize << (BLOCK_MIN_BITS as usize + i);
            let scale = 1.0 / (1u64 << (BLOCK_MIN_BITS as usize + i - 1)) as f64 / (1u64 << (cfg.bits_per_sample - 1)) as f64;
            s.tx.push(Imdct::new(len, scale as f32 as f64));
        }
        for i in 0..num_possible {
            let block_size = (samples_per_frame >> i) as i64;
            let sr = cfg.sample_rate.max(1) as i64;
            let cutoff = (440 * block_size + 3 * (sr >> 1) - 1) / sr;
            s.subwoofer_cutoffs[i] = cutoff.clamp(4, block_size) as usize;
        }
        Ok(s)
    }

    pub fn set_lfe_from_mask(&mut self, channel_mask: u32) {
        if channel_mask & 8 != 0 {
            let mut l = -1;
            let mut m = 1;
            while m < 16 {
                if channel_mask & m != 0 {
                    l += 1;
                }
                m <<= 1;
            }
            self.lfe_channel = l;
        }
    }

    pub fn channels(&self) -> usize {
        self.nb_channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn decode_subframe_length(&mut self, offset: usize) -> i32 {
        if offset == self.samples_per_frame - self.min_samples_per_subframe {
            return self.min_samples_per_subframe as i32;
        }
        if self.gb.left() < 1 {
            return -1;
        }
        let mut shift = 0;
        if self.max_subframe_len_bit {
            if self.gb.read1() != 0 {
                shift = 1 + self.gb.read(self.subframe_len_bits - 1);
            }
        } else {
            shift = self.gb.read(self.subframe_len_bits);
        }
        if shift >= 32 {
            return -1;
        }
        let len = self.samples_per_frame >> shift;
        if len < self.min_samples_per_subframe || len > self.samples_per_frame {
            return -1;
        }
        len as i32
    }

    fn decode_tilehdr(&mut self) -> bool {
        let mut num_samples = [0i32; MAX_CHANNELS];
        let mut contains = [false; MAX_CHANNELS];
        let mut channels_for_cur = self.nb_channels;
        let mut fixed = false;
        let mut min_len = 0i32;
        for c in 0..self.nb_channels {
            self.channel[c].num_subframes = 0;
        }
        if self.max_num_subframes == 1 || self.gb.read1() != 0 {
            fixed = true;
        }
        loop {
            for c in 0..self.nb_channels {
                if num_samples[c] == min_len {
                    contains[c] = if fixed || channels_for_cur == 1 || min_len == (self.samples_per_frame - self.min_samples_per_subframe) as i32 {
                        true
                    } else {
                        self.gb.read1() != 0
                    };
                } else {
                    contains[c] = false;
                }
            }
            let sl = self.decode_subframe_length(min_len as usize);
            if sl <= 0 {
                return false;
            }
            min_len += sl;
            for c in 0..self.nb_channels {
                let ch = &mut self.channel[c];
                if contains[c] {
                    if ch.num_subframes >= MAX_SUBFRAMES {
                        return false;
                    }
                    ch.subframe_len[ch.num_subframes] = sl as u16;
                    num_samples[c] += sl;
                    ch.num_subframes += 1;
                    if num_samples[c] > self.samples_per_frame as i32 {
                        return false;
                    }
                } else if num_samples[c] <= min_len {
                    if num_samples[c] < min_len {
                        channels_for_cur = 0;
                        min_len = num_samples[c];
                    }
                    channels_for_cur += 1;
                }
            }
            if min_len >= self.samples_per_frame as i32 {
                break;
            }
        }
        true
    }

    fn decode_decorrelation_matrix(&mut self, g: usize) {
        let t = tables();
        let n = self.chgroup[g].num_channels;
        let mut rot = [0usize; MAX_CHANNELS * MAX_CHANNELS];
        let mut offset = 0;
        let mut m = [0f32; 64];
        for r in rot.iter_mut().take(n * (n - 1) >> 1) {
            *r = self.gb.read(6) as usize;
        }
        for i in 0..n {
            m[n * i + i] = if self.gb.read1() != 0 { 1.0 } else { -1.0 };
        }
        for i in 1..n {
            for x in 0..i {
                for y in 0..i + 1 {
                    let v1 = m[x * n + y];
                    let v2 = m[i * n + y];
                    let k = rot[offset + x];
                    let (sinv, cosv) = if k < 32 { (t.sin64[k], t.sin64[32 - k]) } else { (t.sin64[64 - k], -t.sin64[k - 32]) };
                    m[y + x * n] = v1 * sinv - v2 * cosv;
                    m[y + i * n] = v1 * cosv + v2 * sinv;
                }
            }
            offset += i;
        }
        self.chgroup[g].matrix = m;
    }

    fn decode_channel_transform(&mut self) -> bool {
        self.num_chgroups = 0;
        if self.nb_channels > 1 {
            let mut remaining = self.channels_for_cur_subframe;
            if self.gb.read1() != 0 {
                return false;
            }
            while remaining > 0 && self.num_chgroups < self.channels_for_cur_subframe {
                let g = self.num_chgroups;
                self.chgroup[g].num_channels = 0;
                self.chgroup[g].transform = false;
                self.chgroup[g].channels.clear();
                if remaining > 2 {
                    for i in 0..self.channels_for_cur_subframe {
                        let ci = self.channel_indexes[i];
                        if !self.channel[ci].grouped && self.gb.read1() != 0 {
                            self.chgroup[g].num_channels += 1;
                            self.channel[ci].grouped = true;
                            self.chgroup[g].channels.push(ci);
                        }
                    }
                } else {
                    self.chgroup[g].num_channels = remaining;
                    for i in 0..self.channels_for_cur_subframe {
                        let ci = self.channel_indexes[i];
                        if !self.channel[ci].grouped {
                            self.chgroup[g].channels.push(ci);
                        }
                        self.channel[ci].grouped = true;
                    }
                }
                let nc = self.chgroup[g].num_channels;
                if nc == 2 {
                    if self.gb.read1() != 0 {
                        if self.gb.read1() != 0 {
                            return false;
                        }
                    } else {
                        self.chgroup[g].transform = true;
                        let v = if self.nb_channels == 2 { [1.0, -1.0, 1.0, 1.0] } else { [0.70703125, -0.70703125, 0.70703125, 0.70703125] };
                        self.chgroup[g].matrix[..4].copy_from_slice(&v);
                    }
                } else if nc > 2 && self.gb.read1() != 0 {
                    self.chgroup[g].transform = true;
                    if self.gb.read1() != 0 {
                        self.decode_decorrelation_matrix(g);
                    } else if nc <= 6 {
                        self.chgroup[g].matrix[..nc * nc].copy_from_slice(default_decorrelation(nc));
                    }
                }
                if self.chgroup[g].transform {
                    if self.gb.read1() == 0 {
                        for i in 0..self.num_bands {
                            self.chgroup[g].transform_band[i] = self.gb.read1() != 0;
                        }
                    } else {
                        for i in 0..self.num_bands {
                            self.chgroup[g].transform_band[i] = true;
                        }
                    }
                }
                if nc > remaining {
                    return false;
                }
                remaining -= nc;
                self.num_chgroups += 1;
            }
        }
        true
    }

    fn large_val(gb: &mut Bits) -> u32 {
        let mut n = 8;
        if gb.read1() != 0 {
            n += 8;
            if gb.read1() != 0 {
                n += 8;
                if gb.read1() != 0 {
                    n += 7;
                }
            }
        }
        gb.read(n)
    }

    fn decode_coeffs(&mut self, c: usize) -> bool {
        let t = tables();
        let vlctable = self.gb.read1() as usize;
        let (run, level): (&[u16], &[f32]) = if vlctable != 0 { (&COEF1_RUN, &COEF1_LEVEL) } else { (&COEF0_RUN, &COEF0_LEVEL) };
        let base = self.channel[c].coeffs;
        let nvc = self.channel[c].num_vec_coeffs;
        let subframe_len = self.subframe_len;
        let esc_len = self.esc_len;
        let transmit_nvc = self.transmit_num_vec_coeffs;
        let gb = &mut self.gb;
        let out = &mut self.channel[c].out;
        let mut rl_mode = false;
        let mut cur = 0usize;
        let mut num_zeros = 0usize;
        while (transmit_nvc || !rl_mode) && cur + 3 < nvc {
            let mut vals = [0f32; 4];
            let idx = t.vec4.decode(gb);
            if idx < 0 {
                let mut i = 0;
                while i < 4 {
                    let idx2 = t.vec2.decode(gb);
                    if idx2 < 0 {
                        let mut v0 = t.vec1.decode(gb) as u32;
                        if v0 as i32 == VEC1_SIZE - 1 {
                            v0 = v0.wrapping_add(Self::large_val(gb));
                        }
                        let mut v1 = t.vec1.decode(gb) as u32;
                        if v1 as i32 == VEC1_SIZE - 1 {
                            v1 = v1.wrapping_add(Self::large_val(gb));
                        }
                        vals[i] = v0 as f32;
                        vals[i + 1] = v1 as f32;
                    } else {
                        vals[i] = (idx2 >> 4) as f32;
                        vals[i + 1] = (idx2 & 0xf) as f32;
                    }
                    i += 2;
                }
            } else {
                vals = [(idx >> 12) as f32, ((idx >> 8) & 0xf) as f32, ((idx >> 4) & 0xf) as f32, (idx & 0xf) as f32];
            }
            for v in vals {
                if v != 0.0 {
                    let neg = gb.read1() == 0;
                    out[base + cur] = if neg { -v } else { v };
                    num_zeros = 0;
                } else {
                    out[base + cur] = 0.0;
                    num_zeros += 1;
                    rl_mode |= num_zeros > subframe_len >> 8;
                }
                cur += 1;
            }
        }
        if cur < subframe_len {
            let n = subframe_len;
            for v in &mut out[base + cur..base + n] {
                *v = 0.0;
            }
            let mask = n - 1;
            let vlc = &t.coef[vlctable];
            let mut offset = cur;
            while offset < n {
                let code = vlc.decode(gb);
                if code > 1 {
                    offset += run[code as usize] as usize;
                    let neg = gb.read1() == 0;
                    let l = level[code as usize];
                    out[base + (offset & mask)] = if neg { -l } else { l };
                } else if code == 1 {
                    break;
                } else {
                    let lv = Self::large_val(gb);
                    if gb.read1() != 0 {
                        if gb.read1() != 0 {
                            if gb.read1() != 0 {
                                return false;
                            }
                            offset += gb.read(esc_len) as usize + 4;
                        } else {
                            offset += gb.read(2) as usize + 1;
                        }
                    }
                    let neg = gb.read1() == 0;
                    let v = lv as i32;
                    let v = if neg { v.wrapping_neg() } else { v };
                    out[base + (offset & mask)] = v as f32;
                }
                offset += 1;
            }
            if offset > n {
                return false;
            }
        }
        true
    }

    fn decode_scale_factors(&mut self) -> bool {
        let t = tables();
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes[i];
            let nb = self.num_bands;
            let ch = &mut self.channel[c];
            ch.sf_buf = 1 - ch.scale_factor_idx;
            if ch.reuse_sf {
                let so = self.sf_offsets[self.table_idx][ch.table_idx];
                for b in 0..nb {
                    ch.saved_scale_factors[ch.sf_buf][b] = ch.saved_scale_factors[ch.scale_factor_idx][so[b]];
                }
            }
            if ch.cur_subframe == 0 || self.gb.read1() != 0 {
                if !ch.reuse_sf {
                    ch.scale_factor_step = self.gb.read(2) as i32 + 1;
                    let mut val = 45 / ch.scale_factor_step;
                    for b in 0..nb {
                        val += t.sf.decode(&mut self.gb);
                        ch.saved_scale_factors[ch.sf_buf][b] = val;
                    }
                } else {
                    let mut i = 0usize;
                    while i < nb {
                        let idx = t.sf_rl.decode(&mut self.gb);
                        let (skip, val, sign): (usize, i32, i32);
                        if idx == 0 {
                            let code = self.gb.read(14);
                            val = (code >> 6) as i32;
                            sign = (code & 1) as i32 - 1;
                            skip = ((code & 0x3f) >> 1) as usize;
                        } else if idx == 1 {
                            break;
                        } else if idx < 0 {
                            return false;
                        } else {
                            skip = SCALE_RL_RUN[idx as usize] as usize;
                            val = SCALE_RL_LEVEL[idx as usize] as i32;
                            sign = self.gb.read1() as i32 - 1;
                        }
                        i += skip;
                        if i >= nb {
                            return false;
                        }
                        ch.saved_scale_factors[ch.sf_buf][i] += (val ^ sign) - sign;
                        i += 1;
                    }
                }
                ch.scale_factor_idx = 1 - ch.scale_factor_idx;
                ch.table_idx = self.table_idx;
                ch.reuse_sf = true;
            }
            let sf = &ch.saved_scale_factors[ch.sf_buf];
            ch.max_scale_factor = sf[..nb].iter().copied().max().unwrap_or(0);
        }
        true
    }

    fn inverse_channel_transform(&mut self) {
        let sfb = &self.sfb_offsets[self.table_idx];
        let subframe_len = self.subframe_len;
        let num_bands = self.num_bands;
        let stereo = self.nb_channels == 2;
        let channel = &mut self.channel;
        for grp in self.chgroup[..self.num_chgroups].iter() {
            if !grp.transform {
                continue;
            }
            let nc = grp.num_channels;
            let n = nc.min(grp.channels.len());
            for b in 0..num_bands {
                let start = sfb[b] as usize;
                let end = (sfb[b + 1] as usize).min(subframe_len);
                if start >= end {
                    continue;
                }
                if grp.transform_band[b] {
                    if n == 2 {
                        let (c0, c1) = (grp.channels[0], grp.channels[1]);
                        let (m00, m01, m10, m11) = (grp.matrix[0], grp.matrix[1], grp.matrix[nc], grp.matrix[nc + 1]);
                        let (o0, o1) = (channel[c0].coeffs, channel[c1].coeffs);
                        let (lo, hi) = if c0 < c1 { channel.split_at_mut(c1) } else { channel.split_at_mut(c0) };
                        let (a_ch, b_ch) = if c0 < c1 { (&mut lo[c0], &mut hi[0]) } else { (&mut hi[0], &mut lo[c1]) };
                        let xa = &mut a_ch.out[o0 + start..o0 + end];
                        let xb = &mut b_ch.out[o1 + start..o1 + end];
                        for (a, b) in xa.iter_mut().zip(xb.iter_mut()) {
                            let (d0, d1) = (*a, *b);
                            let mut s0 = 0f32;
                            s0 += d0 * m00;
                            s0 += d1 * m01;
                            let mut s1 = 0f32;
                            s1 += d0 * m10;
                            s1 += d1 * m11;
                            *a = s0;
                            *b = s1;
                        }
                    } else {
                        for y in start..end {
                            let mut data = [0f32; MAX_CHANNELS];
                            for (k, &ci) in grp.channels.iter().take(n).enumerate() {
                                data[k] = channel[ci].out[channel[ci].coeffs + y];
                            }
                            for (row, &ci) in grp.channels.iter().take(n).enumerate() {
                                let mut sum = 0f32;
                                for k in 0..n {
                                    sum += data[k] * grp.matrix[row * nc + k];
                                }
                                let off = channel[ci].coeffs + y;
                                channel[ci].out[off] = sum;
                            }
                        }
                    }
                } else if stereo {
                    for &ci in grp.channels.iter().take(2) {
                        let o = channel[ci].coeffs;
                        for v in &mut channel[ci].out[o + start..o + end] {
                            *v *= 181.0 / 128.0;
                        }
                    }
                }
            }
        }
    }

    fn window(&mut self) {
        let t = tables();
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes[i];
            let ch = &mut self.channel[c];
            let mut winlen = ch.prev_block_len as usize;
            let mut start = ch.coeffs as isize - (winlen >> 1) as isize;
            if self.subframe_len < winlen {
                start += ((winlen - self.subframe_len) >> 1) as isize;
                winlen = self.subframe_len;
            }
            let win = &t.windows[log2(winlen as u32) as usize];
            let len = winlen >> 1;
            let base = (start + len as isize) as usize;
            for k in 0..len {
                let i_ = base - len + k;
                let j_ = base + len - 1 - k;
                let s0 = ch.out[i_];
                let s1 = ch.out[j_];
                let wi = win[k];
                let wj = win[winlen - 1 - k];
                ch.out[i_] = s0 * wj - s1 * wi;
                ch.out[j_] = s0 * wi + s1 * wj;
            }
            ch.prev_block_len = self.subframe_len as i32;
        }
    }

    fn decode_subframe(&mut self) -> bool {
        let mut offset = self.samples_per_frame as i32;
        let mut subframe_len = self.samples_per_frame as i32;
        let mut total = (self.samples_per_frame * self.nb_channels) as i32;
        let mut transmit = false;
        for i in 0..self.nb_channels {
            self.channel[i].grouped = false;
            if offset > self.channel[i].decoded_samples {
                offset = self.channel[i].decoded_samples;
                subframe_len = self.channel[i].subframe_len[self.channel[i].cur_subframe.min(MAX_SUBFRAMES - 1)] as i32;
            }
        }
        self.channels_for_cur_subframe = 0;
        for i in 0..self.nb_channels {
            let cs = self.channel[i].cur_subframe.min(MAX_SUBFRAMES - 1);
            total -= self.channel[i].decoded_samples;
            if offset == self.channel[i].decoded_samples && subframe_len == self.channel[i].subframe_len[cs] as i32 {
                total -= self.channel[i].subframe_len[cs] as i32;
                self.channel[i].decoded_samples += self.channel[i].subframe_len[cs] as i32;
                self.channel_indexes[self.channels_for_cur_subframe] = i;
                self.channels_for_cur_subframe += 1;
            }
        }
        if total == 0 {
            self.parsed_all_subframes = true;
        }
        if subframe_len <= 0 {
            return false;
        }
        self.table_idx = log2((self.samples_per_frame / subframe_len as usize) as u32) as usize;
        self.num_bands = self.num_sfb[self.table_idx];
        let cutoff = self.subwoofer_cutoffs[self.table_idx];
        let off = offset as usize + (self.samples_per_frame >> 1);
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes[i];
            self.channel[c].coeffs = off;
        }
        self.subframe_len = subframe_len as usize;
        self.esc_len = log2(subframe_len as u32 - 1) + 1;
        if self.gb.read1() != 0 {
            let mut fill = self.gb.read(2) as i64;
            if fill == 0 {
                let len = self.gb.read(4);
                fill = self.gb.read(len) as i64 + 1;
            }
            if self.gb.count() + fill > self.num_saved_bits {
                return false;
            }
            self.gb.skip(fill);
        }
        if self.gb.read1() != 0 {
            return false;
        }
        if !self.decode_channel_transform() {
            return false;
        }
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes[i];
            self.channel[c].transmit_coefs = self.gb.read1() != 0;
            if self.channel[c].transmit_coefs {
                transmit = true;
            }
        }
        if transmit {
            let mut quant_step = (90 * self.bits_per_sample >> 4) as i32;
            self.transmit_num_vec_coeffs = self.gb.read1() != 0;
            if self.transmit_num_vec_coeffs {
                let num_bits = log2(((self.subframe_len + 3) / 4) as u32) + 1;
                for i in 0..self.channels_for_cur_subframe {
                    let c = self.channel_indexes[i];
                    let nvc = (self.gb.read(num_bits) as usize) << 2;
                    if nvc > self.subframe_len {
                        return false;
                    }
                    self.channel[c].num_vec_coeffs = nvc;
                }
            } else {
                for i in 0..self.channels_for_cur_subframe {
                    let c = self.channel_indexes[i];
                    self.channel[c].num_vec_coeffs = self.subframe_len;
                }
            }
            let mut step = self.gb.sread(6);
            quant_step += step;
            if step == -32 || step == 31 {
                let sign = if step == 31 { 0 } else { -1 };
                let mut quant = 0;
                loop {
                    if !(self.gb.count() + 5 < self.num_saved_bits) {
                        break;
                    }
                    step = self.gb.read(5) as i32;
                    if step != 31 {
                        break;
                    }
                    quant += 31;
                }
                quant_step += ((quant + step) ^ sign) - sign;
            }
            if self.channels_for_cur_subframe == 1 {
                let c = self.channel_indexes[0];
                self.channel[c].quant_step = quant_step;
            } else {
                let modifier_len = self.gb.read(3);
                for i in 0..self.channels_for_cur_subframe {
                    let c = self.channel_indexes[i];
                    self.channel[c].quant_step = quant_step;
                    if self.gb.read1() != 0 {
                        if modifier_len != 0 {
                            self.channel[c].quant_step += self.gb.read(modifier_len) as i32 + 1;
                        } else {
                            self.channel[c].quant_step += 1;
                        }
                    }
                }
            }
            if !self.decode_scale_factors() {
                return false;
            }
        }
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes[i];
            if self.channel[c].transmit_coefs && self.gb.count() < self.num_saved_bits {
                self.decode_coeffs(c);
            } else {
                let o = self.channel[c].coeffs;
                for v in &mut self.channel[c].out[o..o + subframe_len as usize] {
                    *v = 0.0;
                }
            }
        }
        if transmit {
            let ti = (log2(subframe_len as u32) - BLOCK_MIN_BITS) as usize;
            self.inverse_channel_transform();
            let sfb = self.sfb_offsets[self.table_idx];
            for i in 0..self.channels_for_cur_subframe {
                let c = self.channel_indexes[i];
                if c as i32 == self.lfe_channel {
                    for v in &mut self.tmp[cutoff..subframe_len as usize] {
                        *v = 0.0;
                    }
                }
                let ch = &self.channel[c];
                let sf = &ch.saved_scale_factors[ch.sf_buf];
                for b in 0..self.num_bands {
                    let end = (sfb[b + 1] as usize).min(self.subframe_len);
                    let start = sfb[b] as usize;
                    let exp = ch.quant_step - (ch.max_scale_factor - sf[b]) * ch.scale_factor_step;
                    let quant = quant_scale(exp);
                    if start < end {
                        for (d, &s) in self.tmp[start..end].iter_mut().zip(&ch.out[ch.coeffs + start..ch.coeffs + end]) {
                            *d = s * quant;
                        }
                    }
                }
                let o = ch.coeffs;
                let n = subframe_len as usize;
                let ch = &mut self.channel[c];
                self.tx[ti].half(&mut ch.out[o..o + n], &self.tmp[..n]);
            }
        }
        self.window();
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes[i];
            if self.channel[c].cur_subframe >= self.channel[c].num_subframes {
                return false;
            }
            self.channel[c].cur_subframe += 1;
        }
        true
    }

    fn decode_frame(&mut self, frames: &mut Vec<Vec<Vec<f32>>>) -> bool {
        let mut len = 0i64;
        if self.len_prefix {
            len = self.gb.read(self.log2_frame_size) as i64;
        }
        if !self.decode_tilehdr() {
            self.packet_loss = true;
            return false;
        }
        if self.nb_channels > 1 && self.gb.read1() != 0 && self.gb.read1() != 0 {
            for _ in 0..self.nb_channels * self.nb_channels {
                self.gb.skip(4);
            }
        }
        if self.drc {
            self.gb.read(8);
        }
        if self.gb.read1() != 0 {
            let b = log2((self.samples_per_frame * 2) as u32);
            if self.gb.read1() != 0 {
                self.trim_start = self.gb.read(b);
            }
            if self.gb.read1() != 0 {
                self.trim_end = self.gb.read(b);
            }
        } else {
            self.trim_start = 0;
            self.trim_end = 0;
        }
        self.parsed_all_subframes = false;
        for i in 0..self.nb_channels {
            self.channel[i].decoded_samples = 0;
            self.channel[i].cur_subframe = 0;
            self.channel[i].reuse_sf = false;
        }
        let mut guard = 0;
        while !self.parsed_all_subframes {
            guard += 1;
            if guard > 1024 || !self.decode_subframe() {
                self.packet_loss = true;
                return false;
            }
        }
        let spf = self.samples_per_frame;
        let frame: Vec<Vec<f32>> = (0..self.nb_channels).map(|i| self.channel[i].out[..spf].to_vec()).collect();
        for i in 0..self.nb_channels {
            self.channel[i].out.copy_within(spf..spf + spf / 2, 0);
        }
        if self.skip_frame {
            self.skip_frame = false;
        } else {
            frames.push(frame);
        }
        if self.len_prefix {
            if len != (self.gb.count() - self.frame_offset) + 2 {
                self.packet_loss = true;
                return false;
            }
            let n = len - (self.gb.count() - self.frame_offset) - 1;
            self.gb.skip(n);
        } else {
            while self.gb.count() < self.num_saved_bits && self.gb.read1() == 0 {}
        }
        let more = self.gb.read1() != 0;
        self.frame_num += 1;
        more
    }

    fn save_bits(&mut self, len: i64, append: bool) {
        let buflen;
        if !append {
            self.frame_offset = (self.pgb.count() & 7) as i64;
            self.num_saved_bits = self.frame_offset;
            self.pb.reset();
            buflen = (self.num_saved_bits + len + 7) >> 3;
        } else {
            buflen = (self.pb.bits as i64 + len + 7) >> 3;
        }
        if len <= 0 || buflen as usize > MAX_FRAMESIZE {
            self.packet_loss = true;
            return;
        }
        self.num_saved_bits += len;
        let mut len = len;
        if !append {
            let start = (self.pgb.count() >> 3) as usize;
            let n = self.num_saved_bits as usize;
            let src = std::mem::take(&mut self.pgb.buf);
            self.pb.copy_from(&src, start, n);
            self.pgb.buf = src;
        } else {
            let align = (8 - (self.pgb.count() & 7)).min(len);
            let v = self.pgb.read(align as u32);
            self.pb.put(align as u32, v);
            len -= align;
            let start = (self.pgb.count() >> 3) as usize;
            let src = std::mem::take(&mut self.pgb.buf);
            self.pb.copy_from(&src, start, len as usize);
            self.pgb.buf = src;
        }
        self.pgb.skip(len);
        self.gb = Bits::new(self.pb.buf.clone(), self.num_saved_bits as usize);
        self.gb.pos = self.frame_offset as usize;
    }

    fn remaining_bits(&self) -> i64 {
        self.buf_bit_size - self.pgb.count()
    }

    fn decode_packet_step(&mut self, data: &[u8], frames: &mut Vec<Vec<Vec<f32>>>) -> Result<usize, ()> {
        let mut buf_size = data.len();
        if self.packet_done || self.packet_loss {
            self.packet_done = false;
            if self.codec == Codec::WmaPro && buf_size < self.block_align {
                self.packet_loss = true;
                return Err(());
            }
            if self.codec == Codec::WmaPro {
                self.next_packet_start = buf_size - self.block_align;
                buf_size = self.block_align;
            } else {
                self.next_packet_start = buf_size - buf_size.min(self.block_align);
                buf_size = buf_size.min(self.block_align);
            }
            self.buf_bit_size = (buf_size << 3) as i64;
            self.pgb = Bits::new(data[..buf_size].to_vec(), buf_size * 8);
            let seq;
            if self.codec != Codec::Xma2 {
                seq = self.pgb.read(4);
                self.pgb.skip(2);
            } else {
                self.pgb.read(6);
                seq = 0;
            }
            let mut nbpf = self.pgb.read(self.log2_frame_size) as i64;
            if self.codec != Codec::WmaPro {
                self.pgb.skip(3);
                self.skip_packets = self.pgb.read(8);
            }
            if self.codec == Codec::WmaPro && !self.packet_loss && ((self.packet_sequence_number + 1) & 0xf) != seq {
                self.packet_loss = true;
            }
            self.packet_sequence_number = seq;
            if nbpf > 0 {
                let rem = self.buf_bit_size - self.pgb.count();
                if nbpf >= rem {
                    nbpf = rem;
                    self.packet_done = true;
                }
                self.save_bits(nbpf, true);
                if !self.packet_loss {
                    self.decode_frame(frames);
                }
            }
            if self.packet_loss {
                self.num_saved_bits = 0;
                self.packet_loss = false;
            }
        } else {
            if data.len() < self.next_packet_start {
                self.packet_loss = true;
                return Err(());
            }
            let n = data.len() - self.next_packet_start;
            self.buf_bit_size = (n << 3) as i64;
            self.pgb = Bits::new(data[..n].to_vec(), n * 8);
            self.pgb.skip(self.packet_offset as i64);
            let fs = if self.remaining_bits() > self.log2_frame_size as i64 { self.pgb.peek(self.log2_frame_size) as i64 } else { 0 };
            if self.len_prefix && self.remaining_bits() > self.log2_frame_size as i64 && fs != 0 && fs <= self.remaining_bits() {
                self.save_bits(fs, false);
                if !self.packet_loss {
                    self.packet_done = !self.decode_frame(frames);
                }
            } else if !self.len_prefix && self.num_saved_bits > self.gb.count() {
                self.packet_done = !self.decode_frame(frames);
            } else {
                self.packet_done = true;
            }
        }
        if self.remaining_bits() < 0 {
            self.packet_loss = true;
        }
        if self.packet_done && !self.packet_loss && self.remaining_bits() > 0 {
            let r = self.remaining_bits();
            self.save_bits(r, false);
        }
        self.packet_offset = (self.pgb.count() & 7) as u32;
        if self.packet_loss {
            return Err(());
        }
        Ok((self.pgb.count() >> 3) as usize)
    }

    pub fn set_skip_first_frame(&mut self, skip: bool) {
        if self.frame_num == 0 {
            self.skip_frame = skip;
        }
    }

    pub fn decode_packet(&mut self, packet: &[u8]) -> Vec<Vec<Vec<f32>>> {
        let mut frames = Vec::new();
        let mut pos = 0usize;
        let mut guard = 0;
        while pos < packet.len() && guard < 256 {
            guard += 1;
            match self.decode_packet_step(&packet[pos..], &mut frames) {
                Ok(0) => break,
                Ok(n) => pos += n,
                Err(()) => break,
            }
        }
        frames
    }
}
