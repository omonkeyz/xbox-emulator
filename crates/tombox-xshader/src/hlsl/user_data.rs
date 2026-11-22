use super::Emitter;
use crate::user_data_abi::{TB_USER_DATA_DWORDS, TB_USER_DATA_REGISTER, TB_USER_DATA_SPACE};
use std::fmt::Write as _;

const XYZW: [&str; 4] = ["x", "y", "z", "w"];

impl Emitter<'_> {
    pub(super) fn ud_active(&self) -> bool {
        self.user_data && self.ex.root_sig.is_none() && self.durango.is_none()
    }

    fn ud_regs(&self) -> Vec<usize> {
        let mut r: Vec<usize> = self.regs.iter().filter_map(|n| n.strip_prefix('s')?.parse().ok()).filter(|&n: &usize| n < TB_USER_DATA_DWORDS).collect();
        r.sort_unstable();
        r
    }

    pub(super) fn ud_header(&self) -> Option<String> {
        self.ud_active().then(|| format!("cbuffer TbUserData : register(b{TB_USER_DATA_REGISTER}, space{TB_USER_DATA_SPACE}) {{ uint4 tb_ud[{}]; }};\n", TB_USER_DATA_DWORDS / 4))
    }

    pub(super) fn ud_init(&self) -> String {
        let mut s = String::new();
        if self.ud_active() {
            for n in self.ud_regs() {
                let _ = writeln!(s, "    s{n} = tb_ud[{}].{};", n / 4, XYZW[n % 4]);
            }
        }
        s
    }
}
