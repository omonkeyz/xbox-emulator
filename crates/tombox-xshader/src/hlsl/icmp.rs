use super::{hi64, r};
use crate::rdna2::Opnd;

/// (low, high) dwords of a 64-bit integer compare source. Inline integers are sign-extended;
/// a 32-bit literal is sign-extended for i64 and zero-extended for u64 (LLVM
/// AMDGPUOperandSyntax, "Conversion of Integer Values"). Inline float constants have no
/// defined 64-bit integer value (the assembler refuses them), so they are refused here.
pub(super) fn pair(o: &Opnd, signed: bool) -> Result<(String, String), String> {
    Ok(match o {
        Opnd::ConstInt(k) => (format!("{:#x}u", *k as i32 as u32), format!("{:#x}u", if *k < 0 { u32::MAX } else { 0 })),
        Opnd::Literal(k) => (format!("{k:#x}u"), format!("{:#x}u", if signed && (*k as i32) < 0 { u32::MAX } else { 0 })),
        Opnd::Vgpr(_) | Opnd::Sgpr(_) | Opnd::VccLo | Opnd::ExecLo => (r(o)?, r(&hi64(o))?),
        other => return Err(format!("64-bit integer compare operand {other:?} unsupported")),
    })
}

/// 64-bit integer predicate on (low, high) dword pairs: the high dwords decide unless equal,
/// then the low dwords compare unsigned.
pub(super) fn cond64(cc: &str, signed: bool, a: (String, String), b: (String, String)) -> Result<String, String> {
    let ((al, ah), (bl, bh)) = (a, b);
    let (sa, sb) = if signed { (format!("asint({ah})"), format!("asint({bh})")) } else { (ah.clone(), bh.clone()) };
    let ord = |hop: &str, lop: &str| format!("(({sa} {hop} {sb}) || (({ah} == {bh}) && ({al} {lop} {bl})))");
    Ok(match cc {
        "f" => "false".into(),
        "lt" => ord("<", "<"),
        "eq" => format!("(({ah} == {bh}) && ({al} == {bl}))"),
        "le" => ord("<", "<="),
        "gt" => ord(">", ">"),
        "ne" => format!("(({ah} != {bh}) || ({al} != {bl}))"),
        "ge" => ord(">", ">="),
        "t" => "true".into(),
        other => return Err(format!("64-bit compare cond {other} unsupported")),
    })
}
