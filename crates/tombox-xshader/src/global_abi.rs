pub const TB_GLOBAL_SPACE: u32 = 4_000_003;
pub const TB_GLOBAL_WINDOWS: usize = 64;
pub const TB_GLOBAL_CBV: u32 = 0;
pub const TB_GLOBAL_FAULT_UAV: u32 = TB_GLOBAL_WINDOWS as u32;
pub const TB_GLOBAL_CB_DWORDS: usize = 4 * TB_GLOBAL_WINDOWS + 4;
pub const TB_GLOBAL_CB_BYTES: usize = (TB_GLOBAL_CB_DWORDS * 4).div_ceil(256) * 256;
pub const TB_GLOBAL_SRV_OFFSET: usize = 1;
pub const TB_GLOBAL_UAV_OFFSET: usize = 1 + TB_GLOBAL_WINDOWS;
pub const TB_GLOBAL_FAULT_OFFSET: usize = 1 + 2 * TB_GLOBAL_WINDOWS;
pub const TB_GLOBAL_TABLE_DESCRIPTORS: usize = 2 + 2 * TB_GLOBAL_WINDOWS;
pub const TB_GLOBAL_FAULT_BYTES: usize = 16;
pub const TB_GLOBAL_ROOT_DWORDS: u32 = 1;
pub const TB_GLOBAL_WIN_WRITABLE: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub base: u64,
    pub size: u32,
}

pub fn window_cb(win: &[Window], ms_in: [u32; 2]) -> [u32; TB_GLOBAL_CB_DWORDS] {
    let tagged: Vec<(Window, bool)> = win.iter().take(TB_GLOBAL_WINDOWS).map(|&w| (w, true)).collect();
    window_cb_flags(&tagged, ms_in)
}

pub fn window_cb_flags(win: &[(Window, bool)], ms_in: [u32; 2]) -> [u32; TB_GLOBAL_CB_DWORDS] {
    let mut c = [0u32; TB_GLOBAL_CB_DWORDS];
    let n = win.len().min(TB_GLOBAL_WINDOWS);
    for (i, &(w, writable)) in win.iter().take(n).enumerate() {
        c[4 * i] = w.base as u32;
        c[4 * i + 1] = (w.base >> 32) as u32;
        c[4 * i + 2] = w.size;
        c[4 * i + 3] = if writable { TB_GLOBAL_WIN_WRITABLE } else { 0 };
    }
    c[4 * TB_GLOBAL_WINDOWS] = ms_in[0];
    c[4 * TB_GLOBAL_WINDOWS + 1] = ms_in[1];
    c[4 * TB_GLOBAL_WINDOWS + 2] = n as u32;
    c
}

pub fn resolve(win: &[Window], va: u64) -> Option<(usize, u32)> {
    win.iter().take(TB_GLOBAL_WINDOWS).enumerate().find_map(|(i, w)| {
        let off = va.wrapping_sub(w.base);
        (w.size != 0 && va >= w.base && off < w.size as u64).then_some((i, off as u32))
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fault {
    pub count: u32,
    pub first_va: u64,
}

pub fn fault_of(rec: &[u32; 4]) -> Fault {
    Fault { count: rec[0], first_va: rec[1] as u64 | (rec[2] as u64) << 32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_table_and_resolution() {
        let w = [Window { base: 0x10_0000_0000, size: 0x100 }, Window { base: 0x7f10_0000_0000, size: 64 }];
        let c = window_cb(&w, [3, 0x1234]);
        assert_eq!(&c[..8], &[0, 0x10, 0x100, 1, 0, 0x7f10, 64, 1]);
        let cro = window_cb_flags(&[(w[0], false), (w[1], true)], [3, 0x1234]);
        assert_eq!(&cro[..8], &[0, 0x10, 0x100, 0, 0, 0x7f10, 64, 1]);
        assert_eq!(&c[4 * TB_GLOBAL_WINDOWS..4 * TB_GLOBAL_WINDOWS + 3], &[3, 0x1234, 2]);
        assert_eq!(resolve(&w, 0x10_0000_00fc), Some((0, 0xfc)));
        assert_eq!(resolve(&w, 0x10_0000_0100), None);
        assert_eq!(resolve(&w, 0x7f10_0000_0010), Some((1, 0x10)));
        assert_eq!(resolve(&w, 0x0f_ffff_fffc), None);
        assert_eq!(fault_of(&[2, 0x40, 0x7f, 0]), Fault { count: 2, first_va: 0x7f_0000_0040 });
        assert_eq!(TB_GLOBAL_ROOT_DWORDS, 1);
        assert_eq!(TB_GLOBAL_CB_BYTES % 256, 0);
        assert!(TB_GLOBAL_CB_BYTES >= TB_GLOBAL_CB_DWORDS * 4);
    }
}
