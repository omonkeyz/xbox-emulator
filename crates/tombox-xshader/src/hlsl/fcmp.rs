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

pub(super) fn cond(cc: &str, x: &str, y: &str, nan: &str) -> Result<String, String> {
    let rel = match cc {
        "f" => return Ok("false".into()),
        "tru" => return Ok("true".into()),
        "o" => return Ok(format!("(!{nan})")),
        "u" => return Ok(format!("({nan})")),
        "lt" | "nlt" => format!("({x} < {y})"),
        "eq" | "neq" => format!("({x} == {y})"),
        "le" | "nle" => format!("({x} <= {y})"),
        "gt" | "ngt" => format!("({x} > {y})"),
        "ge" | "nge" => format!("({x} >= {y})"),
        "lg" | "nlg" => format!("(({x} < {y}) || ({x} > {y}))"),
        other => return Err(format!("compare cond {other} unsupported")),
    };
    Ok(if cc.starts_with('n') { format!("({nan} || (!{rel}))") } else { format!("({rel} && (!{nan}))") })
}

pub(super) fn either(ty: &str, a: &Opnd, b: &Opnd) -> Result<String, String> {
    Ok(format!("({} || {})", nan(ty, a)?, nan(ty, b)?))
}

/// Class-test source bits after VOP3 modifiers: abs/neg act on the sign bit of src0; the
/// class mask (src1) takes none (LLVM VOPC_Class_Profile: Src0Mod only, HasSrc1Mods = 0).
pub(super) fn class_src(x: String, sign: u32, mods: u8) -> Result<String, String> {
    if mods & 0x36 != 0 {
        return Err("class compare: modifier on the class-mask operand".into());
    }
    let x = if mods & 1 != 0 { format!("(({x}) & {:#x}u)", sign - 1) } else { x };
    Ok(if mods & 8 != 0 { format!("(({x}) ^ {sign:#x}u)") } else { x })
}
