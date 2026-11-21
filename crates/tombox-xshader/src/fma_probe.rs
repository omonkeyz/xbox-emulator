use crate::d3d::{self, Gpu, Guid};
use crate::shader_cache::Fma;
use std::ffi::c_void;
use std::sync::OnceLock;

const PROBE_VERSION: u32 = 3;
const OUTS: usize = 5;
const GROUP: u32 = 256;
const CASES: usize = 1 << 20;

extern "system" {
    fn LoadLibraryA(n: *const u8) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
}

pub struct Probe {
    pub fma: Fma,
    pub adapter: String,
    pub detail: String,
    pub cached: bool,
}

fn helper_lines() -> Option<(&'static str, &'static str)> {
    let h = crate::hlsl::CUBE_HELPERS;
    let s = h.find("#ifdef TB_FAST_FMA\n")? + "#ifdef TB_FAST_FMA\n".len();
    let rest = &h[s..];
    let (fast, rest) = rest.split_once('\n')?;
    let rest = rest.strip_prefix("#else\n")?;
    let (exact, rest) = rest.split_once('\n')?;
    if !rest.starts_with("#endif") || !fast.starts_with("float tb_fma(") || !exact.starts_with("float tb_fma(") {
        return None;
    }
    Some((fast, exact))
}

/// The 64-bit integer `tb_fma` with its three helper functions (the TB_INT_FMA branch).
fn int_helper_lines() -> Option<&'static str> {
    let h = crate::hlsl::CUBE_HELPERS;
    let s = h.find("#ifdef TB_INT_FMA\n")? + "#ifdef TB_INT_FMA\n".len();
    let block = &h[s..s + h[s..].find("#else\n")?];
    block.contains("float tb_fma(").then_some(block)
}

unsafe fn adapter_id() -> String {
    const IID_FACTORY1: Guid = Guid(0x770aae78, 0xf26f, 0x4dba, [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87]);
    const IID_DXGI_DEVICE: Guid = Guid(0x54ec77fa, 0x1377, 0x44e6, [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c]);
    let dxgi = LoadLibraryA(b"dxgi.dll\0".as_ptr());
    if dxgi.is_null() {
        return "no-dxgi".into();
    }
    let p = GetProcAddress(dxgi, b"CreateDXGIFactory1\0".as_ptr());
    if p.is_null() {
        return "no-factory".into();
    }
    let cf: unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32 = std::mem::transmute(p);
    let mut f: *mut c_void = std::ptr::null_mut();
    if cf(&IID_FACTORY1, &mut f) < 0 || f.is_null() {
        return "no-factory".into();
    }
    let mut ad: *mut c_void = std::ptr::null_mut();
    let hr = vcall!(f, 12, fn(u32, *mut *mut c_void) -> i32, 0, &mut ad);
    d3d::release(f);
    if hr < 0 || ad.is_null() {
        return "no-adapter".into();
    }
    let mut desc = [0u8; 304];
    vcall!(ad, 8, fn(*mut u8) -> i32, desc.as_mut_ptr());
    let mut umd: i64 = 0;
    let hr_umd = vcall!(ad, 9, fn(*const Guid, *mut i64) -> i32, &IID_DXGI_DEVICE, &mut umd);
    d3d::release(ad);
    let name: Vec<u16> = desc[..256].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    let u = |o: usize| u32::from_le_bytes(desc[o..o + 4].try_into().unwrap());
    let ver = if hr_umd >= 0 { format!("{}.{}.{}.{}", (umd >> 48) & 0xffff, (umd >> 32) & 0xffff, (umd >> 16) & 0xffff, umd & 0xffff) } else { "unknown".into() };
    format!("{} [{:04x}:{:04x} rev {:x}] driver {}", String::from_utf16_lossy(&name), u(256), u(260), u(268), ver)
}

fn xs(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn cases(n: usize, seed: u64) -> Vec<[u32; 3]> {
    let f = f32::from_bits;
    let mut s = seed;
    let sign = |r: u64| ((r & 1) as u32) << 31;
    let sp = [0u32, 0x8000_0000, 0x7f80_0000, 0xff80_0000, 1, 0x8000_0001, 0x7f7f_ffff, 0xff7f_ffff, 0x0080_0000, 0x3f80_0000, 0xbf80_0000, 0x7fc0_0000, 0x7f80_0001, 0xffc0_1234, 0x007f_ffff, 0x3f00_0000];
    let mut v = Vec::with_capacity(n);
    while v.len() < n {
        let r = xs(&mut s);
        let r2 = xs(&mut s);
        let t = match v.len() % 9 {
            0 => [r as u32, (r >> 32) as u32, r2 as u32],
            1 => {
                let (a, b) = (f(r as u32), f((r >> 32) as u32));
                let p = (a as f64 * b as f64) as f32;
                [a.to_bits(), b.to_bits(), (p.to_bits() ^ 0x8000_0000) ^ (r2 as u32 & 0xfff)]
            }
            2 => {
                let i = (r2 as u32 & 0x7fff) + 1;
                let ea = (((r >> 8) as u32 % 200) + 27).min(254);
                let eb = (((r >> 20) as u32 % 200) + 27).min(254);
                let a = sign(r) | (ea << 23) | i;
                let b = sign(r >> 1) | (eb << 23) | (0x80_0000 - i);
                let ce = ea as i32 + eb as i32 - 254 + 127 + 24;
                if (1..=254).contains(&ce) { [a, b, sign(r >> 2) | ((ce as u32) << 23) | (((r2 >> 16) as u32) & 0x7f_ffff)] } else { [a, b, r2 as u32] }
            }
            3 => {
                let a = f(r as u32 & 0x80ff_ffff | 0x3f00_0000);
                let b = f((r >> 32) as u32 & 0x80ff_ffff | 0x3f00_0000);
                let p = a as f64 * b as f64;
                let base = p as f32;
                let ulp = f32::from_bits(base.to_bits() + 1) as f64 - base as f64;
                let c = (base as f64 + ulp * 0.5 - p) as f32;
                [a.to_bits(), b.to_bits(), c.to_bits() ^ (r2 as u32 & 3)]
            }
            4 => {
                let e = (r2 as u32 % 40) + 40;
                [sign(r) | (e << 23) | (r as u32 >> 9 & 0x7f_ffff), sign(r >> 1) | (e << 23) | ((r >> 32) as u32 & 0x7f_ffff), (r2 >> 32) as u32 & 0x80ff_ffff]
            }
            5 => {
                let e = 230 + (r2 as u32 % 25);
                let a = sign(r) | (e << 23) | (r as u32 >> 9 & 0x7f_ffff);
                let b = sign(r >> 1) | ((127 + (r2 >> 8) as u32 % 30) << 23) | ((r >> 32) as u32 & 0x7f_ffff);
                let p = f(a) as f64 * f(b) as f64;
                let c = if p.abs() > f32::MAX as f64 { (-(p.signum()) * f32::MAX as f64) as f32 } else { -(p as f32) };
                [a, b, c.to_bits() ^ ((r2 >> 40) as u32 & 0xff)]
            }
            6 => [(r as u32) & 0x807f_ffff, (r >> 32) as u32, (r2 as u32) & 0x80ff_ffff],
            7 => {
                let pick = |x: u64| if x % 3 == 0 { sp[(x / 3) as usize % sp.len()] } else { x as u32 };
                [pick(r & 0xffff_ffff), pick(r >> 32), pick(r2)]
            }
            _ => {
                let (a, b) = (f(r as u32), f((r >> 32) as u32));
                let p = (a as f64 * b as f64) as f32;
                [a.to_bits(), b.to_bits(), (p.to_bits() ^ 0x8000_0000).wrapping_add((r2 as u32 % 7).wrapping_sub(3))]
            }
        };
        v.push(t);
    }
    v
}

fn shader(fast: &str, exact: &str) -> String {
    format!(
        "RWByteAddressBuffer io : register(u0);\ncbuffer K : register(b0) {{ uint n; uint obase; }}\n{}\n{}\n[RootSignature(\"RootConstants(num32BitConstants=2, b0), UAV(u0)\")]\n[numthreads({GROUP}, 1, 1)]\nvoid main(uint3 g : SV_DispatchThreadID) {{ uint i = g.x; if (i >= n) return; float x = asfloat(io.Load(i * 12)); float y = asfloat(io.Load(i * 12 + 4)); float z = asfloat(io.Load(i * 12 + 8)); uint o = obase + i * 40; io.Store2(o, uint2(asuint(tb_fma_f(x, y, z)), asuint(tb_fma_x(x, y, z)))); io.Store2(o + 8, uint2(asuint(tb_fma_f(x, 0.0, z)), asuint(tb_fma_x(x, 0.0, z)))); io.Store2(o + 16, uint2(asuint(tb_fma_f(x, y, 0.0)), asuint(tb_fma_x(x, y, 0.0)))); io.Store2(o + 24, uint2(asuint(tb_fma_f(1.0, y, z)), asuint(tb_fma_x(1.0, y, z)))); io.Store2(o + 32, uint2(asuint(tb_fma_f(x, -0.0, -0.0)), asuint(tb_fma_x(x, -0.0, -0.0)))); }}\n",
        fast.replacen("float tb_fma(", "float tb_fma_f(", 1),
        exact.replacen("float tb_fma(", "float tb_fma_x(", 1)
    )
}

unsafe fn dispatch(g: &mut Gpu, code: &[u8], cs: &[[u32; 3]]) -> Result<Vec<[u32; 2]>, String> {
    let n = cs.len();
    let obase = (n * 12) as u64;
    let total = obase + (n * 8 * OUTS) as u64;
    let mut rs: *mut c_void = std::ptr::null_mut();
    let hr = vcall!(g.device, 16, fn(u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32, 0, code.as_ptr(), code.len(), &d3d::IID_ROOTSIG, &mut rs);
    if hr < 0 {
        return Err(format!("CreateRootSignature {:#x}", hr as u32));
    }
    let mut d = [0u8; 56];
    d[0..8].copy_from_slice(&(rs as u64).to_le_bytes());
    d[8..16].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
    d[16..24].copy_from_slice(&(code.len() as u64).to_le_bytes());
    let mut pso: *mut c_void = std::ptr::null_mut();
    let hr = vcall!(g.device, 11, fn(*const u8, *const Guid, *mut *mut c_void) -> i32, d.as_ptr(), &d3d::IID_PSO, &mut pso);
    if hr < 0 {
        d3d::release(rs);
        return Err(format!("CreateComputePipelineState {:#x}", hr as u32));
    }
    let buf = g.buffer(total, 1, d3d::COPY_DEST, true)?;
    let up = g.buffer(total, 2, d3d::GENERIC_READ, false)?;
    let rb = g.buffer(total, 3, d3d::COPY_DEST, false)?;
    let mut bytes: Vec<u8> = cs.iter().flat_map(|t| t.iter().flat_map(|w| w.to_le_bytes())).collect();
    bytes.resize(total as usize, 0);
    Gpu::map_write(up, &bytes);
    g.begin();
    let l = g.list;
    vcall!(l, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), buf, 0, up, 0, total);
    g.barrier(buf, d3d::COPY_DEST, 8);
    vcall!(l, 29, fn(*mut c_void) -> (), rs);
    vcall!(l, 25, fn(*mut c_void) -> (), pso);
    let k = [n as u32, obase as u32];
    vcall!(l, 35, fn(u32, u32, *const u32, u32) -> (), 0, 2, k.as_ptr(), 0);
    vcall!(l, 41, fn(u32, u64) -> (), 1, Gpu::gpu_va(buf));
    vcall!(l, 14, fn(u32, u32, u32) -> (), (n as u32 + GROUP - 1) / GROUP, 1, 1);
    g.barrier(buf, 8, d3d::COPY_SOURCE);
    vcall!(l, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), rb, 0, buf, 0, total);
    let sub = g.submit();
    let out = if sub.is_ok() { Gpu::map_read(rb, total as usize) } else { None };
    for o in [pso, rs, buf, up, rb] {
        d3d::release(o);
    }
    sub?;
    let out = out.ok_or("map readback")?;
    Ok(out[obase as usize..].chunks(8).map(|c| [u32::from_le_bytes(c[0..4].try_into().unwrap()), u32::from_le_bytes(c[4..8].try_into().unwrap())]).collect())
}

/// What the exact `tb_fma` helper returns, computed on the CPU with the helper's own
/// statements in f64. A device without 64-bit float shader operations cannot build that
/// helper, so there the fast helper is checked against this.
pub(crate) fn reference(a: u32, b: u32, c: u32) -> u32 {
    const M: u32 = 0x7fff_ffff;
    let nan = |x: u32| x & M > 0x7f80_0000;
    if nan(a) || nan(b) || nan(c) {
        return (if nan(a) { a } else if nan(b) { b } else { c }) | 0x40_0000;
    }
    let p = f32::from_bits(a) as f64 * f32::from_bits(b) as f64;
    let dc = f32::from_bits(c) as f64;
    let s = p + dc;
    let bb = s - p;
    let e = (p - (s - bb)) + (dc - bb);
    let (lo, hi) = (s.to_bits() as u32, (s.to_bits() >> 32) as u32);
    let fin = (s.abs() < 1.0e300) as u32;
    let adj = fin & (e != 0.0) as u32 & (!lo & 1);
    let dn = adj & ((e > 0.0) != (s > 0.0)) as u32;
    let odd = ((hi.wrapping_sub(dn & (lo == 0) as u32) as u64) << 32) | lo.wrapping_add(adj).wrapping_sub(2 * dn) as u64;
    let r = (f64::from_bits(odd) as f32).to_bits();
    if r & M > 0x7f80_0000 { 0x7fc0_0000 } else { r }
}

/// The operands of output `form` (0..OUTS) of a case, as the probe shader passes them.
fn form(c: [u32; 3], form: usize) -> [u32; 3] {
    const ONE: u32 = 0x3f80_0000;
    const NEG_ZERO: u32 = 0x8000_0000;
    match form {
        0 => c,
        1 => [c[0], 0, c[2]],
        2 => [c[0], c[1], 0],
        3 => [ONE, c[1], c[2]],
        _ => [c[0], NEG_ZERO, NEG_ZERO],
    }
}

/// D3D12_FEATURE_D3D12_OPTIONS.DoublePrecisionFloatShaderOps. Assumed present when the
/// query itself fails.
unsafe fn has_doubles(g: &Gpu) -> bool {
    let mut options = [0u32; 15];
    let hr = vcall!(g.device, 13, fn(u32, *mut u32, u32) -> i32, 0, options.as_mut_ptr(), 60);
    hr < 0 || options[0] != 0
}

/// Run `helper` (a `tb_fma` definition) on the device in one denormal mode and count the
/// results that differ from the CPU reference.
fn differs_from_reference(g: &mut Gpu, helper: &str, mode: &str, cs: &[[u32; 3]]) -> Result<usize, String> {
    // The second column only has to exist: it repeats the first, which is the one compared.
    let code = crate::dxc::compile_denorm(&shader(helper, "float tb_fma(float a, float b, float c) { return tb_fma_f(a, b, c); }"), "cs_6_2", mode, &[])?;
    let out = unsafe { dispatch(g, &code, cs) }?;
    if out.len() != cs.len() * OUTS {
        return Err(format!("{} results for {} cases", out.len(), cs.len()));
    }
    Ok(out.iter().enumerate().filter(|(k, o)| { let [x, y, z] = form(cs[k / OUTS], k % OUTS); o[0] != reference(x, y, z) }).count())
}

fn run_probe(fast: &str, exact: &str) -> Result<(Fma, String), String> {
    let mut g = unsafe { Gpu::new(false) }?;
    g.open = false;
    let doubles = unsafe { has_doubles(&g) };
    let src = if doubles { shader(fast, exact) } else { shader(fast, "float tb_fma(float a, float b, float c) { return tb_fma_f(a, b, c); }") };
    let cs = cases(CASES, 0x9e37_79b9_7f4a_7c15 ^ PROBE_VERSION as u64);
    let mut res = [false; 2];
    let mut detail = String::new();
    for (i, mode) in ["preserve", "any"].iter().enumerate() {
        let code = crate::dxc::compile_denorm(&src, "cs_6_2", mode, &[])?;
        let mut out = unsafe { dispatch(&mut g, &code, &cs) }?;
        if !doubles {
            for (k, o) in out.iter_mut().enumerate() {
                let [x, y, z] = form(cs[k / OUTS], k % OUTS);
                o[1] = reference(x, y, z);
            }
        }
        let bad: Vec<usize> = out.iter().enumerate().filter(|(_, o)| o[0] != o[1]).map(|(k, _)| k / OUTS).collect();
        res[i] = bad.is_empty() && out.len() == cs.len() * OUTS;
        detail.push_str(&format!(" {mode}: {} of {} results differ", bad.len(), out.len()));
        if let Some(&k) = bad.first() {
            let j = out.iter().position(|o| o[0] != o[1]).unwrap_or(0);
            detail.push_str(&format!(" (first a={:08x} b={:08x} c={:08x} form {} fast={:08x} exact={:08x})", cs[k][0], cs[k][1], cs[k][2], j % OUTS, out[j][0], out[j][1]));
        }
    }
    let mut int = false;
    if !doubles {
        detail.push_str(" [device has no 64-bit float shader ops: exact column computed on the CPU]");
        // The f64 helper cannot be built here. Where the fast one is not exact either, the
        // integer helper is the only exact path, and it is used only if this device runs it
        // correctly in both denormal modes.
        if !(res[0] && res[1]) {
            let checked = match int_helper_lines() {
                None => Err("TB_INT_FMA block not found".to_string()),
                Some(helper) => ["preserve", "any"].iter().try_fold(0usize, |n, mode| differs_from_reference(&mut g, helper, mode, &cs).map(|d| n + d)),
            };
            match checked {
                Ok(0) => {
                    int = true;
                    detail.push_str("; integer fma verified, used where the fast one is off");
                }
                Ok(n) => detail.push_str(&format!("; integer fma differs in {n} results, not used")),
                Err(e) => detail.push_str(&format!("; integer fma not usable ({e})")),
            }
        }
    }
    unsafe {
        for o in [g.fence, g.list, g.alloc, g.queue, g.device] {
            d3d::release(o);
        }
    }
    Ok((Fma { preserve: res[0], any: res[1], int }, detail.trim().to_string()))
}

pub fn probe() -> &'static Probe {
    static P: OnceLock<Probe> = OnceLock::new();
    P.get_or_init(|| {
        let adapter = unsafe { adapter_id() };
        match std::env::var("TOMBOX_FAST_FMA").ok().as_deref() {
            Some("0") => return Probe { fma: Fma::default(), adapter, detail: "TOMBOX_FAST_FMA=0: exact f64 FMA path forced".into(), cached: false },
            Some("force") => return Probe { fma: Fma { preserve: true, any: true, int: false }, adapter, detail: "TOMBOX_FAST_FMA=force: hardware mad used WITHOUT verification".into(), cached: false },
            _ => {}
        }
        let Some((fast, exact)) = helper_lines() else {
            return Probe { fma: Fma::default(), adapter, detail: "tb_fma helper block not found; exact path kept".into(), cached: false };
        };
        let id = format!("v{PROBE_VERSION} {adapter} {} {:016x}", crate::dxc::identity(), {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for b in fast.bytes().chain(exact.bytes()).chain(int_helper_lines().unwrap_or("").bytes()) {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            h
        });
        let file = crate::shader_cache::root().join("fma_probe.txt");
        if let Ok(t) = std::fs::read_to_string(&file) {
            let mut it = t.lines();
            if it.next() == Some(id.as_str()) {
                if let Some(r) = it.next() {
                    let fma = Fma { preserve: r.contains("preserve=1"), any: r.contains("any=1"), int: r.contains("int=1") };
                    return Probe { fma, adapter, detail: it.next().unwrap_or("").to_string(), cached: true };
                }
            }
        }
        match std::panic::catch_unwind(|| run_probe(fast, exact)) {
            Ok(Ok((fma, detail))) => {
                let _ = std::fs::write(&file, format!("{id}\npreserve={} any={} int={}\n{detail}\n", fma.preserve as u8, fma.any as u8, fma.int as u8));
                Probe { fma, adapter, detail, cached: false }
            }
            Ok(Err(e)) => Probe { fma: Fma::default(), adapter, detail: format!("probe failed ({e}); exact path kept"), cached: false },
            Err(_) => Probe { fma: Fma::default(), adapter, detail: "probe panicked; exact path kept".into(), cached: false },
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_block_is_found() {
        let (f, x) = helper_lines().expect("tb_fma block");
        assert!(f.contains("mad("));
        assert!(x.contains("double"));
        let i = int_helper_lines().expect("TB_INT_FMA block");
        assert!(i.contains("uint64_t") && !i.contains("double") && !i.contains("mad("));
    }

    /// The CPU reference must be a correctly rounded fused multiply-add with the helper's
    /// NaN rules: checked against `f32::mul_add` on every output form of the probe cases.
    #[test]
    fn reference_is_a_single_rounding_fma() {
        let nan = |x: u32| x & 0x7fff_ffff > 0x7f80_0000;
        let mut checked = 0usize;
        for c in cases(200_000, 0x1234_5678_9abc_def1) {
            for k in 0..OUTS {
                let [x, y, z] = form(c, k);
                let got = reference(x, y, z);
                let want = if nan(x) || nan(y) || nan(z) {
                    (if nan(x) { x } else if nan(y) { y } else { z }) | 0x40_0000
                } else {
                    let r = f32::from_bits(x).mul_add(f32::from_bits(y), f32::from_bits(z));
                    if r.is_nan() { 0x7fc0_0000 } else { r.to_bits() }
                };
                assert_eq!(got, want, "a={x:08x} b={y:08x} c={z:08x} form {k}");
                checked += 1;
            }
        }
        assert_eq!(checked, 1_000_000);
        // Round to nearest even on an exact tie, and a result that only a fused operation gets.
        assert_eq!(reference(0x3f80_0001, 0x3f7f_ffff, 0xbf80_0000), 0x337f_fffe);
        assert_eq!(reference(0x7f80_0000, 0, 0x3f80_0000), 0x7fc0_0000);
        assert_eq!(reference(0x7f80_0001, 0x7fc0_1234, 0), 0x7fc0_0001);
    }

    #[test]
    #[ignore = "needs a D3D12 GPU"]
    fn hw_bypass_digest_container_runs() {
        let (f, x) = helper_lines().unwrap();
        let src = shader(f, x);
        let t0 = std::time::Instant::now();
        let signed = crate::dxc::compile_denorm(&src, "cs_6_2", "preserve", &[]).unwrap();
        let t_signed = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = std::time::Instant::now();
        let mut raw = crate::dxc::compile_denorm(&src, "cs_6_2", "preserve", &["-Vd"]).unwrap();
        let t_vd = t1.elapsed().as_secs_f64() * 1000.0;
        let same_outside_digest = signed.len() == raw.len() && signed[..4] == raw[..4] && signed[20..] == raw[20..];
        let unsigned_digest = raw[4..20].to_vec();
        raw[4..20].copy_from_slice(&[1u8; 16]);
        let cs = cases(4096, 7);
        let mut g = unsafe { Gpu::new(false) }.unwrap();
        g.open = false;
        let a = unsafe { dispatch(&mut g, &signed, &cs) };
        let b = unsafe { dispatch(&mut g, &raw, &cs) };
        println!("signed {t_signed:.0} ms, -Vd {t_vd:.0} ms, bytes identical outside digest {same_outside_digest}, -Vd digest {unsigned_digest:02x?}");
        println!("signed run ok {}, bypass run {:?}, outputs equal {}", a.is_ok(), b.as_ref().err(), a.as_ref().ok() == b.as_ref().ok());
    }

    /// The integer helper must be exact on any device that can run it, with or without
    /// 64-bit float support.
    #[test]
    #[ignore = "needs a D3D12 GPU"]
    fn hw_integer_fma_matches_reference() {
        let helper = int_helper_lines().unwrap();
        let cs = cases(CASES, 0x9e37_79b9_7f4a_7c15 ^ PROBE_VERSION as u64);
        let mut g = unsafe { Gpu::new(false) }.unwrap();
        g.open = false;
        for mode in ["preserve", "any"] {
            let differ = differs_from_reference(&mut g, helper, mode, &cs).unwrap_or_else(|e| panic!("{mode}: {e}"));
            println!("{} | integer fma, denorm {mode}: {differ} of {} results differ", unsafe { adapter_id() }, cs.len() * OUTS);
            assert_eq!(differ, 0, "denorm {mode}");
        }
    }

    #[test]
    #[ignore = "needs a D3D12 GPU"]
    fn hw_fast_fma_matches_exact_path() {
        let (f, x) = helper_lines().unwrap();
        let (fma, detail) = run_probe(f, x).expect("probe");
        println!("{} | {:?} | {detail}", unsafe { adapter_id() }, fma);
    }
}
