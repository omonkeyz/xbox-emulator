use crate::d3d::{self, Gpu, Guid};
use std::ffi::c_void;
use std::sync::OnceLock;

pub struct Probe {
    pub wave64: bool,
    pub detail: String,
}

const LANES: usize = 64;

pub const SHADER: &str = "RWByteAddressBuffer o : register(u0);\n[RootSignature(\"UAV(u0)\")]\n[WaveSize(64)]\n[numthreads(64, 1, 1)]\nvoid main(uint3 t : SV_GroupThreadID) { o.Store2(t.x * 8u, uint2(WaveGetLaneCount(), WaveGetLaneIndex())); }\n";

unsafe fn lane_range(g: &Gpu) -> Result<(u32, u32), String> {
    let mut o1 = [0u32; 6];
    let hr = vcall!(g.device, 13, fn(u32, *mut u32, u32) -> i32, 8, o1.as_mut_ptr(), 24);
    if hr < 0 {
        return Err(format!("CheckFeatureSupport(OPTIONS1) {:#x}", hr as u32));
    }
    Ok((o1[1], o1[2]))
}

unsafe fn dispatch(g: &mut Gpu, code: &[u8]) -> Result<Vec<[u32; 2]>, String> {
    let n = (LANES * 8) as u64;
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
    let buf = g.buffer(n, 1, d3d::COPY_DEST, true)?;
    let up = g.buffer(n, 2, d3d::GENERIC_READ, false)?;
    let rb = g.buffer(n, 3, d3d::COPY_DEST, false)?;
    Gpu::map_write(up, &vec![0xffu8; n as usize]);
    g.begin();
    let l = g.list;
    vcall!(l, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), buf, 0, up, 0, n);
    g.barrier(buf, d3d::COPY_DEST, 8);
    vcall!(l, 29, fn(*mut c_void) -> (), rs);
    vcall!(l, 25, fn(*mut c_void) -> (), pso);
    vcall!(l, 41, fn(u32, u64) -> (), 0, Gpu::gpu_va(buf));
    vcall!(l, 14, fn(u32, u32, u32) -> (), 1, 1, 1);
    g.barrier(buf, 8, d3d::COPY_SOURCE);
    vcall!(l, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), rb, 0, buf, 0, n);
    let sub = g.submit();
    let out = if sub.is_ok() { Gpu::map_read(rb, n as usize) } else { None };
    for o in [pso, rs, buf, up, rb] {
        d3d::release(o);
    }
    sub?;
    let out = out.ok_or("map readback")?;
    Ok(out.chunks(8).map(|c| [u32::from_le_bytes(c[0..4].try_into().unwrap()), u32::from_le_bytes(c[4..8].try_into().unwrap())]).collect())
}

pub fn verdict(min: u32, max: u32, out: Option<&[[u32; 2]]>) -> (bool, String) {
    let mut detail = format!("WaveLaneCountMin={min} WaveLaneCountMax={max}");
    if !(min..=max).contains(&64) {
        detail.push_str(": the device has no 64-lane waves");
        return (false, detail);
    }
    let Some(out) = out else {
        detail.push_str(": [WaveSize(64)] dispatch did not run");
        return (false, detail);
    };
    let bad = out.iter().enumerate().filter(|(i, r)| r[0] != 64 || r[1] != *i as u32).count();
    let ok = out.len() == LANES && bad == 0;
    detail.push_str(&format!(": [WaveSize(64)] group of {LANES}: {} of {} threads report 64 lanes and their own lane index", out.len() - bad, out.len()));
    (ok, detail)
}

fn run() -> Result<(bool, String), String> {
    if !crate::dxc::available() {
        return Ok((false, "DXC not available".into()));
    }
    let code = crate::dxc::compile(SHADER, "cs_6_6")?;
    let mut g = unsafe { Gpu::new(false) }?;
    g.open = false;
    let res = unsafe {
        lane_range(&g).map(|(min, max)| {
            if (min..=max).contains(&64) {
                match dispatch(&mut g, &code) {
                    Ok(out) => verdict(min, max, Some(&out)),
                    Err(e) => {
                        let (_, d) = verdict(min, max, None);
                        (false, format!("{d} ({e})"))
                    }
                }
            } else {
                verdict(min, max, None)
            }
        })
    };
    unsafe {
        for o in [g.fence, g.list, g.alloc, g.queue, g.device] {
            d3d::release(o);
        }
    }
    res
}

pub fn probe() -> &'static Probe {
    static P: OnceLock<Probe> = OnceLock::new();
    P.get_or_init(|| {
        if !crate::pack_key::wave64_path_from_env() {
            return Probe { wave64: false, detail: "TOMBOX_WAVE64_HOST=0: wave64 programs run with the driver's default wave size and the previous LDS fence and lane rules".into() };
        }
        match std::panic::catch_unwind(run) {
            Ok(Ok((wave64, detail))) => Probe { wave64, detail },
            Ok(Err(e)) => Probe { wave64: false, detail: format!("probe failed ({e})") },
            Err(_) => Probe { wave64: false, detail: "probe panicked".into() },
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_needs_every_lane_in_one_wave() {
        let good: Vec<[u32; 2]> = (0..64).map(|i| [64, i]).collect();
        assert!(verdict(32, 64, Some(&good)).0);
        let split: Vec<[u32; 2]> = (0..64).map(|i| [32, i & 31]).collect();
        assert!(!verdict(32, 64, Some(&split)).0);
        assert!(!verdict(32, 64, Some(&good[..32])).0);
        assert!(!verdict(32, 64, None).0);
        assert!(!verdict(32, 32, Some(&good)).0);
        assert!(!verdict(4, 4, None).0);
    }

    #[test]
    fn probe_shader_compiles_for_shader_model_6_6() {
        if !crate::dxc::available() {
            return;
        }
        crate::dxc::compile(SHADER, "cs_6_6").expect("probe shader");
    }

    #[test]
    fn hw_wave64_probe() {
        if std::env::var_os("TOMBOX_EXEC_HW").is_none() {
            return;
        }
        let p = probe();
        println!("host wave64 {}: {}", p.wave64, p.detail);
    }
}
