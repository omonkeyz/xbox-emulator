use super::*;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const IID_DEV: Guid = Guid { d1: 0x1898_19f1, d2: 0x1db6, d3: 0x4b57, d4: [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7] };
const IID_RES: Guid = Guid { d1: 0x6964_42be, d2: 0xa72e, d3: 0x4059, d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad] };
const IID_QUEUE: Guid = Guid { d1: 0x0ec8_70a6, d2: 0x5d7e, d3: 0x4c22, d4: [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed] };
const IID_ALLOCATOR: Guid = Guid { d1: 0x6102_dee4, d2: 0xaf59, d3: 0x4b09, d4: [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24] };
const IID_LIST: Guid = Guid { d1: 0x5b16_0d0f, d2: 0xac1b, d3: 0x4185, d4: [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55] };
const IID_FENCE: Guid = Guid { d1: 0x0a75_3dcf, d2: 0xc4d8, d3: 0x4b91, d4: [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76] };
const IID_ROOT_SIGNATURE: Guid = Guid { d1: 0xc54a_6b66, d2: 0x72df, d3: 0x4ee8, d4: [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14] };
const IID_PSO: Guid = Guid { d1: 0x765a_30f3, d2: 0xf624, d3: 0x4c6f, d4: [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45] };
const IID_DESC_HEAP: Guid = Guid { d1: 0x8efb_471d, d2: 0x616c, d3: 0x4f49, d4: [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51] };
const IID_FACTORY4: Guid = Guid { d1: 0x1bc6_ea02, d2: 0xef36, d3: 0x464f, d4: [0xbf, 0x0c, 0x21, 0xca, 0x39, 0xe5, 0x16, 0x8a] };
const IID_ADAPTER: Guid = Guid { d1: 0x2411_e7e1, d2: 0x12ac, d3: 0x4ccf, d4: [0xbd, 0x14, 0x97, 0x98, 0xe8, 0x53, 0x4d, 0xc0] };
const IID_DEBUG: Guid = Guid { d1: 0x344488b7, d2: 0x6846, d3: 0x474b, d4: [0xb9, 0x89, 0xf0, 0x27, 0x44, 0x82, 0x45, 0xe0] };
const IID_INFO_QUEUE: Guid = Guid { d1: 0x0742_a90b, d2: 0xc387, d3: 0x483f, d4: [0xb9, 0x46, 0x30, 0xa7, 0xe4, 0xe6, 0x14, 0x58] };

const HEAP_DEFAULT: u32 = 1;
const HEAP_UPLOAD: u32 = 2;
const HEAP_READBACK: u32 = 3;
const ST_RT: u32 = 0x4;
const ST_UAV: u32 = 0x8;
const ST_NPSR: u32 = 0x40;
const ST_GENERIC_READ: u32 = 0xac3;
const FMT_RGBA32_UINT: u32 = 3;
const LIST_DIRECT: u32 = 0;
const LIST_COMPUTE: u32 = 2;
const N: usize = 20;
const CB_BYTES: usize = N * 16;
const PITCH: u64 = 512;

const P_CLOSE: usize = 9;
const P_RESET: usize = 10;
const P_DRAW: usize = 12;
const P_DISPATCH: usize = 14;
const P_COPY_TEXTURE: usize = 16;
const P_TOPOLOGY: usize = 20;
const P_VIEWPORTS: usize = 21;
const P_SCISSORS: usize = 22;
const P_PSO: usize = 25;
const P_BARRIER: usize = 26;
const P_COMPUTE_RS: usize = 29;
const P_GRAPHICS_RS: usize = 30;
const P_COMPUTE_SRV: usize = 39;
const P_GRAPHICS_SRV: usize = 40;
const P_COMPUTE_UAV: usize = 41;
const P_RTS: usize = 46;

macro_rules! call {
    ($o:expr, $i:expr, ($($t:ty),*) -> $r:ty $(, $a:expr)*) => {{
        let o = $o as *mut c_void;
        let f: unsafe extern "system" fn(*mut c_void $(, $t)*) -> $r = core::mem::transmute(vslot(o, $i));
        f(o $(, $a)*)
    }};
}

extern "system" {
    fn LoadLibraryW(p: *const u16) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
}

fn warp() -> bool {
    std::env::var("TOMBOX_HW_ADAPTER").map(|v| v.eq_ignore_ascii_case("warp")).unwrap_or(false)
}

fn locked() -> Option<String> {
    for d in ["D:/TomBox/tools/dev", "D:/TomBox/tools/dev/batch_runs"] {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_ascii_uppercase();
            if n.starts_with("GAME_LOCK") || n.starts_with("USER_PLAYING") {
                return Some(format!("{d}/{}", e.file_name().to_string_lossy()));
            }
        }
    }
    None
}

fn skip() -> bool {
    if warp() {
        return false;
    }
    match locked() {
        Some(p) => {
            eprintln!("SKIPPED: {p} exists, so the hardware GPU is in use; rerun with TOMBOX_HW_ADAPTER=warp or once the lock is gone");
            true
        }
        None => false,
    }
}

struct Gpu {
    dev: usize,
    gq: usize,
    cq: usize,
    info: usize,
}

unsafe impl Send for Gpu {}
unsafe impl Sync for Gpu {}

unsafe fn warp_adapter() -> *mut c_void {
    let name: Vec<u16> = "dxgi.dll\0".encode_utf16().collect();
    let h = unsafe { LoadLibraryW(name.as_ptr()) };
    assert!(!h.is_null());
    let p = unsafe { GetProcAddress(h, b"CreateDXGIFactory2\0".as_ptr()) };
    assert!(!p.is_null());
    let create: unsafe extern "system" fn(u32, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(p) };
    let mut f: *mut c_void = core::ptr::null_mut();
    assert!(unsafe { create(0, &IID_FACTORY4, &mut f) } >= 0);
    let mut a: *mut c_void = core::ptr::null_mut();
    assert!(unsafe { call!(f, 27, (*const Guid, *mut *mut c_void) -> i32, &IID_ADAPTER, &mut a) } >= 0);
    a
}

unsafe fn make_queue(dev: usize, ty: u32) -> usize {
    let desc = [ty, 0, 0, 0];
    let mut q: *mut c_void = core::ptr::null_mut();
    assert!(unsafe { call!(dev, 8, (*const u32, *const Guid, *mut *mut c_void) -> i32, desc.as_ptr(), &IID_QUEUE, &mut q) } >= 0);
    q as usize
}

fn gpu() -> &'static Gpu {
    static G: OnceLock<Gpu> = OnceLock::new();
    G.get_or_init(|| unsafe {
        crate::enable_experimental_shader_models();
        let debug = std::env::var("TOMBOX_D3D12_DEBUG").is_ok();
        if debug {
            let p = crate::host_proc(b"D3D12GetDebugInterface\0");
            let mut dbg: *mut c_void = core::ptr::null_mut();
            let hr = if p == 0 { -1 } else {
                let get: unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(p);
                get(&IID_DEBUG, &mut dbg)
            };
            if hr >= 0 && !dbg.is_null() {
                call!(dbg, 3, () -> ());
                eprintln!("D3D12 debug layer enabled");
            } else {
                eprintln!("D3D12 debug layer requested but unavailable (hr={hr:#010x}); running without it");
            }
        }
        let adapter = if warp() { warp_adapter() } else { core::ptr::null_mut() };
        let p = crate::host_proc(b"D3D12CreateDevice\0");
        let create: extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(p);
        let mut dev: *mut c_void = core::ptr::null_mut();
        let hr = create(adapter, 0xb000, &IID_DEV, &mut dev);
        assert!(hr >= 0 && !dev.is_null(), "D3D12CreateDevice ({}) hr={hr:#010x}", if warp() { "WARP" } else { "hardware" });
        let mut info: *mut c_void = core::ptr::null_mut();
        if debug && call!(dev, 0, (*const Guid, *mut *mut c_void) -> i32, &IID_INFO_QUEUE, &mut info) < 0 {
            info = core::ptr::null_mut();
        }
        let dev = dev as usize;
        Gpu { dev, gq: make_queue(dev, LIST_DIRECT), cq: make_queue(dev, LIST_COMPUTE), info: info as usize }
    })
}

unsafe fn debug_errors(g: &Gpu) -> Vec<String> {
    let mut out = Vec::new();
    if g.info == 0 {
        return out;
    }
    let n = unsafe { call!(g.info, 8, () -> u64) };
    for i in 0..n {
        let mut len = 0usize;
        unsafe { call!(g.info, 5, (u64, *mut u8, *mut usize) -> i32, i, core::ptr::null_mut(), &mut len) };
        if len < 32 {
            continue;
        }
        let mut m = vec![0u64; len.div_ceil(8)];
        if unsafe { call!(g.info, 5, (u64, *mut u8, *mut usize) -> i32, i, m.as_mut_ptr() as *mut u8, &mut len) } < 0 {
            continue;
        }
        let b = m.as_ptr() as *const u8;
        let sev = unsafe { *(b.add(4) as *const u32) };
        let text = unsafe { *(b.add(16) as *const *const i8) };
        if sev <= 1 && !text.is_null() {
            out.push(unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy().into_owned());
        }
    }
    unsafe { call!(g.info, 4, () -> ()) };
    out
}

struct Buf {
    res: usize,
    va: u64,
    cpu: *mut u8,
}

unsafe fn committed(dev: usize, heap: u32, dim: u32, w: u64, fmt: u32, flags: u32, state: u32) -> usize {
    let props: [u32; 5] = [heap, 0, 0, 1, 1];
    let mut d = [0u8; 56];
    d[0..4].copy_from_slice(&dim.to_le_bytes());
    d[0x10..0x18].copy_from_slice(&w.to_le_bytes());
    d[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
    d[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
    d[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
    d[0x20..0x24].copy_from_slice(&fmt.to_le_bytes());
    d[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
    d[0x2c..0x30].copy_from_slice(&(if dim == 1 { 1u32 } else { 0 }).to_le_bytes());
    d[0x30..0x34].copy_from_slice(&flags.to_le_bytes());
    let mut r: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { call!(dev, 27, (*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
        props.as_ptr(), 0, d.as_ptr(), state, core::ptr::null(), &IID_RES, &mut r) };
    assert!(hr >= 0 && !r.is_null(), "CreateCommittedResource dim={dim} heap={heap} hr={hr:#010x}");
    r as usize
}

unsafe fn buffer(dev: usize, heap: u32, size: u64, flags: u32, state: u32, tracked: Option<u64>) -> Buf {
    let r = unsafe { committed(dev, heap, 1, size, 0, flags, state) };
    let va = unsafe { call!(r, 11, () -> u64) };
    if let Some(len) = tracked {
        crate::gpu_va::insert(crate::gpu_va::Entry { start: va, len, res: r, buffer: true });
        crate::gpu_va::set_state(r, state);
    }
    let mut cpu: *mut u8 = core::ptr::null_mut();
    if heap != HEAP_DEFAULT {
        assert!(unsafe { call!(r, 8, (u32, *const c_void, *mut *mut u8) -> i32, 0, core::ptr::null(), &mut cpu) } >= 0);
    }
    Buf { res: r, va, cpu }
}

fn pattern(seed: u32, n: usize) -> Vec<u8> {
    (0..n / 4).flat_map(|i| ((i as u32).wrapping_mul(0x9e37_79b9) ^ seed ^ 0x0101_0101).to_le_bytes()).collect()
}

unsafe fn put(b: &Buf, off: usize, data: &[u8]) {
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), b.cpu.add(off), data.len()) };
}

unsafe fn get(b: &Buf, off: usize, n: usize) -> Vec<u8> {
    (0..n).map(|i| unsafe { core::ptr::read_volatile(b.cpu.add(off + i)) }).collect()
}

fn transition_bar(res: usize, before: u32, after: u32) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[8..16].copy_from_slice(&(res as u64).to_le_bytes());
    b[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    b[20..24].copy_from_slice(&before.to_le_bytes());
    b[24..28].copy_from_slice(&after.to_le_bytes());
    b
}

unsafe fn barrier(l: usize, res: usize, before: u32, after: u32) {
    let b = transition_bar(res, before, after);
    unsafe { call!(l, P_BARRIER, (u32, *const [u8; 32]) -> (), 1, &b) };
}

struct Run {
    alloc: usize,
    list: usize,
    queue: usize,
    fence: usize,
    value: u64,
}

unsafe fn run_new(g: &Gpu, ty: u32) -> Run {
    let mut a: *mut c_void = core::ptr::null_mut();
    assert!(unsafe { call!(g.dev, 9, (u32, *const Guid, *mut *mut c_void) -> i32, ty, &IID_ALLOCATOR, &mut a) } >= 0);
    let mut l: *mut c_void = core::ptr::null_mut();
    assert!(unsafe { call!(g.dev, 12, (u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32, 0, ty, a, core::ptr::null_mut(), &IID_LIST, &mut l) } >= 0);
    let mut f: *mut c_void = core::ptr::null_mut();
    assert!(unsafe { call!(g.dev, 36, (u64, u32, *const Guid, *mut *mut c_void) -> i32, 0, 0, &IID_FENCE, &mut f) } >= 0);
    Run { alloc: a as usize, list: l as usize, queue: if ty == LIST_COMPUTE { g.cq } else { g.gq }, fence: f as usize, value: 0 }
}

unsafe fn execute(g: &Gpu, r: &mut Run) {
    assert!(unsafe { call!(r.list, P_CLOSE, () -> i32) } >= 0, "Close failed");
    let l = [r.list as *mut c_void];
    unsafe { call!(r.queue, 10, (u32, *const *mut c_void) -> (), 1, l.as_ptr()) };
    unsafe { stamp_executed(r.queue as *mut c_void, &l) };
    r.value += 1;
    assert!(unsafe { call!(r.queue, 14, (*mut c_void, u64) -> i32, r.fence as *mut c_void, r.value) } >= 0);
    let t = Instant::now();
    while unsafe { call!(r.fence, 8, () -> u64) } < r.value {
        assert!(t.elapsed() < Duration::from_secs(60), "GPU never finished (device removed reason {:#010x})", unsafe { call!(g.dev, 37, () -> i32) });
        std::thread::sleep(Duration::from_millis(1));
    }
    let errs = unsafe { debug_errors(g) };
    assert!(errs.is_empty(), "D3D12 debug layer errors:\n{}", errs.join("\n"));
}

unsafe fn restart(r: &mut Run) {
    with(|e| e.forget(r.list));
    assert!(unsafe { call!(r.alloc, 8, () -> i32) } >= 0);
    assert!(unsafe { call!(r.list, P_RESET, (*mut c_void, *mut c_void) -> i32, r.alloc as *mut c_void, core::ptr::null_mut()) } >= 0);
}

fn compile(src: &str, profile: &str) -> Vec<u8> {
    tombox_xshader::dxc::compile(src, profile).unwrap_or_else(|e| panic!("dxc {profile}: {e}\n{src}"))
}

unsafe fn root_sig(dev: usize, blob: &[u8]) -> usize {
    let mut r: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { call!(dev, 16, (u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32, 0, blob.as_ptr(), blob.len(), &IID_ROOT_SIGNATURE, &mut r) };
    assert!(hr >= 0 && !r.is_null(), "CreateRootSignature hr={hr:#010x}");
    r as usize
}

struct Sigs {
    copy: usize,
    raw: usize,
}

unsafe fn sigs(dev: usize, rs: &str, stage: &str) -> Sigs {
    let probe = if stage == "cs_6_0" {
        format!("#define RS \"{rs}\"\n[RootSignature(RS)] [numthreads(1, 1, 1)] void main() {{ }}")
    } else {
        format!("#define RS \"{rs}\"\n[RootSignature(RS)] float4 main() : SV_Position {{ return 0; }}")
    };
    let blob = compile(&probe, stage);
    let params = crate::cbv_raw::root_cbv_params(&blob);
    assert!(!params.is_empty(), "the copy root signature has no root CBV");
    let (raw_blob, patch) = crate::cbv_raw::patch_blob(&blob).expect("patch_blob").expect("raw root signature");
    assert_eq!(patch.root_cbvs, vec![0]);
    let copy = unsafe { root_sig(dev, &blob) };
    let raw = unsafe { root_sig(dev, &raw_blob) };
    with(|e| e.rs_params.insert(copy, params));
    Sigs { copy, raw }
}

fn cb_decl() -> String {
    format!("cbuffer CB0 : register(b0) {{ uint4 cb0[{N}]; }};\n")
}

fn sizes_for(vis: u32, hlsl: &str) -> Vec<(u32, u32, u32, u64)> {
    let v: Vec<(u32, u32, u32, u64)> = tombox_xshader::cbv_raw_hlsl::declared_sizes(hlsl).into_iter().map(|(r, s, b)| (vis, r, s, b)).collect();
    assert_eq!(v, vec![(vis, 0, 0, CB_BYTES as u64)]);
    v
}

const VS: &str = "float4 main(uint id : SV_VertexID) : SV_Position { float2 t = float2((id << 1) & 2, id & 2); return float4(t * float2(2, -2) + float2(-1, 1), 0, 1); }";

fn ps_src() -> String {
    format!("{}uint4 main(float4 p : SV_Position) : SV_Target {{ return cb0[(uint)p.x]; }}\n", cb_decl())
}

fn cs_src() -> String {
    format!("{}RWByteAddressBuffer o : register(u0);\n[numthreads({N}, 1, 1)] void main(uint i : SV_GroupThreadID) {{ o.Store4(i * 16, cb0[i]); }}\n", cb_decl())
}

struct Gfx {
    sigs: Sigs,
    copy_pso: usize,
    raw_pso: usize,
    rt: usize,
    rtv: usize,
    rb: Buf,
    _keep: Vec<Vec<u8>>,
}

unsafe fn gfx_pso(dev: usize, rs: usize, vs: &[u8], ps: &[u8]) -> usize {
    let mut d = vec![0u8; 0x290];
    let mut w = |o: usize, v: &[u8]| d[o..o + v.len()].copy_from_slice(v);
    w(0, &(rs as u64).to_le_bytes());
    w(0x08, &(vs.as_ptr() as u64).to_le_bytes());
    w(0x10, &(vs.len() as u64).to_le_bytes());
    w(0x18, &(ps.as_ptr() as u64).to_le_bytes());
    w(0x20, &(ps.len() as u64).to_le_bytes());
    w(0x78, &crate::dbcount::default_blend());
    w(0x1c0, &u32::MAX.to_le_bytes());
    w(0x1c4, &3u32.to_le_bytes());
    w(0x1c8, &1u32.to_le_bytes());
    w(0x1dc, &1u32.to_le_bytes());
    w(0x23c, &3u32.to_le_bytes());
    w(0x240, &1u32.to_le_bytes());
    w(0x244, &FMT_RGBA32_UINT.to_le_bytes());
    w(0x268, &1u32.to_le_bytes());
    let mut p: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { call!(dev, 10, (*const u8, *const Guid, *mut *mut c_void) -> i32, d.as_ptr(), &IID_PSO, &mut p) };
    assert!(hr >= 0 && !p.is_null(), "CreateGraphicsPipelineState hr={hr:#010x}");
    p as usize
}

unsafe fn compute_pso(dev: usize, rs: usize, cs: &[u8]) -> usize {
    let mut d = [0u8; 56];
    d[0..8].copy_from_slice(&(rs as u64).to_le_bytes());
    d[8..16].copy_from_slice(&(cs.as_ptr() as u64).to_le_bytes());
    d[16..24].copy_from_slice(&(cs.len() as u64).to_le_bytes());
    let mut p: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { call!(dev, 11, (*const u8, *const Guid, *mut *mut c_void) -> i32, d.as_ptr(), &IID_PSO, &mut p) };
    assert!(hr >= 0 && !p.is_null(), "CreateComputePipelineState hr={hr:#010x}");
    p as usize
}

unsafe fn gfx(g: &Gpu) -> Gfx {
    let sigs = unsafe { sigs(g.dev, "RootFlags(0), CBV(b0, visibility=SHADER_VISIBILITY_PIXEL)", "vs_6_0") };
    let ps_text = ps_src();
    let raw_text = tombox_xshader::cbv_raw_hlsl::rewrite_hlsl(&ps_text).expect("rewrite_hlsl");
    assert!(raw_text.contains("Load4"), "raw PS was not rewritten:\n{raw_text}");
    let vs = compile(VS, "vs_6_0");
    let ps = compile(&ps_text, "ps_6_0");
    let raw_ps = compile(&raw_text, "ps_6_0");
    let copy_pso = unsafe { gfx_pso(g.dev, sigs.copy, &vs, &ps) };
    let raw_pso = unsafe { gfx_pso(g.dev, sigs.raw, &vs, &raw_ps) };
    with(|e| e.pso_sizes.insert(copy_pso, sizes_for(5, &ps_text)));
    let rt = unsafe { committed(g.dev, HEAP_DEFAULT, 3, N as u64, FMT_RGBA32_UINT, 1, ST_RT) };
    let hd = [2u32, 1, 0, 0];
    let mut h: *mut c_void = core::ptr::null_mut();
    assert!(unsafe { call!(g.dev, 14, (*const u32, *const Guid, *mut *mut c_void) -> i32, hd.as_ptr(), &IID_DESC_HEAP, &mut h) } >= 0);
    let mut rtv = 0usize;
    let ret = unsafe { call!(h, 9, (*mut usize) -> usize, &mut rtv) };
    if ret != &mut rtv as *mut usize as usize {
        rtv = ret;
    }
    unsafe { call!(g.dev, 20, (*mut c_void, *const c_void, usize) -> (), rt as *mut c_void, core::ptr::null(), rtv) };
    let rb = unsafe { buffer(g.dev, HEAP_READBACK, PITCH, 0, ST_COPY_DEST, None) };
    Gfx { sigs, copy_pso, raw_pso, rt, rtv, rb, _keep: vec![vs, ps, raw_ps] }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Via {
    Raw,
    Copy,
}

unsafe fn record_draw(f: &Gfx, l: usize, via: Via, va: u64) {
    unsafe {
        call!(l, P_RTS, (u32, *const usize, i32, *const usize) -> (), 1, &f.rtv, 0, core::ptr::null());
        let vp = [0f32, 0.0, N as f32, 1.0, 0.0, 1.0];
        call!(l, P_VIEWPORTS, (u32, *const f32) -> (), 1, vp.as_ptr());
        let sc = [0i32, 0, N as i32, 1];
        call!(l, P_SCISSORS, (u32, *const i32) -> (), 1, sc.as_ptr());
        call!(l, P_TOPOLOGY, (u32) -> (), 4);
        match via {
            Via::Raw => {
                call!(l, P_GRAPHICS_RS, (usize) -> (), f.sigs.raw);
                call!(l, P_PSO, (usize) -> (), f.raw_pso);
                call!(l, P_GRAPHICS_SRV, (u32, u64) -> (), 0, va);
            }
            Via::Copy => {
                let lp = l as *mut c_void;
                call!(l, P_GRAPHICS_RS, (usize) -> (), f.sigs.copy);
                with(|e| e.on_root_signature(l, SIDE_GRAPHICS, f.sigs.copy));
                call!(l, P_PSO, (usize) -> (), f.copy_pso);
                set_root_cbv(lp, SIDE_GRAPHICS, 0, va, &|v| bind(lp, SIDE_GRAPHICS, 0, v));
                resolve_with(lp, MASK_GRAPHICS, Some(f.copy_pso), None);
                assert!(!with(|e| e.has_pending(l, MASK_GRAPHICS)), "the draw left a root CBV pending");
            }
        }
        call!(l, P_DRAW, (u32, u32, u32, u32) -> (), 3, 1, 0, 0);
        barrier(l, f.rt, ST_RT, ST_COPY_SOURCE);
        let mut dst = [0u8; 48];
        dst[0..8].copy_from_slice(&(f.rb.res as u64).to_le_bytes());
        dst[8..12].copy_from_slice(&1u32.to_le_bytes());
        dst[24..28].copy_from_slice(&FMT_RGBA32_UINT.to_le_bytes());
        dst[28..32].copy_from_slice(&(N as u32).to_le_bytes());
        dst[32..36].copy_from_slice(&1u32.to_le_bytes());
        dst[36..40].copy_from_slice(&1u32.to_le_bytes());
        dst[40..44].copy_from_slice(&(PITCH as u32).to_le_bytes());
        let mut src = [0u8; 48];
        src[0..8].copy_from_slice(&(f.rt as u64).to_le_bytes());
        call!(l, P_COPY_TEXTURE, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), dst.as_ptr(), 0, 0, 0, src.as_ptr(), core::ptr::null());
        barrier(l, f.rt, ST_COPY_SOURCE, ST_RT);
    }
}

fn slots(l: usize) -> Vec<u64> {
    with(|e| e.lists.get(&l).map(|s| s.dedupe.values().copied().collect()).unwrap_or_default())
}

unsafe fn draw_once(g: &Gpu, f: &Gfx, r: &mut Run, via: Via, va: u64) -> (Vec<u8>, Vec<u64>) {
    unsafe {
        core::ptr::write_bytes(f.rb.cpu, 0xcd, PITCH as usize);
        record_draw(f, r.list, via, va);
        let s = slots(r.list);
        execute(g, r);
        let out = get(&f.rb, 0, CB_BYTES);
        restart(r);
        (out, s)
    }
}

fn hexdiff(a: &[u8], b: &[u8]) -> String {
    a.chunks(16).zip(b.chunks(16)).enumerate().filter(|(_, (x, y))| x != y).take(6)
        .map(|(i, (x, y))| format!("  cb0[{i}] got {x:02x?} want {y:02x?}")).collect::<Vec<_>>().join("\n")
}

fn same(what: &str, got: &[u8], want: &[u8]) {
    assert_eq!(got.len(), want.len());
    assert!(got == want, "{what} differs:\n{}", hexdiff(got, want));
}

struct Gfxs {
    f: Gfx,
}

unsafe impl Send for Gfxs {}
unsafe impl Sync for Gfxs {}

fn fixture() -> &'static Gfx {
    static F: OnceLock<Gfxs> = OnceLock::new();
    &F.get_or_init(|| Gfxs { f: unsafe { gfx(gpu()) } }).f
}

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

unsafe fn raw_vs_copy(off: usize, tracked_extra: Option<usize>, seed: u32) {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g = gpu();
    let f = fixture();
    let size = 8192usize;
    let tracked = tracked_extra.map(|n| (off + n) as u64).unwrap_or(size as u64);
    let src = unsafe { buffer(g.dev, HEAP_UPLOAD, size as u64, 0, ST_GENERIC_READ, Some(tracked)) };
    let data = pattern(seed, size);
    unsafe { put(&src, 0, &data) };
    let mut want = data[off..off + CB_BYTES].to_vec();
    if let Some(n) = tracked_extra {
        want[n..].iter_mut().for_each(|b| *b = 0);
        unsafe { put(&src, off + n, &vec![0xa5u8; size - off - n]) };
    }
    let va = src.va + off as u64;
    let mut r = unsafe { run_new(g, LIST_DIRECT) };
    let (copy, s) = unsafe { draw_once(g, f, &mut r, Via::Copy, va) };
    if va % SLOT_ALIGN == 0 {
        assert!(s.is_empty(), "an aligned root CBV was copied: {s:x?}");
    } else {
        assert_eq!(s.len(), 1, "an unaligned root CBV was not copied exactly once");
        assert_eq!(s[0] % SLOT_ALIGN, 0);
    }
    same("cbv_raw=copy output vs the source bytes", &copy, &want);
    if let Some(n) = tracked_extra {
        unsafe { put(&src, off + n, &vec![0u8; size - off - n]) };
    }
    let (raw, _) = unsafe { draw_once(g, f, &mut r, Via::Raw, va) };
    same("cbv_raw=copy vs cbv_raw=on (raw SRV) output", &copy, &raw);
    eprintln!("va={va:#x} (+{:#x} from 256) {} bytes bit-exact raw == copy == source{}", va % SLOT_ALIGN, CB_BYTES, if tracked_extra.is_some() { " with zero tail" } else { "" });
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_cbv_copy -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_cbv_copy_aligned_root_cbv_matches_raw() {
    if skip() {
        return;
    }
    unsafe { raw_vs_copy(0x200, None, 0x1111_0000) };
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_cbv_copy -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_cbv_copy_unaligned_root_cbv_matches_raw() {
    if skip() {
        return;
    }
    unsafe { raw_vs_copy(0x2b0, None, 0x2222_0000) };
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_cbv_copy -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_cbv_copy_short_source_gets_a_zero_tail_and_matches_raw() {
    if skip() {
        return;
    }
    unsafe { raw_vs_copy(0x1e50, Some(100), 0x3333_0000) };
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_cbv_copy -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_cbv_copy_two_draws_reuse_a_slot_after_the_fence() {
    if skip() {
        return;
    }
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g = gpu();
    let f = fixture();
    let a = unsafe { buffer(g.dev, HEAP_UPLOAD, 4096, 0, ST_GENERIC_READ, Some(4096)) };
    let b = unsafe { buffer(g.dev, HEAP_UPLOAD, 4096, 0, ST_GENERIC_READ, Some(4096)) };
    let (da, db) = (pattern(0x4444_0000, 4096), pattern(0x5555_0000, 4096));
    unsafe { put(&a, 0, &da) };
    unsafe { put(&b, 0, &db) };
    let off = 0x150usize;
    let hidden = with(|e| {
        let ring = e.ring(g.dev);
        ring.reclaim(&|f, v| unsafe { crate::cl2_state::fence_passed(f, v) });
        std::mem::take(&mut ring.free)
    });
    let mut r = unsafe { run_new(g, LIST_DIRECT) };
    let (c1, s1) = unsafe { draw_once(g, f, &mut r, Via::Copy, a.va + off as u64) };
    same("first copy draw", &c1, &da[off..off + CB_BYTES]);
    assert_eq!(s1.len(), 1);
    let chunks = with(|e| e.ring(g.dev).chunks.len());
    let retired = with(|e| e.ring(g.dev).retired_count());
    assert!(retired >= 1, "the first list's chunk was not retired behind its fence at Reset");
    let (c2, s2) = unsafe { draw_once(g, f, &mut r, Via::Copy, b.va + off as u64) };
    let grown = with(|e| {
        let ring = e.ring(g.dev);
        let n = ring.chunks.len();
        ring.free.extend(hidden);
        n
    });
    same("second copy draw (reused slot)", &c2, &db[off..off + CB_BYTES]);
    assert_eq!(s2, s1, "the slot was not reused once its fence passed");
    assert_eq!(grown, chunks, "the ring grew although the old slot was free");
    let mut r2 = unsafe { run_new(g, LIST_DIRECT) };
    let (raw, _) = unsafe { draw_once(g, f, &mut r2, Via::Raw, b.va + off as u64) };
    same("reused-slot copy vs raw", &c2, &raw);
    eprintln!("slot {:#x} reused across the fence; both draws bit-exact", s1[0]);
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_cbv_copy -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_cbv_copy_compute_list_dispatch_matches_raw() {
    if skip() {
        return;
    }
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g = gpu();
    let sigs = unsafe { sigs(g.dev, "RootFlags(0), CBV(b0), UAV(u0)", "cs_6_0") };
    let cs_text = cs_src();
    let raw_text = tombox_xshader::cbv_raw_hlsl::rewrite_hlsl(&cs_text).expect("rewrite_hlsl");
    let cs = compile(&cs_text, "cs_6_0");
    let raw_cs = compile(&raw_text, "cs_6_0");
    let copy_pso = unsafe { compute_pso(g.dev, sigs.copy, &cs) };
    let raw_pso = unsafe { compute_pso(g.dev, sigs.raw, &raw_cs) };
    with(|e| e.pso_sizes.insert(copy_pso, sizes_for(0, &cs_text)));
    let size = 4096u64;
    let up = unsafe { buffer(g.dev, HEAP_UPLOAD, size, 0, ST_GENERIC_READ, None) };
    let data = pattern(0x6666_0000, size as usize);
    unsafe { put(&up, 0, &data) };
    let src = unsafe { buffer(g.dev, HEAP_DEFAULT, size, 0, ST_COPY_DEST, Some(size)) };
    let uav = unsafe { buffer(g.dev, HEAP_DEFAULT, 1024, 4, ST_UAV, None) };
    let rb = unsafe { buffer(g.dev, HEAP_READBACK, 1024, 0, ST_COPY_DEST, None) };
    let mut setup = unsafe { run_new(g, LIST_DIRECT) };
    unsafe { call!(setup.list, 15, (usize, u64, usize, u64, u64) -> (), src.res, 0, up.res, 0, size) };
    unsafe { execute(g, &mut setup) };
    let off = 0x3d0u64;
    let va = src.va + off;
    let want = data[off as usize..off as usize + CB_BYTES].to_vec();
    let mut r = unsafe { run_new(g, LIST_COMPUTE) };
    assert_eq!(unsafe { list_type(r.list as *mut c_void) }, LIST_COMPUTE);
    let mut outs = Vec::new();
    for via in [Via::Copy, Via::Raw] {
        let l = r.list;
        let lp = l as *mut c_void;
        unsafe {
            core::ptr::write_bytes(rb.cpu, 0xcd, 1024);
            barrier(l, src.res, ST_COMMON, ST_NPSR);
            crate::gpu_va::set_state(src.res, ST_NPSR);
            match via {
                Via::Raw => {
                    call!(l, P_COMPUTE_RS, (usize) -> (), sigs.raw);
                    call!(l, P_PSO, (usize) -> (), raw_pso);
                    call!(l, P_COMPUTE_SRV, (u32, u64) -> (), 0, va);
                    call!(l, P_COMPUTE_UAV, (u32, u64) -> (), 1, uav.va);
                }
                Via::Copy => {
                    call!(l, P_COMPUTE_RS, (usize) -> (), sigs.copy);
                    with(|e| e.on_root_signature(l, SIDE_COMPUTE, sigs.copy));
                    call!(l, P_PSO, (usize) -> (), copy_pso);
                    set_root_cbv(lp, SIDE_COMPUTE, 0, va, &|v| bind(lp, SIDE_COMPUTE, 0, v));
                    call!(l, P_COMPUTE_UAV, (u32, u64) -> (), 1, uav.va);
                    resolve_with(lp, MASK_COMPUTE, Some(copy_pso), None);
                    assert!(!with(|e| e.has_pending(l, MASK_COMPUTE)));
                }
            }
            call!(l, P_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
            barrier(l, uav.res, ST_UAV, ST_COPY_SOURCE);
            call!(l, 15, (usize, u64, usize, u64, u64) -> (), rb.res, 0, uav.res, 0, CB_BYTES as u64);
            barrier(l, uav.res, ST_COPY_SOURCE, ST_UAV);
            barrier(l, src.res, ST_NPSR, ST_COMMON);
            crate::gpu_va::set_state(src.res, ST_COMMON);
            let s = slots(l);
            if via == Via::Copy {
                assert_eq!(s.len(), 1, "the unaligned compute root CBV was not copied");
            }
            execute(g, &mut r);
            outs.push(get(&rb, 0, CB_BYTES));
            restart(&mut r);
        }
    }
    same("compute cbv_raw=copy output vs source", &outs[0], &want);
    same("compute cbv_raw=copy vs cbv_raw=on", &outs[0], &outs[1]);
    eprintln!("compute list (type 2) on the compute queue: va={va:#x}, DEFAULT-heap source in NON_PIXEL_SHADER_RESOURCE, {CB_BYTES} bytes bit-exact raw == copy == source");
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_cbv_copy -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_cbv_copy_nr_limit_copies_only_nr_bytes_and_zero_fills_the_slot() {
    if skip() {
        return;
    }
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g = gpu();
    let f = fixture();
    let src = unsafe { buffer(g.dev, HEAP_UPLOAD, 4096, 0, ST_GENERIC_READ, Some(4096)) };
    let data = pattern(0x7777_0000, 4096);
    unsafe { put(&src, 0, &data) };
    let off = 0x1b0usize;
    let nr = 0x44usize;
    let mut want = data[off..off + CB_BYTES].to_vec();
    want[nr..].iter_mut().for_each(|b| *b = 0);
    with(|e| e.cb_limits.insert(f.copy_pso, vec![(5, 0, 0, nr as u64)]));
    let mut r = unsafe { run_new(g, LIST_DIRECT) };
    let (copy, s) = unsafe { draw_once(g, f, &mut r, Via::Copy, src.va + off as u64) };
    with(|e| e.cb_limits.remove(&f.copy_pso));
    assert_eq!(s.len(), 1);
    same("NR-limited copy (data up to NR*stride, zeros after)", &copy, &want);
    let (full, _) = unsafe { draw_once(g, f, &mut r, Via::Copy, src.va + off as u64) };
    same("copy without a limit", &full, &data[off..off + CB_BYTES]);
    eprintln!("NR*stride={nr:#x}: {nr} bytes copied, the remaining {} bytes of the slot read as zero", CB_BYTES - nr);
}

unsafe fn heap_handles(dev: usize) -> (usize, usize, u64) {
    let hd = [0u32, 1, 1, 0];
    let mut h: *mut c_void = core::ptr::null_mut();
    assert!(unsafe { call!(dev, 14, (*const u32, *const Guid, *mut *mut c_void) -> i32, hd.as_ptr(), &IID_DESC_HEAP, &mut h) } >= 0);
    let mut cpu = 0usize;
    let ret = unsafe { call!(h, 9, (*mut usize) -> usize, &mut cpu) };
    if ret != &mut cpu as *mut usize as usize {
        cpu = ret;
    }
    let mut gpu = 0u64;
    let ret = unsafe { call!(h, 10, (*mut u64) -> usize, &mut gpu) };
    if ret != &mut gpu as *mut u64 as usize {
        gpu = ret as u64;
    }
    (h as usize, cpu, gpu)
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_cbv_copy -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_cbv_copy_unaligned_descriptor_table_cbv_reads_through_a_raw_view_next_to_a_root_cbv() {
    if skip() {
        return;
    }
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g = gpu();
    let f = fixture();
    let rs_text = "RootFlags(0), CBV(b0, visibility=SHADER_VISIBILITY_PIXEL), DescriptorTable(CBV(b1), visibility=SHADER_VISIBILITY_PIXEL)";
    let probe = format!("#define RS \"{rs_text}\"\n[RootSignature(RS)] float4 main() : SV_Position {{ return 0; }}");
    let blob = compile(&probe, "vs_6_0");
    let parsed = tombox_xshader::parse_root_signature(&blob).expect("parse root signature");
    let tables = table_cbv_ranges(Some(&parsed));
    assert_eq!(tables, vec![(1, 1, 0)]);
    let (tbl, p) = crate::cbv_raw::table_blob(&blob).expect("table_blob").expect("a CBV range to rebind");
    assert_eq!((p.ranges, p.root_cbvs.len()), (1, 0));
    assert_eq!(crate::cbv_raw::root_cbv_params(&tbl), crate::cbv_raw::root_cbv_params(&blob), "the root CBV must stay a root CBV");
    let rs = unsafe { root_sig(g.dev, &tbl) };
    let ps_text = format!("cbuffer CB0 : register(b0) {{ uint4 cb0[{N}]; }};\ncbuffer CB1 : register(b1) {{ uint4 cb1[23]; }};\nuint4 main(float4 p : SV_Position) : SV_Target {{ uint i = (uint)p.x; return cb0[i] ^ cb1[i]; }}\n");
    let copy_text = tombox_xshader::cbv_raw_hlsl::rewrite_hlsl_where(&ps_text, &|r, sp| in_table(&tables, r, sp)).expect("rewrite_hlsl_where");
    assert!(copy_text.contains("cbuffer CB0 : register(b0)") && copy_text.contains("tb_cbr1.Load4"), "{copy_text}");
    let vs = compile(VS, "vs_6_0");
    let ps = compile(&copy_text, "ps_6_0");
    let pso = unsafe { gfx_pso(g.dev, rs, &vs, &ps) };
    let size = 4096usize;
    let src = unsafe { buffer(g.dev, HEAP_UPLOAD, size as u64, 0, ST_GENERIC_READ, Some(size as u64)) };
    let data = pattern(0x7777_0000, size);
    unsafe { put(&src, 0, &data) };
    let (root_off, table_off) = (0x200usize, 0x4b0usize);
    let table_va = src.va + table_off as u64;
    assert_ne!(table_va % SLOT_ALIGN, 0);
    let (res, d) = crate::cbv_raw::raw_view(table_va, 368, &crate::gpu_va::lookup).expect("raw view of the table CBV");
    assert_eq!(res, src.res);
    let (heap, cpu, gpu_h) = unsafe { heap_handles(g.dev) };
    unsafe { call!(g.dev, 18, (*mut c_void, *const u8, usize) -> (), res as *mut c_void, d.as_ptr(), cpu) };
    let want: Vec<u8> = (0..CB_BYTES).map(|i| data[root_off + i] ^ data[table_off + i]).collect();
    let mut r = unsafe { run_new(g, LIST_DIRECT) };
    let l = r.list;
    unsafe {
        core::ptr::write_bytes(f.rb.cpu, 0xcd, PITCH as usize);
        call!(l, P_RTS, (u32, *const usize, i32, *const usize) -> (), 1, &f.rtv, 0, core::ptr::null());
        let vp = [0f32, 0.0, N as f32, 1.0, 0.0, 1.0];
        call!(l, P_VIEWPORTS, (u32, *const f32) -> (), 1, vp.as_ptr());
        let sc = [0i32, 0, N as i32, 1];
        call!(l, P_SCISSORS, (u32, *const i32) -> (), 1, sc.as_ptr());
        call!(l, P_TOPOLOGY, (u32) -> (), 4);
        let heaps = [heap];
        call!(l, 28, (u32, *const usize) -> (), 1, heaps.as_ptr());
        call!(l, P_GRAPHICS_RS, (usize) -> (), rs);
        call!(l, P_PSO, (usize) -> (), pso);
        call!(l, 38, (u32, u64) -> (), 0, src.va + root_off as u64);
        call!(l, 32, (u32, u64) -> (), 1, gpu_h);
        call!(l, P_DRAW, (u32, u32, u32, u32) -> (), 3, 1, 0, 0);
        barrier(l, f.rt, ST_RT, ST_COPY_SOURCE);
        let mut dst = [0u8; 48];
        dst[0..8].copy_from_slice(&(f.rb.res as u64).to_le_bytes());
        dst[8..12].copy_from_slice(&1u32.to_le_bytes());
        dst[24..28].copy_from_slice(&FMT_RGBA32_UINT.to_le_bytes());
        dst[28..32].copy_from_slice(&(N as u32).to_le_bytes());
        dst[32..36].copy_from_slice(&1u32.to_le_bytes());
        dst[36..40].copy_from_slice(&1u32.to_le_bytes());
        dst[40..44].copy_from_slice(&(PITCH as u32).to_le_bytes());
        let mut s = [0u8; 48];
        s[0..8].copy_from_slice(&(f.rt as u64).to_le_bytes());
        call!(l, P_COPY_TEXTURE, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), dst.as_ptr(), 0, 0, 0, s.as_ptr(), core::ptr::null());
        barrier(l, f.rt, ST_COPY_SOURCE, ST_RT);
        execute(g, &mut r);
        let out = get(&f.rb, 0, CB_BYTES);
        same("root CBV ^ unaligned table CBV (raw view) output", &out, &want);
    }
    eprintln!("table CBV at +{:#x} from 256 read through a raw SRV next to a real root CBV: {} bytes bit-exact", table_va % SLOT_ALIGN, CB_BYTES);
}

fn issue105_shader_container(root: &[u8], value: u32) -> Vec<u8> {
    let store = [0, 0, 0, 0, 0x7e02_0280, 0x7e08_02ff, value, 0xf408_0002, 0xfa00_0000,
        0xe010_2000, 0x8000_0401, 0xbf81_0000];
    let mut fssm = vec![0; 0xe0];
    fssm[..4].copy_from_slice(b"FSSM");
    fssm[0xc0..0xc4].copy_from_slice(&0xe0u32.to_le_bytes());
    fssm[0xc8..0xcc].copy_from_slice(&((store.len() * 4) as u32).to_le_bytes());
    fssm.extend(store.into_iter().flat_map(u32::to_le_bytes));

    let mut dxil: Vec<_> = [0x0005_0060u32, 0, 0x4c49_5844, 0, 24, 4]
        .into_iter().flat_map(u32::to_le_bytes).collect();
    dxil.extend_from_slice(b"XDAT\0\0\0\0NOBC");
    let len = u16::try_from(fssm.len()).unwrap();
    dxil.extend_from_slice(&[0x78, 0x01, 0x01]);
    dxil.extend_from_slice(&len.to_le_bytes());
    dxil.extend_from_slice(&(!len).to_le_bytes());
    dxil.extend_from_slice(&fssm);
    let (a, b) = fssm.iter().fold((1u32, 0u32), |(a, b), &byte| {
        let a = (a + u32::from(byte)) % 65521;
        (a, (a + b) % 65521)
    });
    dxil.extend_from_slice(&((b << 16) | a).to_be_bytes());
    dxil.resize(dxil.len().next_multiple_of(4), 0);

    let root_offset = 48 + dxil.len() as u32;
    let size = root_offset + 8 + root.len() as u32;
    let mut blob = b"DXBC".to_vec();
    blob.extend_from_slice(&[0; 16]);
    blob.extend([1u32, size, 2, 40, root_offset].into_iter().flat_map(u32::to_le_bytes));
    for (name, bytes) in [(b"DXIL", dxil.as_slice()), (b"RTS0", root)] {
        blob.extend_from_slice(name);
        blob.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        blob.extend_from_slice(bytes);
    }
    blob
}

#[test]
#[ignore = "requires Direct3D 12 and TOMBOX_TBUF_COMPUTE=1"]
fn hw_issue105_compute_switches_and_restores_pipeline() {
    assert!(crate::tbuf_compute::enabled(), "set TOMBOX_TBUF_COMPUTE=1");
    assert!(!crate::pso_async::enabled(), "set TOMBOX_PSO_ASYNC=0 for this test");
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g = gpu();
    const VALUE: u32 = 0xc0ff_ee00;
    const FORMATS: [u32; 4] = [42, 43, 42, 41];

    unsafe {
        let root_blob = compile(
            r#"[RootSignature("RootFlags(0), DescriptorTable(UAV(u0, flags=DATA_VOLATILE))")]
               [numthreads(1, 1, 1)] void main() {}"#,
            "cs_6_0",
        );
        let dxbc = tombox_xshader::Dxbc::parse(&root_blob).unwrap();
        let rts = dxbc.chunk(b"RTS0").unwrap();
        let shader = issue105_shader_container(&root_blob[rts.offset..rts.offset + rts.size], VALUE);
        let translated = tombox_xshader::hlsl::translate(&tombox_xshader::extract(&shader).unwrap()).unwrap();
        assert_eq!(translated.tbuf_unresolved, [(true, 0)]);
        assert!(translated.hlsl.contains("RWBuffer<float4> utb0"));

        call!(g.dev, 1, () -> u32);
        let wrapper = crate::device_wrap::wrap_device(g.dev as *mut c_void);
        let root = root_sig(wrapper as usize, &root_blob);
        let pso = compute_pso(wrapper as usize, root, &shader);
        assert_eq!(crate::tbuf_compute::cached_variants_for_test(pso), Some(0), "pipeline metadata missing");

        let out = buffer(g.dev, HEAP_DEFAULT, 16, 4, ST_UAV, None);
        let readback = buffer(g.dev, HEAP_READBACK, 16, 0, ST_COPY_DEST, None);
        let mut heap: *mut c_void = core::ptr::null_mut();
        let heap_desc = [0u32, FORMATS.len() as u32, 1, 0];
        let hr = call!(wrapper, 14, (*const u32, *const Guid, *mut *mut c_void) -> i32,
            heap_desc.as_ptr(), &IID_DESC_HEAP, &mut heap);
        assert!(hr >= 0 && !heap.is_null(), "CreateDescriptorHeap hr={hr:#010x}");
        let stride = call!(g.dev, 15, (u32) -> u32, 0) as u64;
        let cpu = call!(heap, 9, () -> usize);
        let gpu = call!(heap, 10, () -> u64);
        for (i, fmt) in FORMATS.into_iter().enumerate() {
            let mut d = [0u8; 40];
            d[..4].copy_from_slice(&fmt.to_le_bytes());
            d[4..8].copy_from_slice(&1u32.to_le_bytes());
            d[8..16].copy_from_slice(&(i as u64).to_le_bytes());
            d[16..20].copy_from_slice(&1u32.to_le_bytes());
            call!(g.dev, 19, (usize, *mut c_void, *const u8, usize) -> (),
                out.res, core::ptr::null_mut(), d.as_ptr(), cpu + i * stride as usize);
            assert_eq!(
                crate::bindtrace::typed_buffer_format(&[heap as usize], gpu + i as u64 * stride, 0, true),
                Some(fmt), "descriptor {i} was not recognized",
            );
        }

        let mut run = run_new(g, LIST_COMPUTE);
        let shim = crate::cmdlist_wrap::wrap(run.list as *mut c_void);
        call!(shim, 32, (u32, *const *mut c_void) -> (), 1, &heap);
        call!(shim, 33, (usize) -> (), root);
        call!(shim, 25, (usize) -> (), pso);
        for (index, (count, extended)) in [(1, false), (2, true), (2, false)].into_iter().enumerate() {
            call!(shim, 35, (u32, u64) -> (), 0, gpu + index as u64 * stride);
            if extended {
                call!(shim, crate::console_slots::CL_DISPATCH_X, (u32, u32, u32, u32) -> (), 1, 1, 1, 0);
            } else {
                call!(shim, 14, (u32, u32, u32) -> (), 1, 1, 1);
            }
            assert_eq!(crate::tbuf_compute::cached_variants_for_test(pso), Some(count), "dispatch {index}");
        }

        call!(shim, 35, (u32, u64) -> (), 0, gpu + 3 * stride);
        call!(run.list, P_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
        barrier(run.list, out.res, ST_UAV, ST_COPY_SOURCE);
        call!(run.list, 15, (usize, u64, usize, u64, u64) -> (), readback.res, 0, out.res, 0, 16);
        execute(g, &mut run);
        assert_eq!(get(&readback, 0, 16), VALUE.to_le_bytes().repeat(4));
        println!("ISSUE 105 GPU PASS: UINT, SINT, cached UINT, restored FLOAT");

        call!(shim, 2, () -> u32);
        for obj in [run.alloc, run.fence, out.res, readback.res, heap as usize, pso, root, wrapper as usize] {
            call!(obj, 2, () -> u32);
        }
    }
}
