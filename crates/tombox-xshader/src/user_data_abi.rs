use crate::meta::{RootParamKind, RootSig};

pub const TB_USER_DATA_SPACE: u32 = 4_000_005;
pub const TB_USER_DATA_REGISTER: u32 = 0;
pub const TB_USER_DATA_DWORDS: usize = 32;
pub const TB_USER_DATA_BYTES: usize = 256;

pub type UserData = [u32; TB_USER_DATA_DWORDS];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootArg<'a> {
    Table(u32),
    Descriptor(u64),
    Constants(&'a [u32]),
}

pub fn param_dwords(kind: &RootParamKind) -> u32 {
    match kind {
        RootParamKind::Table(_) => 1,
        RootParamKind::Descriptor { .. } => 2,
        RootParamKind::Constants { num32, .. } => *num32,
    }
}

pub fn pack(rs: &RootSig, user_base: usize, args: &[RootArg]) -> Result<UserData, String> {
    if args.len() != rs.params.len() {
        return Err(format!("user data: {} root arguments for {} root parameters", args.len(), rs.params.len()));
    }
    let mut out = [0u32; TB_USER_DATA_DWORDS];
    let mut at = user_base;
    for (p, (param, arg)) in rs.params.iter().zip(args).enumerate() {
        let n = param_dwords(&param.kind) as usize;
        if at + n > TB_USER_DATA_DWORDS {
            return Err(format!("user data: root parameter {p} needs s{at}..s{} beyond s{}", at + n - 1, TB_USER_DATA_DWORDS - 1));
        }
        match (&param.kind, arg) {
            (RootParamKind::Table(_), RootArg::Table(ptr)) => out[at] = *ptr,
            (RootParamKind::Descriptor { .. }, RootArg::Descriptor(va)) => {
                out[at] = *va as u32;
                out[at + 1] = (*va >> 32) as u32;
            }
            (RootParamKind::Constants { .. }, RootArg::Constants(c)) if c.len() == n => out[at..at + n].copy_from_slice(c),
            (RootParamKind::Constants { .. }, RootArg::Constants(c)) => {
                return Err(format!("user data: root parameter {p} takes {n} constants, got {}", c.len()));
            }
            (kind, arg) => return Err(format!("user data: root parameter {p} is {kind:?}, argument is {arg:?}")),
        }
        at += n;
    }
    Ok(out)
}

pub fn bytes(ud: &UserData) -> [u8; TB_USER_DATA_BYTES] {
    let mut b = [0u8; TB_USER_DATA_BYTES];
    for (i, w) in ud.iter().enumerate() {
        b[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{RootParam, RootRange};

    fn rs(kinds: Vec<RootParamKind>) -> RootSig {
        RootSig { version: 1, flags: 0, params: kinds.into_iter().map(|kind| RootParam { visibility: 0, kind }).collect(), num_static_samplers: 0, static_samplers: Vec::new() }
    }

    fn table() -> RootParamKind {
        RootParamKind::Table(vec![RootRange { range_type: 0, num: 1, base_register: 0, space: 0, offset_in_table: u32::MAX }])
    }

    #[test]
    fn packs_tables_descriptors_and_constants_in_root_order() {
        let r = rs(vec![table(), RootParamKind::Descriptor { kind: 2, register: 0, space: 0 }, RootParamKind::Constants { register: 1, space: 0, num32: 3 }]);
        let ud = pack(&r, 8, &[RootArg::Table(0x1234), RootArg::Descriptor(0x0000_0007_8000_0100), RootArg::Constants(&[10, 11, 12])]).unwrap();
        assert_eq!(&ud[8..14], &[0x1234, 0x8000_0100, 7, 10, 11, 12]);
        assert!(ud[..8].iter().chain(&ud[14..]).all(|&w| w == 0));
    }

    #[test]
    fn refuses_mismatched_or_overflowing_arguments() {
        let r = rs(vec![RootParamKind::Constants { register: 0, space: 0, num32: 2 }]);
        assert!(pack(&r, 8, &[]).unwrap_err().contains("0 root arguments for 1"));
        assert!(pack(&r, 8, &[RootArg::Constants(&[1])]).unwrap_err().contains("takes 2 constants, got 1"));
        assert!(pack(&r, 8, &[RootArg::Table(1)]).unwrap_err().contains("is Constants"));
        assert!(pack(&r, 31, &[RootArg::Constants(&[1, 2])]).unwrap_err().contains("beyond s31"));
        assert_eq!(pack(&r, 30, &[RootArg::Constants(&[1, 2])]).unwrap()[30..], [1, 2]);
    }

    #[test]
    fn bytes_are_little_endian_dwords() {
        let mut ud = [0u32; TB_USER_DATA_DWORDS];
        ud[1] = 0x0403_0201;
        assert_eq!(&bytes(&ud)[4..8], &[1, 2, 3, 4]);
    }
}
