use super::{hi64, r};
use crate::rdna2::Opnd;

fn nan64_const(lo: u32, hi: u32) -> &'static str {
    let e = hi & 0x7fff_ffff;
    if e > 0x7ff0_0000 || (e == 0x7ff0_0000 && lo != 0) { "true" } else { "false" }
}

pub(super) fn nan(ty: &str, o: &Opnd) -> Result<String, String> {
    Ok(match (ty, o) {
        ("f16", Opnd::ConstF32(_)) => "false".into(),
        ("f16", _) => format!("((({}) & 0x7fffu) > 0x7c00u)", r(o)?),
        ("f32", _) => format!("((({}) & 0x7fffffffu) > 0x7f800000u)", r(o)?),
        ("f64", Opnd::ConstF32(_)) => "false".into(),
        ("f64", Opnd::ConstInt(k)) => nan64_const(*k as u32, if *k < 0 { u32::MAX } else { 0 }).into(),
        ("f64", Opnd::Literal(k)) => nan64_const(0, *k).into(),
        ("f64", _) => {
            let (l, h) = (r(o)?, r(&hi64(o))?);
            format!("(((({h}) & 0x7fffffffu) > 0x7ff00000u) || (((({h}) & 0x7fffffffu) == 0x7ff00000u) && (({l}) != 0u)))")
        }
        (other, _) => return Err(format!("NaN test for {other} unsupported")),
    })
}

// Negated conditions (`n*`) are true on NaN, else the ordered complement relation. The NaN test
// is integer, so the relation is always an ordered compare: `!(x > y)` lowers to an unordered
// `fcmp ule`, and the NVIDIA driver fails pipeline creation (E_OUTOFMEMORY) on `ule`/`uge` of
// f16-converted operands guarded by this NaN test.
pub(super) fn cond(cc: &str, x: &str, y: &str, nan: &str) -> Result<String, String> {
    let rel = match cc {
        "f" => return Ok("false".into()),
        "tru" => return Ok("true".into()),
        "o" => return Ok(format!("(!{nan})")),
        "u" => return Ok(format!("({nan})")),
        "lt" | "nge" => format!("({x} < {y})"),
        "eq" | "nlg" => format!("({x} == {y})"),
        "le" | "ngt" => format!("({x} <= {y})"),
        "gt" | "nle" => format!("({x} > {y})"),
        "ge" | "nlt" => format!("({x} >= {y})"),
        "lg" | "neq" => format!("(({x} < {y}) || ({x} > {y}))"),
        other => return Err(format!("compare cond {other} unsupported")),
    };
    Ok(if cc.starts_with('n') { format!("({nan} || {rel})") } else { format!("({rel} && (!{nan}))") })
}

pub(super) fn either(ty: &str, a: &Opnd, b: &Opnd) -> Result<String, String> {
    Ok(format!("({} || {})", nan(ty, a)?, nan(ty, b)?))
}

pub(super) fn class_src(x: String, sign: u32, mods: u8) -> Result<String, String> {
    if mods & 0x36 != 0 {
        return Err("class compare: modifier on the class-mask operand".into());
    }
    let x = if mods & 1 != 0 { format!("(({x}) & {:#x}u)", sign - 1) } else { x };
    Ok(if mods & 8 != 0 { format!("(({x}) ^ {sign:#x}u)") } else { x })
}
