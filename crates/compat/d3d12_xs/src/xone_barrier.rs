use compat_common::record;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

const RAYTRACING_AS: u32 = 0x40_0000;

const BUFFER_ONLY_STATES: u32 = 0x3;

fn fix_state(s: u32) -> u32 {
    if s & RAYTRACING_AS != 0 && (s != RAYTRACING_AS || crate::rt_x::configured_raytracing_tier() != 0) {
        s & !RAYTRACING_AS
    } else {
        s
    }
}

fn texture_state(s: u32) -> u32 {
    s & !BUFFER_ONLY_STATES
}

pub fn initial_state(dim: u32, s: u32) -> u32 {
    let mut f = if dim == 1 { s } else { texture_state(s) };
    if crate::rt_x::configured_raytracing_tier() != 0 && f & RAYTRACING_AS != 0 {
        f &= !RAYTRACING_AS;
    }
    if f != s {
        static LOGGED: AtomicUsize = AtomicUsize::new(0);
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 8 {
            record("d3d12_xs", "xone.barrier", &format!("initial state {s:#x} -> {f:#x}: translated console state (dim={dim})"));
        }
    }
    f
}

unsafe fn is_buffer(res: usize) -> bool {
    let mut d = [0u8; 0x38];
    let f: unsafe extern "system" fn(*mut c_void, *mut u8) -> *mut u8 =
        core::mem::transmute(*(*(res as *const *const usize)).add(10));
    f(res as *mut c_void, d.as_mut_ptr());
    u32::from_le_bytes(d[0..4].try_into().unwrap()) == 1
}

pub unsafe fn sanitize(n: u32, barriers: *const u8) -> Option<Vec<u8>> {
    if n == 0 || barriers.is_null() {
        return None;
    }
    let src = core::slice::from_raw_parts(barriers, n as usize * 32);
    rewrite(src, |r| is_buffer(r), |r| crate::placed::is_committed_placement(r as *mut c_void))
}

fn rewrite(src: &[u8], is_buffer: impl Fn(usize) -> bool, placed: impl Fn(usize) -> bool) -> Option<Vec<u8>> {
    let mut out: Option<Vec<u8>> = None;
    for (i, b) in src.chunks_exact(32).enumerate() {
        let mut r: [u8; 32] = b.try_into().unwrap();
        let mut keep = true;
        let rd = |o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        let ptr = |o: usize| u64::from_le_bytes(r[o..o + 8].try_into().unwrap()) as usize;
        match rd(0) {
            1 => {
                let (before, after) = (ptr(8), ptr(16));
                if placed(before) || placed(after) {
                    r[0..4].copy_from_slice(&2u32.to_le_bytes());
                    r[4..8].copy_from_slice(&0u32.to_le_bytes());
                    r[8..24].fill(0);
                    static ALIASED: AtomicUsize = AtomicUsize::new(0);
                    if ALIASED.fetch_add(1, Ordering::Relaxed) < 8 {
                        record("d3d12_xs", "xone.barrier", &format!("aliasing barrier before={before:#x} after={after:#x} -> global UAV barrier (placed resources are committed on PC, so they cannot alias)"));
                    }
                }
            }
            0 => {
                let res = ptr(8);
                let (s0, s1) = (rd(20), rd(24));
                let (mut f0, mut f1) = (fix_state(s0), fix_state(s1));
                if f0 != s0 || f1 != s1 {
                    static LOGGED: AtomicUsize = AtomicUsize::new(0);
                    if LOGGED.fetch_add(1, Ordering::Relaxed) < 8 {
                        record("d3d12_xs", "xone.barrier", &format!("state {s0:#x}->{s1:#x} becomes {f0:#x}->{f1:#x}: Xbox bit 0x400000 combined with other states (on PC it is RAYTRACING_ACCELERATION_STRUCTURE, valid only alone)"));
                    }
                }
                if (f0 | f1) & BUFFER_ONLY_STATES != 0 && res != 0 && !is_buffer(res) {
                    let (t0, t1) = (texture_state(f0), texture_state(f1));
                    static TEX: AtomicUsize = AtomicUsize::new(0);
                    if TEX.fetch_add(1, Ordering::Relaxed) < 8 {
                        record("d3d12_xs", "xone.barrier", &format!("texture {res:#x}: {f0:#x}->{f1:#x} becomes {t0:#x}->{t1:#x} (vertex/constant/index buffer states are buffer-only on PC)"));
                    }
                    (f0, f1) = (t0, t1);
                }
                keep = f0 != f1 || s0 == s1;
                r[20..24].copy_from_slice(&f0.to_le_bytes());
                r[24..28].copy_from_slice(&f1.to_le_bytes());
            }
            _ => {}
        }
        if !keep || r[..] != *b {
            let v = out.get_or_insert_with(|| {
                let mut v = Vec::with_capacity(src.len());
                v.extend_from_slice(&src[..i * 32]);
                v
            });
            if keep {
                v.extend_from_slice(&r);
            }
        } else if let Some(v) = &mut out {
            v.extend_from_slice(b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transition(res: u64, flags: u32, before: u32, after: u32) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[4..8].copy_from_slice(&flags.to_le_bytes());
        b[8..16].copy_from_slice(&res.to_le_bytes());
        b[16..20].copy_from_slice(&7u32.to_le_bytes());
        b[20..24].copy_from_slice(&before.to_le_bytes());
        b[24..28].copy_from_slice(&after.to_le_bytes());
        b
    }

    fn opaque(ty: u32, fill: u8) -> [u8; 32] {
        let mut b = [fill; 32];
        b[0..4].copy_from_slice(&ty.to_le_bytes());
        b[4..8].copy_from_slice(&0u32.to_le_bytes());
        b
    }

    fn states(r: &[u8]) -> (u32, u32, u32, u32) {
        let rd = |o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        (rd(0), rd(4), rd(20), rd(24))
    }

    const TEX: u64 = 0x2000;
    const BUF: u64 = 0x1000;
    fn buffers(r: usize) -> bool {
        r == BUF as usize
    }

    #[test]
    fn strips_combined_xbox_bit_only() {
        assert_eq!(fix_state(0x4000c0), 0xc0);
        let expected_as = if crate::rt_x::configured_raytracing_tier() != 0 { 0 } else { 0x400000 };
        assert_eq!(fix_state(0x400000), expected_as);
        assert_eq!(initial_state(1, 0x400000), expected_as);
        assert_eq!(fix_state(0xc0), 0xc0);
        let b = transition(BUF, 0, 0x4, 0x4000c0);
        let out = rewrite(&b, buffers, |_| false).expect("rewritten");
        assert_eq!(states(&out), (0, 0, 0x4, 0xc0));
        let clean = transition(BUF, 0, 0x4, 0xc0);
        assert!(rewrite(&clean, buffers, |_| false).is_none());
    }

    #[test]
    fn aliasing_without_placed_resource_is_kept() {
        let b = opaque(1, 0);
        assert!(unsafe { sanitize(1, b.as_ptr()) }.is_none());
    }

    #[test]
    fn textures_lose_only_buffer_only_read_states() {
        assert_eq!(texture_state(0x8c3), 0x8c0);
        assert_eq!(texture_state(0xac3), 0xac0);
        assert_eq!(texture_state(0x4), 0x4);
        assert_eq!(texture_state(0x3), 0x0);
        assert_eq!(initial_state(1, 0xac3), 0xac3);
        assert_eq!(initial_state(3, 0xac3), 0xac0);
        assert_eq!(initial_state(4, 0x8c3), 0x8c0);
    }

    #[test]
    fn mixed_batch_keeps_order_flags_and_drops_collapsed() {
        let batch = [
            opaque(2, 0x22),
            transition(TEX, 1, 0x4, 0x8c3),
            transition(TEX, 0, 0x81, 0x80),
            opaque(1, 0),
            transition(BUF, 0, 0x4, 0xac3),
            transition(TEX, 2, 0x4, 0x8c3),
            transition(TEX, 0, 0x2, 0x1),
            transition(TEX, 0, 0x80, 0x80),
        ]
        .concat();
        let out = rewrite(&batch, buffers, |_| false).expect("rewritten");
        let got: Vec<_> = out.chunks_exact(32).map(states).collect();
        assert_eq!(got, [
            (2, 0, 0x2222_2222, 0x2222_2222),
            (0, 1, 0x4, 0x8c0),
            (1, 0, 0, 0),
            (0, 0, 0x4, 0xac3),
            (0, 2, 0x4, 0x8c0),
            (0, 0, 0x80, 0x80),
        ]);
        assert_eq!(out[..32], batch[..32], "opaque barrier bytes are copied verbatim");
        for r in out.chunks_exact(32).skip(1).filter(|r| r[0] == 0) {
            assert_eq!(u32::from_le_bytes(r[16..20].try_into().unwrap()), 7, "subresource kept");
        }
        assert_eq!(states(&batch[32..64]).3, 0x8c3, "input is not modified");
    }

    #[test]
    fn fully_collapsed_batch_is_empty_and_clean_batch_is_untouched() {
        let collapsed = [transition(TEX, 1, 0x81, 0x80), transition(TEX, 2, 0x81, 0x80)].concat();
        assert_eq!(rewrite(&collapsed, buffers, |_| false), Some(Vec::new()));
        let clean = [opaque(1, 0), transition(BUF, 0, 0x81, 0x80), transition(TEX, 0, 0x4, 0x8c0)].concat();
        assert!(rewrite(&clean, buffers, |_| false).is_none());
    }

    #[test]
    fn aliasing_on_placed_resource_becomes_uav_barrier_in_place() {
        let mut alias = opaque(1, 0);
        alias[8..16].copy_from_slice(&TEX.to_le_bytes());
        let batch = [transition(TEX, 0, 0x4, 0x8c3), alias].concat();
        let out = rewrite(&batch, buffers, |r| r == TEX as usize).expect("rewritten");
        assert_eq!(out.len(), 64);
        assert_eq!(states(&out[..32]), (0, 0, 0x4, 0x8c0));
        assert_eq!(states(&out[32..]).0, 2);
        assert!(out[40..56].iter().all(|&x| x == 0));
    }
}
