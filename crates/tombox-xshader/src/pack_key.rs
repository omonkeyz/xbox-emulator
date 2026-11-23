use crate::hlsl::{self, InputInterp, Translated};
use crate::shader_cache::{self, Fma};
use crate::{cbv_raw_hlsl, dxc, gcn, runtime_hlsl, ShaderExtract};

pub const TRANSLATOR_HASH: &str = env!("TOMBOX_XLAT_SRC_HASH");
pub const TAG_PREFIX: &str = "TOMBOX-TRANSLATOR:";
pub const TAG_LEN: usize = 18 + 16;

pub const fn translator_tag() -> [u8; TAG_LEN] {
    let (p, h) = (TAG_PREFIX.as_bytes(), TRANSLATOR_HASH.as_bytes());
    let mut o = [0u8; TAG_LEN];
    let mut i = 0;
    while i < TAG_LEN {
        o[i] = if i < 18 { p[i] } else { h[i - 18] };
        i += 1;
    }
    o
}

pub fn find_translator_tag(bytes: &[u8]) -> Option<String> {
    let p = TAG_PREFIX.as_bytes();
    let mut i = 0;
    while i + TAG_LEN <= bytes.len() {
        let rest = &bytes[i..];
        let Some(k) = rest.windows(p.len()).position(|w| w == p) else { return None };
        let s = i + k + p.len();
        if let Some(h) = bytes.get(s..s + 16).filter(|h| h.iter().all(u8::is_ascii_hexdigit)) {
            return Some(String::from_utf8_lossy(h).into_owned());
        }
        i = s;
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CbvRaw {
    Off,
    On,
    Forced,
    Copy,
}

impl CbvRaw {
    pub fn rewrites(self) -> bool {
        matches!(self, CbvRaw::On | CbvRaw::Forced)
    }

    pub fn of(rewrite: bool, xbox_one_title: bool) -> CbvRaw {
        match (rewrite, xbox_one_title) {
            (false, _) => CbvRaw::Off,
            (true, true) => CbvRaw::On,
            (true, false) => CbvRaw::Forced,
        }
    }

    pub fn decide(setting: Option<&str>, xbox_one_title: bool) -> CbvRaw {
        let rewrite = match setting.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("auto") => xbox_one_title,
            Some("1") | Some("on") => true,
            Some("0") | Some("off") => false,
            _ => return CbvRaw::Copy,
        };
        CbvRaw::of(rewrite, xbox_one_title)
    }

    pub fn setting_from_env() -> Option<String> {
        std::env::var("TOMBOX_CBV_RAW").ok().or_else(|| std::env::var("TOMBOX_CBRAW").ok())
    }

    pub fn name(self) -> &'static str {
        match self {
            CbvRaw::Off => "off",
            CbvRaw::On => "on",
            CbvRaw::Forced => "forced",
            CbvRaw::Copy => "copy",
        }
    }

    pub fn parse(s: &str) -> Option<CbvRaw> {
        match s.trim() {
            "off" => Some(CbvRaw::Off),
            "on" => Some(CbvRaw::On),
            "forced" => Some(CbvRaw::Forced),
            "copy" => Some(CbvRaw::Copy),
            _ => None,
        }
    }

    pub fn key_tag(self) -> &'static str {
        match self {
            CbvRaw::Off => "cbraw=0",
            CbvRaw::Copy => "cbraw=copy",
            CbvRaw::On | CbvRaw::Forced => "cbraw=1",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub cbv_raw: CbvRaw,
    pub loop_grad: bool,
    pub fma: Fma,
    pub wave64: bool,
    pub wave64_path: bool,
}

impl Mode {
    pub fn line(&self) -> String {
        let f = self.fma;
        format!("translator={TRANSLATOR_HASH} cbraw={} loop_grad={} fma={}{}{}{} wave64={} wave64_path={} dxc={}", self.cbv_raw.name(), self.loop_grad as u8, f.preserve as u8, f.any as u8, f.hybrid as u8, f.f2h as u8, self.wave64 as u8, self.wave64_path as u8, dxc::identity())
    }

    pub fn parse_line(s: &str) -> Option<(String, Mode)> {
        let get = |k: &str| s.split_whitespace().find_map(|w| w.strip_prefix(k).and_then(|v| v.strip_prefix('=')));
        let fma = get("fma")?.as_bytes();
        if fma.len() != 4 {
            return None;
        }
        let b = |i: usize| fma[i] == b'1';
        let mode = Mode { cbv_raw: CbvRaw::parse(get("cbraw")?)?, loop_grad: get("loop_grad")? == "1", fma: Fma { preserve: b(0), any: b(1), hybrid: b(2), f2h: b(3) }, wave64: get("wave64") == Some("1"), wave64_path: get("wave64_path") == Some("1") };
        Some((get("translator")?.to_string(), mode))
    }
}

pub fn translate(ex: &ShaderExtract, link: Option<&InputInterp>, cbv: CbvRaw) -> Result<Translated, String> {
    let link = link.filter(|l| !l.0.is_empty());
    let mut t = hlsl::translate_linked(ex, link).map_err(|e| format!("translate: {e}"))?;
    if cbv.rewrites() {
        if cbv == CbvRaw::On && !gcn::is_durango(&ex.fssm) {
            return Err("bind: Xbox One title shader is not a Durango program; its constant buffers cannot follow the raw-SRV binding".into());
        }
        t.hlsl = cbv_raw_hlsl::rewrite_hlsl(&t.hlsl).map_err(|e| format!("bind: {e}"))?;
    }
    if cbv == CbvRaw::Copy {
        t.hlsl = cbv_raw_hlsl::rewrite_tables(&t.hlsl, ex.root_sig.as_ref()).map_err(|e| format!("bind: {e}"))?;
    }
    Ok(t)
}

pub fn finalize(hlsl: &str, rtfmt: &[u8; 8], mode: &Mode) -> String {
    let out = runtime_hlsl::finalize(hlsl, rtfmt, mode.loop_grad);
    if mode.wave64_path {
        out
    } else {
        without_wave64_path(&out)
    }
}

pub fn wave64_path_from_env() -> bool {
    std::env::var("TOMBOX_WAVE64_HOST").ok().as_deref() != Some("0")
}

pub fn without_wave64_path(src: &str) -> String {
    let kept: String = src.split_inclusive('\n').filter(|l| l.trim() != "GroupMemoryBarrier();").collect();
    kept.replace("(WaveGetLaneCount() - 1u)", "31u")
}

pub fn host_wave(src: &str, profile: &str, wave64: bool, mode: &Mode) -> (String, String) {
    let at = src.find("[numthreads(").map(|k| src[..k].rfind('\n').map_or(0, |n| n + 1));
    let Some(at) = at.filter(|_| wave64 && mode.wave64 && mode.wave64_path) else {
        return (src.to_string(), profile.to_string());
    };
    let mut out = String::with_capacity(src.len() + 16);
    out.push_str(&src[..at]);
    out.push_str("[WaveSize(64)]\n");
    out.push_str(&src[at..]);
    let mut it = profile.split('_');
    let stage = it.next().unwrap_or("cs");
    let minor = if it.next() == Some("6") { it.next().and_then(|m| m.parse::<u32>().ok()).unwrap_or(0) } else { 0 };
    (out, format!("{stage}_6_{}", minor.max(6)))
}

pub fn extra<'a>(mode: &Mode, more: &[&'a str]) -> Vec<&'a str> {
    let mut v: Vec<&'a str> = vec![mode.cbv_raw.key_tag()];
    v.extend_from_slice(more);
    v
}

pub fn key(src: &str, profile: &str, mode: &Mode, more: &[&str]) -> u128 {
    shader_cache::key(src, profile, mode.fma, &extra(mode, more).join(" "))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyParts {
    pub key: u128,
    pub translator: String,
    pub profile: String,
    pub flavor: String,
    pub fma: String,
    pub dxc: String,
    pub extra: String,
    pub hlsl_len: usize,
    pub hlsl_hash: u64,
}

impl KeyParts {
    pub fn of(src: &str, profile: &str, mode: &Mode, more: &[&str]) -> KeyParts {
        let r = shader_cache::reduced(src);
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in r.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let f = mode.fma;
        KeyParts {
            key: key(src, profile, mode, more),
            translator: TRANSLATOR_HASH.to_string(),
            profile: profile.to_string(),
            flavor: dxc::COMPILE_FLAVOR.to_string(),
            fma: format!("{}{}{}{}", f.preserve as u8, f.any as u8, f.hybrid as u8, f.f2h as u8),
            dxc: dxc::identity().replace(' ', "_"),
            extra: extra(mode, more).join("+"),
            hlsl_len: r.len(),
            hlsl_hash: h,
        }
    }

    pub fn line(&self) -> String {
        format!("key={:032x} translator={} profile={} flavor={} fma={} dxc={} extra={} hlsl_len={} hlsl_hash={:016x}", self.key, self.translator, self.profile, self.flavor.replace(' ', "_"), self.fma, self.dxc, self.extra, self.hlsl_len, self.hlsl_hash)
    }

    pub fn first_difference(a: &str, b: &str) -> Option<String> {
        let fields = |s: &str| s.split_whitespace().filter_map(|w| w.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        let (fa, fb) = (fields(a), fields(b));
        for (k, v) in &fa {
            if k == "key" {
                continue;
            }
            let o = fb.iter().find(|(kb, _)| kb == k).map(|(_, v)| v.as_str()).unwrap_or("<none>");
            if o != v {
                return Some(format!("{k}: {v} vs {o}"));
            }
        }
        None
    }
}

pub struct Keyed {
    pub stage: crate::Stage,
    pub src: String,
    pub profile: String,
    pub parts: KeyParts,
}

pub fn prelaunch(blob: &[u8], root: Option<&[u8]>, rtfmt: &[u8; 8], link: Option<&InputInterp>, mode: &Mode) -> Result<Keyed, (&'static str, String)> {
    let mut ex = crate::extract(blob).map_err(|e| ("extract", format!("extract: {e}")))?;
    if let Some(root) = root.filter(|_| !ex.has_root_sig) {
        ex.root_sig = Some(crate::parse_root_signature(root).map_err(|e| ("extract", format!("pipeline root signature: {e}")))?);
    }
    prelaunch_ex(&ex, rtfmt, link, mode)
}

pub fn prelaunch_ex(ex: &ShaderExtract, rtfmt: &[u8; 8], link: Option<&InputInterp>, mode: &Mode) -> Result<Keyed, (&'static str, String)> {
    let t = translate(ex, link, mode.cbv_raw).map_err(|e| (if e.starts_with("bind:") { "bind" } else { "translate" }, e))?;
    let src = finalize(&t.hlsl, rtfmt, mode);
    let (src, profile) = host_wave(&src, &dxc::profile_for(t.profile), t.wave64, mode);
    let parts = KeyParts::of(&src, &profile, mode, &[]);
    Ok(Keyed { stage: ex.stage.clone(), src, profile, parts })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbv_raw_decision_matches_the_runtime_switch() {
        assert_eq!(CbvRaw::decide(Some("1"), false), CbvRaw::Forced);
        assert_eq!(CbvRaw::decide(Some("on"), true), CbvRaw::On);
        assert_eq!(CbvRaw::decide(Some("0"), true), CbvRaw::Off);
        assert_eq!(CbvRaw::decide(Some("auto"), true), CbvRaw::On);
        assert_eq!(CbvRaw::decide(None, true), CbvRaw::Copy);
        assert_eq!(CbvRaw::decide(None, false), CbvRaw::Copy);
        assert_eq!(CbvRaw::decide(Some("x"), false), CbvRaw::Copy);
        assert_eq!(CbvRaw::decide(Some("copy"), true), CbvRaw::Copy);
        assert_eq!(CbvRaw::decide(Some(" COPY "), false), CbvRaw::Copy);
        for c in [CbvRaw::Off, CbvRaw::On, CbvRaw::Forced, CbvRaw::Copy] {
            assert_eq!(CbvRaw::parse(c.name()), Some(c));
        }
        assert!(!CbvRaw::Copy.rewrites() && !CbvRaw::Off.rewrites() && CbvRaw::On.rewrites() && CbvRaw::Forced.rewrites());
    }

    #[test]
    fn cbv_raw_mode_is_part_of_the_key() {
        let fma = Fma::default();
        let off = Mode { cbv_raw: CbvRaw::Off, loop_grad: true, fma, wave64: false, wave64_path: true };
        let on = Mode { cbv_raw: CbvRaw::On, ..off };
        let forced = Mode { cbv_raw: CbvRaw::Forced, ..off };
        let src = "float4 main() : SV_Target { return 1; }";
        assert_ne!(key(src, "ps_6_0", &off, &[]), key(src, "ps_6_0", &on, &[]));
        assert_eq!(key(src, "ps_6_0", &on, &[]), key(src, "ps_6_0", &forced, &[]));
        let copy = Mode { cbv_raw: CbvRaw::Copy, ..off };
        assert_ne!(key(src, "ps_6_0", &copy, &[]), key(src, "ps_6_0", &off, &[]));
        assert_ne!(key(src, "ps_6_0", &copy, &[]), key(src, "ps_6_0", &on, &[]));
        assert_eq!(key(src, "ps_6_0", &copy, &[]), shader_cache::key(src, "ps_6_0", fma, "cbraw=copy"));
        assert_eq!(key(src, "ps_6_0", &on, &[]), shader_cache::key(src, "ps_6_0", fma, "cbraw=1"));
        assert_eq!(key(src, "ps_6_0", &off, &[]), shader_cache::key(src, "ps_6_0", fma, "cbraw=0"));
        assert_eq!(key(src, "ps_6_0", &off, &["edited-plain-compile"]), shader_cache::key(src, "ps_6_0", fma, &["cbraw=0", "edited-plain-compile"].join(" ")));
    }

    #[test]
    fn mode_line_round_trips() {
        for (wave64, wave64_path) in [(false, false), (false, true), (true, true)] {
            let m = Mode { cbv_raw: CbvRaw::Forced, loop_grad: false, fma: Fma { preserve: true, any: false, hybrid: true, f2h: true }, wave64, wave64_path };
            let (t, back) = Mode::parse_line(&m.line()).unwrap();
            assert_eq!(t, TRANSLATOR_HASH);
            assert_eq!(back, m);
            let c = Mode { cbv_raw: CbvRaw::Copy, ..m };
            assert_eq!(Mode::parse_line(&c.line()).unwrap().1, c);
        }
        assert!(Mode::parse_line("translator=x cbraw=maybe loop_grad=1 fma=0000").is_none());
        let (_, old) = Mode::parse_line("translator=x cbraw=off loop_grad=1 fma=0000 dxc=y").unwrap();
        assert!(!old.wave64 && !old.wave64_path);
    }

    #[test]
    fn switching_the_wave64_path_off_restores_the_previous_translation() {
        let fenced = "void main() {\n    if (exec_lo & 1u) {\n    tb_lds[0] = v1;\n    GroupMemoryBarrier();\n    }\n    v2 = WaveReadLaneAt(v3, ((v4) & (WaveGetLaneCount() - 1u)));\n    GroupMemoryBarrierWithGroupSync();\n}\n";
        let previous = "void main() {\n    if (exec_lo & 1u) {\n    tb_lds[0] = v1;\n    }\n    v2 = WaveReadLaneAt(v3, ((v4) & 31u));\n    GroupMemoryBarrierWithGroupSync();\n}\n";
        assert_eq!(without_wave64_path(fenced), previous);
        let on = Mode { cbv_raw: CbvRaw::Off, loop_grad: false, fma: Fma::default(), wave64: true, wave64_path: true };
        let off = Mode { wave64: false, wave64_path: false, ..on };
        let cs = "[numthreads(64, 1, 1)]\nvoid main() { }\n";
        assert_eq!(host_wave(cs, "cs_6_2", true, &Mode { wave64_path: false, ..on }), (cs.to_string(), "cs_6_2".to_string()));
        assert_ne!(Mode::parse_line(&on.line()).unwrap().1, Mode::parse_line(&off.line()).unwrap().1);
        assert_ne!(key(fenced, "cs_6_6", &on, &[]), key(previous, "cs_6_6", &off, &[]));
    }

    #[test]
    fn host_wave_marks_only_wave64_compute_on_a_wave64_host() {
        let src = "groupshared uint g[4];\n[numthreads(64, 1, 1)]\nvoid main() { }\n";
        let host = Mode { cbv_raw: CbvRaw::Off, loop_grad: true, fma: Fma::default(), wave64: true, wave64_path: true };
        let narrow = Mode { wave64: false, ..host };
        let (s, p) = host_wave(src, "cs_6_2", true, &host);
        assert_eq!(s, "groupshared uint g[4];\n[WaveSize(64)]\n[numthreads(64, 1, 1)]\nvoid main() { }\n");
        assert_eq!(p, "cs_6_6");
        assert_eq!(host_wave(src, "cs_6_8", true, &host).1, "cs_6_8");
        assert_eq!(host_wave(src, "cs_6_2", true, &narrow), (src.to_string(), "cs_6_2".to_string()));
        assert_eq!(host_wave(src, "cs_6_2", false, &host), (src.to_string(), "cs_6_2".to_string()));
        let ps = "float4 main() : SV_Target { return 1; }";
        assert_eq!(host_wave(ps, "ps_6_2", true, &host), (ps.to_string(), "ps_6_2".to_string()));
        assert_ne!(key(&s, &p, &host, &[]), key(src, "cs_6_2", &narrow, &[]));
    }

    #[test]
    fn translator_tag_is_found_in_a_binary() {
        let tag = translator_tag();
        let mut bin = b"junk TOMBOX-TRANSLATOR:zz not hex ".to_vec();
        bin.extend_from_slice(&tag);
        bin.extend_from_slice(b" more");
        assert_eq!(find_translator_tag(&bin).as_deref(), Some(TRANSLATOR_HASH));
        assert_eq!(find_translator_tag(b"nothing here"), None);
        assert_eq!(TRANSLATOR_HASH.len(), 16);
    }

    #[test]
    fn first_difference_names_the_component() {
        let a = "key=1 translator=aa profile=ps_6_0 hlsl_hash=1";
        let b = "key=2 translator=bb profile=ps_6_0 hlsl_hash=2";
        assert_eq!(KeyParts::first_difference(a, b).as_deref(), Some("translator: aa vs bb"));
        assert_eq!(KeyParts::first_difference(a, a), None);
    }
}
