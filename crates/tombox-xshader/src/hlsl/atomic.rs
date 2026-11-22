use super::{r, Emitter};
use crate::rdna2::Opnd;

impl<'a> Emitter<'a> {
    pub(super) fn atomic_rmw(&mut self, op: &str, target: &str, vdata: Opnd) -> Result<String, String> {
        self.touch(&vdata);
        let val = r(&vdata)?;
        let t = self.tmp;
        self.tmp += 1;
        if op == "cmpswap" {
            let cmp = match vdata {
                Opnd::Vgpr(n) => Opnd::Vgpr(n + 1),
                _ => return Err("bad vdata".into()),
            };
            self.touch(&cmp);
            self.line(&format!("uint t{t} = 0u; if (exec_lo & 1u) {{ InterlockedCompareExchange({target}, {}, {val}, t{t}); }}", r(&cmp)?));
            return Ok(format!("t{t}"));
        }
        let c = format!("t{t}c");
        let looped = match op {
            "smin" => Some(format!("asuint(min(asint({c}), asint({val})))")),
            "smax" => Some(format!("asuint(max(asint({c}), asint({val})))")),
            "inc" => Some(format!("(({c} >= {val}) ? 0u : ({c} + 1u))")),
            "dec" => Some(format!("((({c} == 0u) || ({c} > {val})) ? {val} : ({c} - 1u))")),
            _ => None,
        };
        if let Some(new) = looped {
            self.line(&format!(
                "uint t{t} = 0u; if (exec_lo & 1u) {{ uint {c} = {target}; [allow_uav_condition] for (uint t{t}i = 0u; t{t}i < 1024u; t{t}i++) {{ uint t{t}p = 0u; InterlockedCompareExchange({target}, {c}, {new}, t{t}p); if (t{t}p == {c}) break; {c} = t{t}p; }} t{t} = {c}; }}"
            ));
            return Ok(format!("t{t}"));
        }
        let (func, arg) = match op {
            "add" => ("InterlockedAdd", val),
            "sub" => ("InterlockedAdd", format!("(0u - {val})")),
            "umin" => ("InterlockedMin", val),
            "umax" => ("InterlockedMax", val),
            "and" => ("InterlockedAnd", val),
            "or" => ("InterlockedOr", val),
            "xor" => ("InterlockedXor", val),
            "swap" => ("InterlockedExchange", val),
            other => return Err(format!("atomic {other} unsupported")),
        };
        self.line(&format!("uint t{t} = 0u; if (exec_lo & 1u) {{ {func}({target}, {arg}, t{t}); }}"));
        Ok(format!("t{t}"))
    }
}
