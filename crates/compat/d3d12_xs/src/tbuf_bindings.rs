use std::collections::{BTreeMap, BTreeSet};

use tombox_xshader::hlsl::TbufClass;
use tombox_xshader::meta::RootParamKind;

pub type Classes = BTreeMap<(bool, u32), TbufClass>;

#[derive(Debug, PartialEq, Eq)]
pub struct Layout(Vec<((bool, u32), (u32, u32))>);

impl Layout {
    pub fn compute(root: &[u8], slots: &[(bool, u32)]) -> Option<Self> {
        if slots.is_empty() {
            return None;
        }
        let mut static_ranges = BTreeSet::new();
        let root =
            tombox_xshader::parse_root_signature_with_range_flags(root, |param, range, flags| {
                if flags & 1 == 0 {
                    static_ranges.insert((param, range));
                }
            })
            .ok()?;
        let mut bindings = Vec::with_capacity(slots.len());
        for &(uav, slot) in slots {
            if slot & 0x8000_0000 != 0 {
                return None;
            }
            let (space, register) = (slot >> 16, slot & 0xffff);
            let mut binding = None;
            for (param, p) in root
                .params
                .iter()
                .enumerate()
                .filter(|(_, p)| p.visibility == 0)
            {
                let RootParamKind::Table(ranges) = &p.kind else {
                    continue;
                };
                let mut next = 0u32;
                for (index, range) in ranges.iter().enumerate() {
                    let start = if range.offset_in_table == u32::MAX {
                        next
                    } else {
                        range.offset_in_table
                    };
                    next = start.saturating_add(range.num);
                    if range.range_type != u32::from(uav) || range.space != space {
                        continue;
                    }
                    let Some(delta) = register.checked_sub(range.base_register) else {
                        continue;
                    };
                    if delta >= range.num {
                        continue;
                    }
                    if binding.is_some() || !static_ranges.contains(&(param, index)) {
                        return None;
                    }
                    binding = Some((param as u32, start.checked_add(delta)?));
                }
            }
            bindings.push(((uav, slot), binding?));
        }
        Some(Self(bindings))
    }

    pub fn classes(
        &self,
        mut format: impl FnMut(bool, u32, u32) -> Option<u32>,
    ) -> Option<Classes> {
        self.0
            .iter()
            .map(|&(key @ (uav, _), (param, offset))| {
                Some((key, format_class(format(uav, param, offset)?)?))
            })
            .collect()
    }
}

fn format_class(format: u32) -> Option<TbufClass> {
    match format {
        2 | 6 | 10 | 11 | 13 | 16 | 24 | 26 | 28 | 29 | 31 | 34 | 35 | 37 | 41 | 49 | 51 | 54
        | 56 | 58 | 61 | 63 => Some(TbufClass::Float),
        3 | 7 | 12 | 17 | 25 | 30 | 36 | 42 | 50 | 57 | 62 => Some(TbufClass::Uint),
        4 | 8 | 14 | 18 | 32 | 38 | 43 | 52 | 59 | 64 => Some(TbufClass::Sint),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(version: u32, first_flags: u32) -> Vec<u8> {
        let mut words = vec![version, 1, 24, 0, 0, 0, 0, 0, 36, 2, 44];
        words.extend([0, 4, 5, 2]);
        if version >= 2 {
            words.push(first_flags);
        }
        words.push(3);
        words.extend([1, 2, 0, 0]);
        if version >= 2 {
            words.push(0);
        }
        words.push(u32::MAX);
        words.into_iter().flat_map(u32::to_le_bytes).collect()
    }

    #[test]
    fn typed_compute_layout_resolves_register_spaces_and_appended_ranges() {
        let srv = (false, (2 << 16) | 7);
        let uav = (true, 1);
        let layout = Layout::compute(&root(2, 0), &[srv, uav]).unwrap();
        let classes = layout
            .classes(|is_uav, param, offset| match (is_uav, param, offset) {
                (false, 0, 5) => Some(42),
                (true, 0, 8) => Some(18),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            classes,
            Classes::from([(srv, TbufClass::Uint), (uav, TbufClass::Sint)])
        );
        assert!(layout
            .classes(|uav, _, _| if uav { Some(0) } else { Some(42) })
            .is_none());
    }

    #[test]
    fn typed_compute_layout_refuses_volatile_or_unmapped_bindings() {
        let srv = (false, (2 << 16) | 7);
        assert!(Layout::compute(&root(1, 0), &[srv]).is_none());
        assert!(Layout::compute(&root(2, 1), &[srv, (true, 1)]).is_none());
        assert!(Layout::compute(&root(2, 0), &[(false, 7)]).is_none());
        assert!(Layout::compute(&root(2, 0), &[(false, 0x8000_0000)]).is_none());
    }
}
