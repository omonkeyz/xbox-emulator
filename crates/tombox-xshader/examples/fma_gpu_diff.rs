#[cfg(windows)]
#[path = "../../tools/xreplay/src/d3d.rs"]
#[allow(dead_code)]
mod d3d;

#[cfg(windows)]
use core::ffi::c_void;
#[cfg(windows)]
use d3d::Gpu;
#[cfg(windows)]
use tombox_xshader::dxc;

#[cfg(windows)]
const GROUP: u32 = 256;

#[cfg(windows)]
fn xs(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

#[cfg(windows)]
fn f(b: u32) -> f32 {
    f32::from_bits(b)
}

#[cfg(windows)]
fn gen(n: usize, seed: u64, nan: bool) -> Vec<[u32; 3]> {
    let mut s = seed;
    let mut v = Vec::with_capacity(n);
    let sign = |r: u64| ((r & 1) as u32) << 31;
    while v.len() < n {
        let r = xs(&mut s);
        let r2 = xs(&mut s);
        let k = v.len() % 10;
        let t = match k {
            0 => [r as u32, (r >> 32) as u32, r2 as u32],
            1 => {
                let a = f(r as u32);
                let b = f((r >> 32) as u32);
                let p = (a as f64 * b as f64) as f32;
                [a.to_bits(), b.to_bits(), (p.to_bits() ^ 0x8000_0000) ^ (r2 as u32 & 0xfff)]
            }
            2 | 3 => {
                let i = (r2 as u32 & 0x7fff) + 1;
                let ea = ((r >> 8) as u32 % 200) + 27;
                let eb = ((r >> 20) as u32 % 200) + 27;
                let ea = ea.min(254);
                let eb = eb.min(254);
                let pe = ea as i32 + eb as i32 - 127 - 127;
                let a = sign(r) | (ea << 23) | i;
                let b = sign(r >> 1) | (eb << 23) | (0x80_0000 - i);
                let ce = pe + 127 + 24;
                if !(1..=254).contains(&ce) {
                    [a, b, r2 as u32]
                } else {
                    let cm = ((r2 >> 16) as u32) & 0x7f_ffff;
                    [a, b, sign(r >> 2) | ((ce as u32) << 23) | cm]
                }
            }
            4 => {
                let a = f(r as u32 & 0x80ff_ffff | 0x3f00_0000);
                let b = f((r >> 32) as u32 & 0x80ff_ffff | 0x3f00_0000);
                let p = a as f64 * b as f64;
                let base = p as f32;
                let ulp = f32::from_bits(base.to_bits() + 1) as f64 - base as f64;
                let mid = base as f64 + ulp * 0.5;
                let c = (mid - p) as f32;
                [a.to_bits(), b.to_bits(), c.to_bits() ^ (r2 as u32 & 3)]
            }
            5 => {
                let e = (r2 as u32 % 40) + 40;
                let a = sign(r) | (e << 23) | (r as u32 >> 9 & 0x7f_ffff);
                let b = sign(r >> 1) | (e << 23) | ((r >> 32) as u32 & 0x7f_ffff);
                [a, b, (r2 >> 32) as u32 & 0x80ff_ffff]
            }
            6 => {
                let e = 230 + (r2 as u32 % 25);
                let a = sign(r) | (e << 23) | (r as u32 >> 9 & 0x7f_ffff);
                let b = sign(r >> 1) | ((127 + (r2 >> 8) as u32 % 30) << 23) | ((r >> 32) as u32 & 0x7f_ffff);
                let p = f(a) as f64 * f(b) as f64;
                let c = if p.abs() > f32::MAX as f64 { (-(p.signum()) * f32::MAX as f64) as f32 } else { -(p as f32) };
                [a, b, c.to_bits() ^ ((r2 >> 40) as u32 & 0xff)]
            }
            7 => [(r as u32) & 0x807f_ffff, (r >> 32) as u32, (r2 as u32) & 0x80ff_ffff],
            8 => {
                let sp = [0u32, 0x8000_0000, 0x7f80_0000, 0xff80_0000, 1, 0x8000_0001, 0x7f7f_ffff, 0xff7f_ffff, 0x0080_0000, 0x3f80_0000, 0xbf80_0000];
                let pick = |x: u64| if x % 3 == 0 { sp[(x / 3) as usize % sp.len()] } else { x as u32 };
                [pick(r & 0xffff_ffff), pick(r >> 32), pick(r2)]
            }
            _ => {
                let a = f(r as u32);
                let b = f((r >> 32) as u32);
                let p = (a as f64 * b as f64) as f32;
                [a.to_bits(), b.to_bits(), (p.to_bits() ^ 0x8000_0000).wrapping_add((r2 as u32 % 7).wrapping_sub(3))]
            }
        };
        let is_nan = |b: u32| b & 0x7fff_ffff > 0x7f80_0000;
        if !nan && t.iter().any(|&b| is_nan(b)) {
            continue;
        }
        v.push(t);
    }
    v
}

#[cfg(windows)]
fn shader(a: &str, b: &str) -> String {
    let a = a.replacen("float tb_fma(", "float tb_fma_a(", 1);
    let b = b.replacen("float tb_fma(", "float tb_fma_b(", 1);
    format!(
        "RWByteAddressBuffer io : register(u0);\ncbuffer K : register(b0) {{ uint n; uint obase; }}\n{a}\n{b}\n[RootSignature(\"RootConstants(num32BitConstants=2, b0), UAV(u0)\")]\n[numthreads({GROUP}, 1, 1)]\nvoid main(uint3 g : SV_DispatchThreadID) {{ uint i = g.x; if (i >= n) return; float x = asfloat(io.Load(i * 12)); float y = asfloat(io.Load(i * 12 + 4)); float z = asfloat(io.Load(i * 12 + 8)); io.Store2(obase + i * 8, uint2(asuint(tb_fma_a(x, y, z)), asuint(tb_fma_b(x, y, z)))); }}\n"
    )
}

#[cfg(windows)]
unsafe fn run(g: &mut Gpu, code: &[u8], cases: &[[u32; 3]]) -> Result<Vec<[u32; 2]>, String> {
    let n = cases.len();
    let obase = (n * 12) as u64;
    let total = obase + (n * 8) as u64;
    let mut rs: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(g.device, 16, fn(u32, *const u8, usize, *const d3d::Guid, *mut *mut c_void) -> i32, 0, code.as_ptr(), code.len(), &d3d::IID_ROOTSIG, &mut rs);
    if hr < 0 {
        return Err(format!("CreateRootSignature {hr:#x}"));
    }
    let mut d = [0u8; 56];
    d[0..8].copy_from_slice(&(rs as u64).to_le_bytes());
    d[8..16].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
    d[16..24].copy_from_slice(&(code.len() as u64).to_le_bytes());
    let mut pso: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(g.device, 11, fn(*const u8, *const d3d::Guid, *mut *mut c_void) -> i32, d.as_ptr(), &d3d::IID_PSO, &mut pso);
    if hr < 0 {
        return Err(format!("CreateComputePipelineState {hr:#x}"));
    }
    let buf = g.buffer(total, 1, d3d::COPY_DEST, true)?;
    let up = g.buffer(total, 2, d3d::GENERIC_READ, false)?;
    let rb = g.buffer(total, 3, d3d::COPY_DEST, false)?;
    let mut bytes: Vec<u8> = cases.iter().flat_map(|t| t.iter().flat_map(|w| w.to_le_bytes())).collect();
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
    g.submit()?;
    let out = Gpu::map_read(rb, total as usize).ok_or("map")?;
    for k in [pso, rs, buf, up, rb] {
        d3d::release(k);
    }
    Ok(out[obase as usize..].chunks(8).map(|c| [u32::from_le_bytes(c[0..4].try_into().unwrap()), u32::from_le_bytes(c[4..8].try_into().unwrap())]).collect())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("fma_gpu_diff needs Windows");
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: fma_gpu_diff <old_tb_fma.hlsl> <new_tb_fma.hlsl> [million cases] [batches]");
        std::process::exit(2);
    }
    let a = std::fs::read_to_string(&args[0]).expect("old");
    let b = std::fs::read_to_string(&args[1]).expect("new");
    let per: usize = args.get(2).and_then(|s| s.parse::<f64>().ok()).map(|m| (m * 1e6) as usize).unwrap_or(1 << 20);
    let batches: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);
    let src = shader(&a, &b);
    let mut g = unsafe { Gpu::new(false) }.expect("device");
    g.open = false;
    let mut fails = 0usize;
    let mut total = 0usize;
    let mut wrong_old = 0usize;
    for profile in ["cs_6_2", "cs_6_0"] {
        let code = dxc::compile(&src, profile).expect("compile");
        for bi in 0..batches {
            let nan = bi % 2 == 1;
            let cases = gen(per, 0x9e37_79b9_7f4a_7c15 ^ (bi as u64).wrapping_mul(0x1234_5678_9abc_def1) ^ profile.len() as u64, nan);
            let out = unsafe { run(&mut g, &code, &cases) }.expect("gpu run");
            for (c, o) in cases.iter().zip(out.iter()) {
                total += 1;
                let want = {
                    let r = f(c[0]).mul_add(f(c[1]), f(c[2]));
                    if r.is_nan() { None } else { Some(r.to_bits()) }
                };
                if let Some(w) = want {
                    if o[0] != w {
                        wrong_old += 1;
                    }
                }
                let ok = o[0] == o[1] && want.map_or(true, |w| o[1] == w);
                if !ok {
                    fails += 1;
                    if fails <= 30 {
                        println!("{profile} MISMATCH a={:08x} b={:08x} c={:08x} old={:08x} new={:08x} ref={:?}", c[0], c[1], c[2], o[0], o[1], want.map(|w| format!("{w:08x}")));
                    }
                }
            }
            println!("{profile} batch {bi} ({} cases, nan inputs {nan}): cumulative mismatches {fails}, old-vs-ref errors {wrong_old}", cases.len());
        }
    }
    println!("TOTAL {total} cases: {fails} mismatches (new vs old bits, and new vs CPU fma for non-NaN); old-vs-CPU errors {wrong_old}");
    std::process::exit(if fails == 0 { 0 } else { 1 });
}
