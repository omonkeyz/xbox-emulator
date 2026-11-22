pub const SOURCE: &str = "gfx1036, Ryzen 5 9600X iGPU, ROCm/HIP 5.7.1, 2026-11-22";
pub const RANGE_DIGESTS: [(&str, u64); 4] = [
    ("v_rcp_f32", 0x57f508fdac16383e),
    ("v_rcp_iflag_f32", 0x14264fc2c2346b86),
    ("v_rsq_f32", 0x5cfe42d2ddea20cd),
    ("v_sqrt_f32", 0xc696d1c206aff8a9),
];
pub const EDGE_DIGESTS: [(&str, u64); 8] = [
    ("v_rcp_f32 mode 3", 0x53823cdef062f3de),
    ("v_rcp_f32 mode 0", 0x53823cdef062f3de),
    ("v_rcp_iflag_f32 mode 3", 0x53823cdef062f3de),
    ("v_rcp_iflag_f32 mode 0", 0x53823cdef062f3de),
    ("v_rsq_f32 mode 3", 0x54c103074c7be7b6),
    ("v_rsq_f32 mode 0", 0x54c103074c7be7b6),
    ("v_sqrt_f32 mode 3", 0x42f2a23771f89191),
    ("v_sqrt_f32 mode 0", 0x42f2a23771f89191),
];
