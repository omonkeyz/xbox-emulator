use core::ffi::c_void;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::device_wrap::Guid;

const IID_DEVICE: Guid = Guid { d1: 0x1898_19f1, d2: 0x1db6, d3: 0x4b57, d4: [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7] };
const IID_QUEUE: Guid = Guid { d1: 0x0ec8_70a6, d2: 0x5d7e, d3: 0x4c22, d4: [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed] };
const IID_ALLOCATOR: Guid = Guid { d1: 0x6102_dee4, d2: 0xaf59, d3: 0x4b09, d4: [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24] };
const IID_LIST: Guid = Guid { d1: 0x5b16_0d0f, d2: 0xac1b, d3: 0x4185, d4: [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55] };
const IID_RESOURCE: Guid = Guid { d1: 0x6964_42be, d2: 0xa72e, d3: 0x4059, d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad] };
const IID_FENCE: Guid = Guid { d1: 0x0a75_3dcf, d2: 0xc4d8, d3: 0x4b91, d4: [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76] };
const IID_ROOT_SIGNATURE: Guid = Guid { d1: 0xc54a_6b66, d2: 0x72df, d3: 0x4ee8, d4: [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14] };
const IID_PSO: Guid = Guid { d1: 0x765a_30f3, d2: 0xf624, d3: 0x4c6f, d4: [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45] };
const IID_FACTORY4: Guid = Guid { d1: 0x1bc6_ea02, d2: 0xef36, d3: 0x464f, d4: [0xbf, 0x0c, 0x21, 0xca, 0x39, 0xe5, 0x16, 0x8a] };
const IID_ADAPTER: Guid = Guid { d1: 0x2411_e7e1, d2: 0x12ac, d3: 0x4ccf, d4: [0xbd, 0x14, 0x97, 0x98, 0xe8, 0x53, 0x4d, 0xc0] };

const ST_COMMON: u32 = 0;
const ST_COPY_DEST: u32 = 0x400;
const ST_COPY_SOURCE: u32 = 0x800;
const ST_GENERIC_READ: u32 = 0xac3;
const HEAP_DEFAULT: u32 = 1;
const HEAP_UPLOAD: u32 = 2;
const HEAP_READBACK: u32 = 3;
const FLAG_UAV: u32 = 4;
const FUNC_EQUAL: u32 = 3;
const FUNC_GREATER_EQUAL: u32 = 7;

const X_CLOSE: usize = 9;
const X_DISPATCH: usize = 14;
const X_COPY_BUFFER_REGION: usize = 15;
const X_SET_PSO: usize = 25;
const X_BARRIER: usize = 26;
const X_SET_COMPUTE_ROOT_SIGNATURE: usize = 33;
const X_SET_COMPUTE_ROOT_CONSTANT: usize = 37;
const PC_SET_COMPUTE_ROOT_SIGNATURE: usize = 29;
const PC_SET_COMPUTE_ROOT_UAV: usize = 41;

extern "system" {
    fn QueryPerformanceCounter(v: *mut i64) -> i32;
    fn QueryPerformanceFrequency(v: *mut i64) -> i32;
    fn LoadLibraryW(p: *const u16) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
}

unsafe fn vs(o: *mut c_void, i: usize) -> usize {
    unsafe { *(*(o as *const *const usize)).add(i) }
}

macro_rules! call {
    ($o:expr, $i:expr, ($($t:ty),*) -> $r:ty $(, $a:expr)*) => {{
        let o = $o as *mut c_void;
        let f: unsafe extern "system" fn(*mut c_void $(, $t)*) -> $r = core::mem::transmute(vs(o, $i));
        f(o $(, $a)*)
    }};
}

struct Gpu {
    dev: usize,
    a: usize,
    b: usize,
    warp: bool,
}

unsafe impl Send for Gpu {}
unsafe impl Sync for Gpu {}

fn adapter_is_warp() -> bool {
    std::env::var("TOMBOX_HW_ADAPTER").map(|v| v.eq_ignore_ascii_case("warp")).unwrap_or(false)
}

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
    assert!(call!(f, 27, (*const Guid, *mut *mut c_void) -> i32, &IID_ADAPTER, &mut a) >= 0);
    a
}

unsafe fn queue(dev: *mut c_void) -> usize {
    let desc = [0u32; 4];
    let mut q: *mut c_void = core::ptr::null_mut();
    assert!(call!(dev, 8, (*const u32, *const Guid, *mut *mut c_void) -> i32, desc.as_ptr(), &IID_QUEUE, &mut q) >= 0);
    let f16 = unsafe { vs(q, 16) };
    let f17 = unsafe { vs(q, 17) };
    crate::sys_ts::note_queue(q, f16, f17);
    crate::present::apply_queue_vtable(q);
    q as usize
}

fn gpu() -> &'static Gpu {
    static G: OnceLock<Gpu> = OnceLock::new();
    G.get_or_init(|| unsafe {
        crate::enable_experimental_shader_models();
        let warp = adapter_is_warp();
        let adapter = if warp { warp_adapter() } else { core::ptr::null_mut() };
        let p = crate::host_proc(b"D3D12CreateDevice\0");
        let create: extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(p);
        let mut dev: *mut c_void = core::ptr::null_mut();
        let hr = create(adapter, 0xb000, &IID_DEVICE, &mut dev);
        assert!(hr >= 0 && !dev.is_null(), "D3D12CreateDevice ({}) hr={hr:#010x}", if warp { "WARP" } else { "hardware" });
        Gpu { dev: dev as usize, a: queue(dev), b: queue(dev), warp }
    })
}

struct Buf {
    res: usize,
    va: u64,
    cpu: *mut u8,
}

unsafe fn buffer(dev: usize, heap: u32, size: u64, flags: u32, state: u32) -> Buf {
    let h: [u32; 5] = [heap, 0, 0, 1, 1];
    let mut d = [0u8; 56];
    d[0..4].copy_from_slice(&1u32.to_le_bytes());
    d[0x10..0x18].copy_from_slice(&size.to_le_bytes());
    d[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
    d[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
    d[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
    d[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
    d[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
    d[0x30..0x34].copy_from_slice(&flags.to_le_bytes());
    let mut r: *mut c_void = core::ptr::null_mut();
    let hr = call!(dev, 27, (*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
        h.as_ptr(), 0, d.as_ptr(), state, core::ptr::null(), &IID_RESOURCE, &mut r);
    assert!(hr >= 0, "CreateCommittedResource hr={hr:#010x}");
    let va = call!(r, 11, () -> u64);
    crate::gpu_va::insert(crate::gpu_va::Entry { start: va, len: size, res: r as usize, buffer: true });
    crate::gpu_va::set_state(r as usize, state);
    let mut cpu: *mut u8 = core::ptr::null_mut();
    if heap != HEAP_DEFAULT {
        assert!(call!(r, 8, (u32, *const c_void, *mut *mut u8) -> i32, 0, core::ptr::null(), &mut cpu) >= 0);
    }
    Buf { res: r as usize, va, cpu }
}

unsafe fn raw_list(dev: usize) -> *mut c_void {
    let mut a: *mut c_void = core::ptr::null_mut();
    assert!(call!(dev, 9, (u32, *const Guid, *mut *mut c_void) -> i32, 0, &IID_ALLOCATOR, &mut a) >= 0);
    let mut l: *mut c_void = core::ptr::null_mut();
    assert!(call!(dev, 12, (u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32, 0, 0, a, core::ptr::null_mut(), &IID_LIST, &mut l) >= 0);
    l
}

unsafe fn title_list(dev: usize) -> *mut c_void {
    crate::cmdlist_wrap::wrap(unsafe { raw_list(dev) })
}

unsafe fn fence(dev: usize) -> *mut c_void {
    let mut f: *mut c_void = core::ptr::null_mut();
    assert!(call!(dev, 36, (u64, u32, *const Guid, *mut *mut c_void) -> i32, 0, 0, &IID_FENCE, &mut f) >= 0);
    f
}

unsafe fn submit(q: usize, list: *mut c_void, f: *mut c_void, v: u64) {
    let l = [list];
    call!(q, 10, (u32, *const *mut c_void) -> (), 1, l.as_ptr());
    call!(q, 14, (*mut c_void, u64) -> i32, f, v);
}

fn reached(f: *mut c_void, v: u64, within: Duration) -> bool {
    let t = Instant::now();
    loop {
        if unsafe { call!(f, 8, () -> u64) } >= v {
            return true;
        }
        if t.elapsed() > within {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

unsafe fn rd(b: &Buf, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile(b.cpu.add(off) as *const u32) }
}

unsafe fn compute_pso(dev: usize, hlsl: &str, rs: Option<*mut c_void>) -> (*mut c_void, *mut c_void) {
    let code = tombox_xshader::dxc::compile(hlsl, "cs_6_0").expect("dxc");
    let rs = rs.unwrap_or_else(|| {
        let mut r: *mut c_void = core::ptr::null_mut();
        assert!(call!(dev, 16, (u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32, 0, code.as_ptr(), code.len(), &IID_ROOT_SIGNATURE, &mut r) >= 0);
        r
    });
    let mut d = [0u8; 56];
    d[0..8].copy_from_slice(&(rs as u64).to_le_bytes());
    d[8..16].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
    d[16..24].copy_from_slice(&(code.len() as u64).to_le_bytes());
    let mut pso: *mut c_void = core::ptr::null_mut();
    let hr = call!(dev, 11, (*const u8, *const Guid, *mut *mut c_void) -> i32, d.as_ptr(), &IID_PSO, &mut pso);
    assert!(hr >= 0, "CreateComputePipelineState hr={hr:#010x}");
    (rs, pso)
}

fn chain_len(shim: *mut c_void) -> usize {
    crate::list_split::expand(&[crate::cmdlist_wrap::unwrap(shim)]).len()
}

unsafe fn waiter(g: &Gpu, flag: &Buf, func: u32, value: u32, out: &Buf) -> *mut c_void {
    let w = unsafe { title_list(g.dev) };
    call!(w, crate::cl_ext::CL_WRITE32_BOTTOM_OF_PIPE, (u64, u32, u32, u32) -> (), out.va, 1, 0, 0);
    call!(w, crate::cl_ext::CL_WAIT32, (u64, u32, u32, u32, u32) -> (), flag.va, func, value, 0xffff_ffff, 0);
    call!(w, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), out.res, 8, flag.res, 0, 4);
    call!(w, crate::cl_ext::CL_WRITE32_BOTTOM_OF_PIPE, (u64, u32, u32, u32) -> (), out.va + 16, 2, 0, 0);
    assert!(call!(w, X_CLOSE, () -> i32) >= 0);
    w
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_mid_list_wait_orders_after_a_value_written_by_another_list() {
    let g = gpu();
    unsafe {
        let flag = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_COMMON);
        let out = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let w = waiter(g, &flag, FUNC_GREATER_EQUAL, 42, &out);
        assert_eq!(chain_len(w), 2, "the wait did not split the list");
        let fa = fence(g.dev);
        submit(g.a, w, fa, 1);
        assert!(!reached(fa, 1, Duration::from_millis(300)), "the waiter ran past the wait before the value was written");
        assert_eq!(rd(&out, 0), 1, "the part before the wait did not execute");
        assert_eq!(rd(&out, 16), 0);
        let p = title_list(g.dev);
        call!(p, crate::cl_ext::CL_WRITE32_BOTTOM_OF_PIPE, (u64, u32, u32, u32) -> (), flag.va, 42, 0, 0);
        assert!(call!(p, X_CLOSE, () -> i32) >= 0);
        let fb = fence(g.dev);
        submit(g.b, p, fb, 1);
        assert!(reached(fb, 1, Duration::from_secs(5)));
        assert!(reached(fa, 1, Duration::from_secs(5)), "the waiter never resumed after the producer wrote the value");
        assert_eq!((rd(&out, 0), rd(&out, 8), rd(&out, 16)), (1, 42, 2));
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_mid_list_wait_after_its_producer_was_submitted_uses_the_address_fence() {
    let g = gpu();
    unsafe {
        let flag = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_COMMON);
        let out = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let p = title_list(g.dev);
        call!(p, crate::cl_ext::CL_WRITE32_BOTTOM_OF_PIPE, (u64, u32, u32, u32) -> (), flag.va, 42, 0, 0);
        assert!(call!(p, X_CLOSE, () -> i32) >= 0);
        let fb = fence(g.dev);
        submit(g.b, p, fb, 1);
        let w = waiter(g, &flag, FUNC_GREATER_EQUAL, 42, &out);
        assert_eq!(chain_len(w), 2, "the wait did not split the list");
        let before = crate::wait_probe::armed_total();
        let fa = fence(g.dev);
        submit(g.a, w, fa, 1);
        assert_eq!(crate::wait_probe::armed_total(), before, "a probe was armed although the producing write was already submitted");
        assert!(reached(fb, 1, Duration::from_secs(5)));
        assert!(reached(fa, 1, Duration::from_secs(5)), "the waiter never resumed");
        assert_eq!((rd(&out, 0), rd(&out, 8), rd(&out, 16)), (1, 42, 2));
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_mid_list_wait_on_a_shader_written_default_heap_value() {
    let g = gpu();
    unsafe {
        let flag = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_COMMON);
        let out = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let w = waiter(g, &flag, FUNC_EQUAL, 77, &out);
        assert_eq!(chain_len(w), 2);
        let fa = fence(g.dev);
        submit(g.a, w, fa, 1);
        assert!(!reached(fa, 1, Duration::from_millis(300)), "the waiter ran past a wait on a value no shader had written yet");
        assert_eq!((rd(&out, 0), rd(&out, 16)), (1, 0));
        let (rs, pso) = compute_pso(g.dev, "#define RS \"UAV(u0)\"\nRWByteAddressBuffer o : register(u0);\n[RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() { o.Store(0, 77u); }\n", None);
        let p = raw_list(g.dev);
        call!(p, PC_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), rs);
        call!(p, X_SET_PSO, (*mut c_void) -> (), pso);
        call!(p, PC_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 0, flag.va);
        call!(p, X_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
        assert!(call!(p, X_CLOSE, () -> i32) >= 0);
        let fb = fence(g.dev);
        submit(g.b, p, fb, 1);
        assert!(reached(fb, 1, Duration::from_secs(5)));
        assert!(reached(fa, 1, Duration::from_secs(5)), "the waiter never resumed after the shader wrote the value");
        assert_eq!((rd(&out, 0), rd(&out, 8), rd(&out, 16)), (1, 77, 2));
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_gds_round_trip_through_the_shader_binding() {
    let g = gpu();
    unsafe {
        let mut param = [0u8; 32];
        param[0..4].copy_from_slice(&1u32.to_le_bytes());
        param[16..20].copy_from_slice(&1u32.to_le_bytes());
        let mut desc = [0u8; 40];
        desc[0..4].copy_from_slice(&1u32.to_le_bytes());
        desc[8..16].copy_from_slice(&(param.as_ptr() as u64).to_le_bytes());
        let (mut blob, mut err): (*mut c_void, *mut c_void) = (core::ptr::null_mut(), core::ptr::null_mut());
        assert!(crate::D3D12SerializeRootSignature(desc.as_ptr() as *const c_void, 1, &mut blob, &mut err) >= 0);
        let bp = call!(blob, 3, () -> *const u8);
        let bl = call!(blob, 4, () -> usize);
        let (patched, idx) = crate::gds::patch_root_signature(bp, bl).expect("GDS root parameter not appended");
        assert_eq!(idx, 1);
        let mut rs: *mut c_void = core::ptr::null_mut();
        assert!(call!(g.dev, 16, (u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32, 0, patched.as_ptr(), patched.len(), &IID_ROOT_SIGNATURE, &mut rs) >= 0);
        crate::gds::note_root_signature(rs, idx);
        let hlsl = format!("{}\ncbuffer C : register(b0) {{ uint add; }};\n[numthreads(64, 1, 1)]\nvoid main() {{ uint o; tb_gds.InterlockedAdd(16, add, o); }}\n", tombox_xshader::gds_abi::TB_GDS_HLSL_DECL);
        let (_, pso) = compute_pso(g.dev, &hlsl, Some(rs));
        let src = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        core::ptr::write_volatile(src.cpu as *mut u32, 1000);
        core::ptr::write_volatile(src.cpu.add(4) as *mut u32, 5);
        let dst = buffer(g.dev, HEAP_DEFAULT, 256, 0, ST_COPY_DEST);
        let out = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let l = title_list(g.dev);
        call!(l, crate::cl_ext::CL_WRITE_GDS, (u32, u32, u64, u32) -> (), 4, 2, src.va, 0);
        call!(l, X_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), rs);
        call!(l, X_SET_PSO, (*mut c_void) -> (), pso);
        call!(l, X_SET_COMPUTE_ROOT_CONSTANT, (u32, u32, u32) -> (), 0, 3, 0);
        call!(l, X_DISPATCH, (u32, u32, u32) -> (), 4, 1, 1);
        call!(l, crate::cl_ext::CL_READ_GDS, (u32, u32, u64, u32) -> (), 4, 2, dst.va, 0);
        let mut b = [0u8; 32];
        b[8..16].copy_from_slice(&(dst.res as u64).to_le_bytes());
        b[16..20].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        b[20..24].copy_from_slice(&ST_COPY_DEST.to_le_bytes());
        b[24..28].copy_from_slice(&ST_COPY_SOURCE.to_le_bytes());
        call!(l, X_BARRIER, (u32, *const u8) -> (), 1, b.as_ptr());
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), out.res, 0, dst.res, 0, 8);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        assert_eq!((rd(&out, 0), rd(&out, 4)), (1000 + 256 * 3, 5));
    }
}

fn counter_window(t0: i64, t1: i64, qpf: i64) -> (u64, u64) {
    let hz = crate::sys_ts::system_counter_hz() as u128;
    let tol = hz / 500;
    let lo = (t0 as u128 * hz / qpf as u128).saturating_sub(tol);
    let hi = t1 as u128 * hz / qpf as u128 + tol;
    (lo as u64, hi as u64)
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_system_timestamp_counts_the_console_gpu_clock_from_the_qpc_epoch() {
    let g = gpu();
    unsafe {
        let out = buffer(g.dev, HEAP_READBACK, 64, 0, ST_COPY_DEST);
        let w = |l: *mut c_void, va: u64, flags: u32| call!(l, crate::cl_ext::CL_WRITE64_BOTTOM_OF_PIPE, (u64, u64, u32, u32) -> (), va, 0, 0, flags);
        let l = title_list(g.dev);
        w(l, out.va, crate::cl_ext::BOP_FLAG_SYSTEM_TIMESTAMP);
        w(l, out.va + 8, crate::cl_ext::BOP_FLAG_SYSTEM_TIMESTAMP);
        w(l, out.va + 16, crate::cl_ext::BOP_FLAG_GPU_TIMESTAMP);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let l2 = title_list(g.dev);
        w(l2, out.va + 24, crate::cl_ext::BOP_FLAG_GPU_TIMESTAMP);
        w(l2, out.va + 32, crate::cl_ext::BOP_FLAG_SYSTEM_TIMESTAMP);
        assert!(call!(l2, X_CLOSE, () -> i32) >= 0);
        let (mut t0, mut t1, mut t2, mut t3, mut qpf) = (0i64, 0i64, 0i64, 0i64, 0i64);
        QueryPerformanceFrequency(&mut qpf);
        let f = fence(g.dev);
        QueryPerformanceCounter(&mut t0);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        QueryPerformanceCounter(&mut t1);
        std::thread::sleep(Duration::from_millis(200));
        QueryPerformanceCounter(&mut t2);
        submit(g.a, l2, f, 2);
        assert!(reached(f, 2, Duration::from_secs(5)));
        QueryPerformanceCounter(&mut t3);
        let q = |o: usize| core::ptr::read_volatile(out.cpu.add(o) as *const u64);
        let (v0, v1, a0, a1, v2) = (q(0), q(8), q(16), q(24), q(32));
        let who = if g.warp { "WARP" } else { "hardware" };
        assert!(v0 != 0 && v1 != 0 && v2 != 0, "SYSTEM_TIMESTAMP not written");
        assert!(v0 <= v1 && v1 < v2, "not monotonic: {v0} {v1} {v2}");
        let (lo, hi) = counter_window(t0, t1, qpf);
        assert!(v0 >= lo && v1 <= hi, "{who}: counter window [{lo}, {hi}] but SYSTEM_TIMESTAMP {v0}, {v1}");
        let (lo2, hi2) = counter_window(t2, t3, qpf);
        assert!(v2 >= lo2 && v2 <= hi2, "{who}: counter window [{lo2}, {hi2}] but SYSTEM_TIMESTAMP {v2}");
        let mut gf = 0u64;
        assert!(call!(g.a, 16, (*mut u64) -> i32, &mut gf) >= 0 && gf != 0);
        let hz = crate::sys_ts::system_counter_hz() as f64;
        let want = (a1 - a0) as f64 * hz / gf as f64;
        let got = (v2 - v1) as f64;
        assert!((got - want).abs() <= want * 0.001 + hz * 0.0002, "{who}: SYSTEM delta {got} vs GPU delta scaled to {hz} Hz {want}");
        let wall = (t3 - t0) as f64 / qpf as f64;
        assert!(got / hz >= 0.19 && got / hz <= wall + 0.01, "{who}: SYSTEM delta {} s outside [0.19, {wall}] s", got / hz);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_write64_bottom_of_pipe_follows_the_release_umd_for_every_flag_and_flush_combination() {
    let g = gpu();
    unsafe {
        let out = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let l = title_list(g.dev);
        let w = |va: u64, value: u64, flush: u32, flags: u32| call!(l, crate::cl_ext::CL_WRITE64_BOTTOM_OF_PIPE, (u64, u64, u32, u32) -> (), va, value, flush, flags);
        w(out.va, 0x1122_3344_5566_7788, 0, 0x80);
        w(out.va + 8, 0xab, 0x4800, 0);
        w(out.va + 16, 0, 0x3800, 3);
        w(out.va + 24, 5, 0x800, 1);
        w(out.va + 32, 9, 0x1000, 0x30);
        w(out.va + 40, 0, 0x3800, 2);
        w(out.va + 48, 0xdead, 0x2000, 0x10);
        w(out.va + 60, 1, 0, 0);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        w(out.va + 64, 1, 0, 0);
        let (mut t0, mut t1, mut qpf) = (0i64, 0i64, 0i64);
        QueryPerformanceFrequency(&mut qpf);
        let f = fence(g.dev);
        QueryPerformanceCounter(&mut t0);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        QueryPerformanceCounter(&mut t1);
        let q = |o: usize| core::ptr::read_volatile(out.cpu.add(o) as *const u64);
        assert_eq!(q(0), 0x1122_3344_5566_7788, "an undefined flag bit dropped the value write");
        assert_eq!(q(8), 0xab, "an undefined flush bit dropped the value write");
        let (lo, hi) = counter_window(t0, t1, qpf);
        let (gt0, gt1) = (q(16), q(24));
        if crate::bop64::scarlett() {
            assert!(gt0 >= lo && gt0 <= hi, "Scarlett: both TIMESTAMP flags must give the SYSTEM counter, got {gt0} outside [{lo}, {hi}]");
            assert!(gt1 != 0 && gt1 != 5, "GPU timestamp {gt1} must ignore Value");
        } else {
            assert!(gt0 != 0 && gt1 >= gt0 && gt1 != 5, "GPU timestamps {gt0} {gt1}: GPU_TIMESTAMP must win over SYSTEM_TIMESTAMP and ignore Value");
            let mut gf = 0u64;
            assert!(call!(g.a, 16, (*mut u64) -> i32, &mut gf) >= 0 && gf != 0);
            let span = (t1 - t0) as u128 * gf as u128 / qpf as u128;
            assert!(((gt1 - gt0) as u128) <= span + 1, "two GPU timestamps of one list {gt0}..{gt1} further apart than the submit window");
        }
        assert_eq!(q(32), 9, "LRU|STREAM together dropped the value write");
        let st = q(40);
        assert!(st >= lo && st <= hi, "SYSTEM_TIMESTAMP {st} outside the console-clock window [{lo}, {hi}]");
        assert_eq!(q(48), 0xdead);
        assert_eq!(q(56), 0, "a write to a destination off 8-byte alignment was recorded");
        assert_eq!(q(64), 0, "a write recorded into a closed list reached the GPU");
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_write64_value_lands_as_one_copy_ignores_and_keeps_title_predication() {
    let g = gpu();
    unsafe {
        let pred = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        core::ptr::write_volatile(pred.cpu as *mut u64, 0);
        let src = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        core::ptr::write_volatile(src.cpu as *mut u64, 0xeeee_eeee_eeee_eeee);
        let dst = buffer(g.dev, HEAP_DEFAULT, 256, 0, ST_COPY_DEST);
        let out = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let l = title_list(g.dev);
        call!(l, 31, (*mut c_void, u64, u32) -> (), pred.res as *mut c_void, 0, 0);
        for i in 0..8u64 {
            call!(l, crate::cl_ext::CL_WRITE64_BOTTOM_OF_PIPE, (u64, u64, u32, u32) -> (), dst.va + 8 * i, 0x0102_0304_0506_0700 + i, 0x800, 0);
        }
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), dst.res, 64, src.res, 0, 8);
        call!(l, crate::cl_ext::CL_WRITE64_BOTTOM_OF_PIPE, (u64, u64, u32, u32) -> (), out.va + 128, u64::MAX - 1, 0, 0);
        call!(l, 31, (*mut c_void, u64, u32) -> (), core::ptr::null_mut::<c_void>(), 0, 0);
        let mut b = [0u8; 32];
        b[8..16].copy_from_slice(&(dst.res as u64).to_le_bytes());
        b[16..20].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        b[20..24].copy_from_slice(&ST_COPY_DEST.to_le_bytes());
        b[24..28].copy_from_slice(&ST_COPY_SOURCE.to_le_bytes());
        call!(l, X_BARRIER, (u32, *const u8) -> (), 1, b.as_ptr());
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), out.res, 0, dst.res, 0, 72);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        let q = |o: usize| core::ptr::read_volatile(out.cpu.add(o) as *const u64);
        for i in 0..8 {
            assert_eq!(q(8 * i), 0x0102_0304_0506_0700 + i as u64, "value {i} under title predication");
        }
        assert_eq!(q(64), 0, "the title's predication was not restored after the value copy");
        assert_eq!(q(128), u64::MAX - 1);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_write64_into_a_closed_list_is_not_recorded() {
    let g = gpu();
    unsafe {
        let out = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let l = title_list(g.dev);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        call!(l, crate::cl_ext::CL_WRITE64_BOTTOM_OF_PIPE, (u64, u64, u32, u32) -> (), out.va + 64, 1, 0, 0);
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        let q = |o: usize| core::ptr::read_volatile(out.cpu.add(o) as *const u64);
        assert_eq!(q(64), 0, "a write recorded into a closed list reached the GPU");
    }
}

const X_PREDICATION: usize = 31;
const X_SET_COMPUTE_ROOT_UAV: usize = 45;
const X_CLOSE_BUNDLE: usize = 92;
const ST_UAV: u32 = 0x8;
const ST_PREDICATION: u32 = 0x200;

unsafe fn bundle_list(dev: usize) -> *mut c_void {
    let mut a: *mut c_void = core::ptr::null_mut();
    assert!(call!(dev, 9, (u32, *const Guid, *mut *mut c_void) -> i32, 1, &IID_ALLOCATOR, &mut a) >= 0);
    let mut l: *mut c_void = core::ptr::null_mut();
    assert!(call!(dev, 12, (u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32, 0, 1, a, core::ptr::null_mut(), &IID_LIST, &mut l) >= 0);
    crate::cmdlist_wrap::wrap(l)
}

unsafe fn raw_barrier(l: *mut c_void, res: usize, before: u32, after: u32) {
    let mut b = [0u8; 32];
    b[8..16].copy_from_slice(&(res as u64).to_le_bytes());
    b[16..20].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    b[20..24].copy_from_slice(&before.to_le_bytes());
    b[24..28].copy_from_slice(&after.to_le_bytes());
    call!(l, X_BARRIER, (u32, *const u8) -> (), 1, b.as_ptr());
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_execute_indirect_bundle_honours_gpu_size_max_records_and_predication() {
    let g = gpu();
    unsafe {
        let (rs, pso) = compute_pso(g.dev, "#define RS \"RootConstants(num32BitConstants=1, b0), UAV(u0)\"\ncbuffer C : register(b0) { uint idx; };\nRWByteAddressBuffer o : register(u0);\n[RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() { uint p; o.InterlockedAdd(idx * 4, 1u, p); }\n", None);
        let out = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_UAV);
        let rb = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let bb = buffer(g.dev, HEAP_UPLOAD, 4096, 0, ST_GENERIC_READ);
        core::ptr::write_bytes(bb.cpu, 0, 4096);
        crate::placed::register(bb.res as *mut c_void, crate::placed::Placed { va: bb.cpu as u64, desc: [0u8; 0x38], init_state: ST_GENERIC_READ });
        let mut bundles = Vec::new();
        for i in 0..2u32 {
            let b = bundle_list(g.dev);
            call!(b, X_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), rs);
            call!(b, X_SET_PSO, (*mut c_void) -> (), pso);
            call!(b, X_SET_COMPUTE_ROOT_CONSTANT, (u32, u32, u32) -> (), 0, i, 0);
            call!(b, X_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 1, out.va);
            call!(b, X_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
            let mut dst = bb.cpu.add(0x100 + 16 * i as usize) as *mut c_void;
            assert_eq!(call!(b, X_CLOSE_BUNDLE, (*mut *mut c_void, u32) -> u32, &mut dst, 16), 16);
            bundles.push(b);
        }
        let szb = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_COMMON);
        let pb = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_COMMON);
        let bb2 = buffer(g.dev, HEAP_DEFAULT, 256, 0, ST_COMMON);
        let (wrs, wpso) = compute_pso(g.dev, "#define RS \"UAV(u0), UAV(u1)\"\nRWByteAddressBuffer s : register(u0);\nRWByteAddressBuffer q : register(u1);\n[RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() { s.Store4(0, uint4(16, 0, 48, 32)); q.Store2(0, uint2(0, 0)); }\n", None);
        let p = raw_list(g.dev);
        call!(p, PC_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), wrs);
        call!(p, X_SET_PSO, (*mut c_void) -> (), wpso);
        call!(p, PC_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 0, szb.va);
        call!(p, PC_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 1, pb.va);
        call!(p, X_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
        raw_barrier(p, bb2.res, ST_COMMON, ST_COPY_DEST);
        call!(p, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), bb2.res, 0, bb.res, 0x100, 32);
        raw_barrier(p, bb2.res, ST_COPY_DEST, ST_COMMON);
        assert!(call!(p, X_CLOSE, () -> i32) >= 0);
        let fp = fence(g.dev);
        submit(g.a, p, fp, 1);
        assert!(reached(fp, 1, Duration::from_secs(5)));
        let l = title_list(g.dev);
        let null = core::ptr::null_mut::<c_void>();
        let eib = |res: usize, off: u64, max: u32, sz: *mut c_void, so: u32| call!(l, crate::cl_ext::CL_EXECUTE_INDIRECT_BUNDLE, (*mut c_void, u64, u32, *mut c_void, u32, u32) -> (), res as *mut c_void, off, max, sz, so, 0);
        let mut n = 0u64;
        let mut snap = || {
            raw_barrier(l, out.res, ST_UAV, ST_COPY_SOURCE);
            call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, n * 8, out.res, 0, 8);
            raw_barrier(l, out.res, ST_COPY_SOURCE, ST_UAV);
            n += 1;
        };
        let szp = szb.res as *mut c_void;
        eib(bb.res, 0x100, 32, null, 0);
        snap();
        eib(bb.res, 0x100, 32, szp, 0);
        snap();
        eib(bb.res, 0x100, 32, szp, 4);
        snap();
        eib(bb.res, 0x100, 16, szp, 8);
        snap();
        eib(bb.res, 0x100, 0x40, szp, 12);
        snap();
        raw_barrier(l, pb.res, ST_COMMON, ST_PREDICATION);
        call!(l, X_PREDICATION, (*mut c_void, u64, u32) -> (), pb.res as *mut c_void, 0, 0);
        eib(bb.res, 0x100, 32, null, 0);
        call!(l, X_PREDICATION, (*mut c_void, u64, u32) -> (), null, 0, 0);
        snap();
        call!(l, X_PREDICATION, (*mut c_void, u64, u32) -> (), pb.res as *mut c_void, 0, 1);
        eib(bb.res, 0x100, 32, null, 0);
        call!(l, X_PREDICATION, (*mut c_void, u64, u32) -> (), null, 0, 0);
        snap();
        raw_barrier(l, pb.res, ST_PREDICATION, ST_COMMON);
        eib(bb2.res, 0, 32, null, 0);
        snap();
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        let got: Vec<(u32, u32)> = (0..8).map(|i| (rd(&rb, i * 8), rd(&rb, i * 8 + 4))).collect();
        assert_eq!(got, vec![(1, 1), (2, 1), (2, 1), (3, 1), (4, 2), (4, 2), (5, 3), (6, 4)], "per-call bundle execution counts (A, B)");
        crate::placed::forget(bb.res as *mut c_void);
        drop(bundles);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_execute_indirect_bundle_reads_the_size_at_each_execution_of_a_closed_list() {
    let g = gpu();
    unsafe {
        let (rs, pso) = compute_pso(g.dev, "#define RS \"RootConstants(num32BitConstants=1, b0), UAV(u0)\"\ncbuffer C : register(b0) { uint idx; };\nRWByteAddressBuffer o : register(u0);\n[RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() { uint p; o.InterlockedAdd(idx * 4, 1u, p); }\n", None);
        let (srs, spso) = compute_pso(g.dev, "#define RS \"RootConstants(num32BitConstants=1, b0), UAV(u0)\"\ncbuffer C : register(b0) { uint v; };\nRWByteAddressBuffer o : register(u0);\n[RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() { o.Store(0, v); }\n", None);
        let out = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_UAV);
        let rb = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let bb = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        core::ptr::write_bytes(bb.cpu, 0, 256);
        crate::placed::register(bb.res as *mut c_void, crate::placed::Placed { va: bb.cpu as u64, desc: [0u8; 0x38], init_state: ST_GENERIC_READ });
        for i in 0..2u32 {
            let b = bundle_list(g.dev);
            call!(b, X_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), rs);
            call!(b, X_SET_PSO, (*mut c_void) -> (), pso);
            call!(b, X_SET_COMPUTE_ROOT_CONSTANT, (u32, u32, u32) -> (), 0, i, 0);
            call!(b, X_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 1, out.va);
            call!(b, X_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
            let mut dst = bb.cpu.add(16 * i as usize) as *mut c_void;
            assert_eq!(call!(b, X_CLOSE_BUNDLE, (*mut *mut c_void, u32) -> u32, &mut dst, 16), 16);
        }
        let szb = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_COMMON);
        let l = title_list(g.dev);
        call!(l, crate::cl_ext::CL_EXECUTE_INDIRECT_BUNDLE, (*mut c_void, u64, u32, *mut c_void, u32, u32) -> (), bb.res as *mut c_void, 0, 32, szb.res as *mut c_void, 0, 0);
        raw_barrier(l, out.res, ST_UAV, ST_COPY_SOURCE);
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, 0, out.res, 0, 8);
        raw_barrier(l, out.res, ST_COPY_SOURCE, ST_UAV);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let mut got = Vec::new();
        for (n, size) in [16u32, 32, 0, 48].into_iter().enumerate() {
            let s = raw_list(g.dev);
            call!(s, PC_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), srs);
            call!(s, X_SET_PSO, (*mut c_void) -> (), spso);
            call!(s, 33, (u32, u32, u32) -> (), 0, size, 0);
            call!(s, PC_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 1, szb.va);
            call!(s, X_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
            assert!(call!(s, X_CLOSE, () -> i32) >= 0);
            let fs = fence(g.dev);
            submit(g.a, s, fs, 1);
            assert!(reached(fs, 1, Duration::from_secs(5)));
            let f = fence(g.dev);
            submit(g.a, l, f, 1);
            assert!(reached(f, 1, Duration::from_secs(5)), "execution {n} did not complete");
            got.push((rd(&rb, 0), rd(&rb, 4)));
        }
        assert_eq!(got, vec![(1, 0), (2, 1), (2, 1), (3, 2)], "each execution must follow the size the GPU holds at that execution");
        crate::placed::forget(bb.res as *mut c_void);
    }
}

const COUNT_HLSL: &str = "#define RS \"RootConstants(num32BitConstants=1, b0), UAV(u0)\"\ncbuffer C : register(b0) { uint idx; };\nRWByteAddressBuffer o : register(u0);\n[RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() { uint p; o.InterlockedAdd(idx * 4, 1u, p); }\n";

unsafe fn counting_bundle(dev: usize, rs: *mut c_void, pso: *mut c_void, idx: u32, out_va: u64) -> *mut c_void {
    let b = unsafe { bundle_list(dev) };
    call!(b, X_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), rs);
    call!(b, X_SET_PSO, (*mut c_void) -> (), pso);
    call!(b, X_SET_COMPUTE_ROOT_CONSTANT, (u32, u32, u32) -> (), 0, idx, 0);
    call!(b, X_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 1, out_va);
    call!(b, X_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
    b
}

unsafe fn close_bundle_into(b: *mut c_void, at: *mut u8) {
    let mut dst = at as *mut c_void;
    assert_eq!(call!(b, X_CLOSE_BUNDLE, (*mut *mut c_void, u32) -> u32, &mut dst, 16), 16);
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_execute_indirect_bundle_runs_bundles_closed_after_the_call_at_each_submit() {
    let g = gpu();
    unsafe {
        let (rs, pso) = compute_pso(g.dev, COUNT_HLSL, None);
        let out = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_UAV);
        let rb = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let bb = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        core::ptr::write_bytes(bb.cpu, 0, 256);
        crate::placed::register(bb.res as *mut c_void, crate::placed::Placed { va: bb.cpu as u64, desc: [0u8; 0x38], init_state: ST_GENERIC_READ });
        let a = counting_bundle(g.dev, rs, pso, 0, out.va);
        let l = title_list(g.dev);
        call!(l, crate::cl_ext::CL_EXECUTE_INDIRECT_BUNDLE, (*mut c_void, u64, u32, *mut c_void, u32, u32) -> (), bb.res as *mut c_void, 0, 32, core::ptr::null_mut::<c_void>(), 0, 0);
        raw_barrier(l, out.res, ST_UAV, ST_COPY_SOURCE);
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, 0, out.res, 0, 8);
        raw_barrier(l, out.res, ST_COPY_SOURCE, ST_UAV);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        close_bundle_into(a, bb.cpu);
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        let first = (rd(&rb, 0), rd(&rb, 4));
        let b = counting_bundle(g.dev, rs, pso, 1, out.va);
        close_bundle_into(b, bb.cpu.add(16));
        submit(g.a, l, f, 2);
        assert!(reached(f, 2, Duration::from_secs(5)));
        let second = (rd(&rb, 0), rd(&rb, 4));
        assert_eq!((first, second), ((1, 0), (2, 1)), "bundles closed after the call, before each ExecuteCommandLists, must be reachable");
        crate::placed::forget(bb.res as *mut c_void);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_execute_indirect_bundle_survives_a_bundle_released_before_submit() {
    let g = gpu();
    unsafe {
        let (rs, pso) = compute_pso(g.dev, COUNT_HLSL, None);
        let out = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_UAV);
        let rb = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let bb = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        core::ptr::write_bytes(bb.cpu, 0, 256);
        crate::placed::register(bb.res as *mut c_void, crate::placed::Placed { va: bb.cpu as u64, desc: [0u8; 0x38], init_state: ST_GENERIC_READ });
        let k = counting_bundle(g.dev, rs, pso, 5, out.va);
        close_bundle_into(k, bb.cpu);
        let kr = crate::cmdlist_wrap::unwrap(k) as usize;
        let l = title_list(g.dev);
        call!(l, crate::cl_ext::CL_EXECUTE_INDIRECT_BUNDLE, (*mut c_void, u64, u32, *mut c_void, u32, u32) -> (), bb.res as *mut c_void, 0, 16, core::ptr::null_mut::<c_void>(), 0, 0);
        raw_barrier(l, out.res, ST_UAV, ST_COPY_SOURCE);
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, 0, out.res, 20, 4);
        raw_barrier(l, out.res, ST_COPY_SOURCE, ST_UAV);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        assert!(crate::console_slots_cl2::live_bundles().contains(&kr));
        assert!(call!(k, 2, () -> u32) >= 1, "the recorded part must keep the PC bundle alive");
        assert!(!crate::console_slots_cl2::live_bundles().contains(&kr), "a bundle the title released is no longer a candidate");
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        assert_eq!(rd(&rb, 0), 0, "a released bundle must not run");
        submit(g.a, l, f, 2);
        assert!(reached(f, 2, Duration::from_secs(5)));
        assert_eq!(crate::indirect_bundle::held_count(kr), 0, "references of the replaced part are dropped once the GPU is done with it");
        crate::placed::forget(bb.res as *mut c_void);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_execute_bundle_state_carries_into_the_calling_list() {
    let g = gpu();
    unsafe {
        let (rs, pso) = compute_pso(g.dev, COUNT_HLSL, None);
        let (_, store) = compute_pso(g.dev, "#define RS \"RootConstants(num32BitConstants=1, b0), UAV(u0)\"\ncbuffer C : register(b0) { uint v; };\nRWByteAddressBuffer o : register(u0);\n[RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() { o.Store(0, v); }\n", Some(rs));
        let out = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_UAV);
        let pout = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_UAV);
        let rb = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let bb = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        let k = counting_bundle(g.dev, rs, pso, 3, out.va);
        close_bundle_into(k, bb.cpu);
        let l = title_list(g.dev);
        call!(l, X_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), rs);
        call!(l, X_SET_PSO, (*mut c_void) -> (), store);
        call!(l, X_SET_COMPUTE_ROOT_CONSTANT, (u32, u32, u32) -> (), 0, 7, 0);
        call!(l, X_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 1, pout.va);
        call!(l, 27, (*mut c_void) -> (), k);
        call!(l, X_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
        raw_barrier(l, pout.res, ST_UAV, ST_COPY_SOURCE);
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, 0, pout.res, 0, 4);
        raw_barrier(l, out.res, ST_UAV, ST_COPY_SOURCE);
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, 4, out.res, 12, 4);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        assert_eq!((rd(&rb, 0), rd(&rb, 4)), (0, 2), "after ExecuteBundle the list must draw with the PSO, root signature, constants and UAV the bundle set");
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_execute_indirect_bundle_has_no_candidate_budget() {
    let g = gpu();
    unsafe {
        const N: u32 = 96;
        let (rs, pso) = compute_pso(g.dev, COUNT_HLSL, None);
        let out = buffer(g.dev, HEAP_DEFAULT, 512, FLAG_UAV, ST_UAV);
        let rb = buffer(g.dev, HEAP_READBACK, 512, 0, ST_COPY_DEST);
        let bb = buffer(g.dev, HEAP_UPLOAD, 4096, 0, ST_GENERIC_READ);
        core::ptr::write_bytes(bb.cpu, 0, 4096);
        crate::placed::register(bb.res as *mut c_void, crate::placed::Placed { va: bb.cpu as u64, desc: [0u8; 0x38], init_state: ST_GENERIC_READ });
        for i in 0..N {
            let b = counting_bundle(g.dev, rs, pso, i, out.va);
            close_bundle_into(b, bb.cpu.add(16 * i as usize));
        }
        let live = crate::console_slots_cl2::live_bundles().len() as u64;
        assert!(live * N as u64 > 4096, "{live} live bundles x {N} slots must exceed the old 4096-candidate budget");
        let l = title_list(g.dev);
        call!(l, crate::cl_ext::CL_EXECUTE_INDIRECT_BUNDLE, (*mut c_void, u64, u32, *mut c_void, u32, u32) -> (), bb.res as *mut c_void, 0, 16 * N, core::ptr::null_mut::<c_void>(), 0, 0);
        raw_barrier(l, out.res, ST_UAV, ST_COPY_SOURCE);
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, 0, out.res, 0, 4 * N as u64);
        raw_barrier(l, out.res, ST_COPY_SOURCE, ST_UAV);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(20)));
        let got: Vec<u32> = (0..N as usize).map(|i| rd(&rb, 4 * i)).collect();
        assert_eq!(got, vec![1; N as usize], "every record slot must run its bundle exactly once");
        crate::placed::forget(bb.res as *mut c_void);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_execute_indirect_bundle_carries_executed_bundle_state_and_nothing_from_skipped_ones() {
    let g = gpu();
    unsafe {
        let (rs, pso) = compute_pso(g.dev, COUNT_HLSL, None);
        let (_, store) = compute_pso(g.dev, "#define RS \"RootConstants(num32BitConstants=1, b0), UAV(u0)\"\ncbuffer C : register(b0) { uint v; };\nRWByteAddressBuffer o : register(u0);\n[RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() { o.Store(0, v); }\n", Some(rs));
        let out = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_UAV);
        let pout = buffer(g.dev, HEAP_DEFAULT, 256, FLAG_UAV, ST_UAV);
        let rb = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let bb = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        core::ptr::write_bytes(bb.cpu, 0, 256);
        crate::placed::register(bb.res as *mut c_void, crate::placed::Placed { va: bb.cpu as u64, desc: [0u8; 0x38], init_state: ST_GENERIC_READ });
        let k = counting_bundle(g.dev, rs, pso, 3, out.va);
        close_bundle_into(k, bb.cpu);
        let l = title_list(g.dev);
        let eib = |off: u64| call!(l, crate::cl_ext::CL_EXECUTE_INDIRECT_BUNDLE, (*mut c_void, u64, u32, *mut c_void, u32, u32) -> (), bb.res as *mut c_void, off, 16, core::ptr::null_mut::<c_void>(), 0, 0);
        let grab = |dst: u64| {
            raw_barrier(l, pout.res, ST_UAV, ST_COPY_SOURCE);
            call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, dst, pout.res, 0, 4);
            raw_barrier(l, pout.res, ST_COPY_SOURCE, ST_UAV);
            raw_barrier(l, out.res, ST_UAV, ST_COPY_SOURCE);
            call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, dst + 4, out.res, 12, 4);
            raw_barrier(l, out.res, ST_COPY_SOURCE, ST_UAV);
        };
        call!(l, X_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), rs);
        call!(l, X_SET_PSO, (*mut c_void) -> (), store);
        call!(l, X_SET_COMPUTE_ROOT_CONSTANT, (u32, u32, u32) -> (), 0, 7, 0);
        call!(l, X_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 1, pout.va);
        eib(0x80);
        call!(l, X_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
        grab(0);
        call!(l, X_SET_COMPUTE_ROOT_CONSTANT, (u32, u32, u32) -> (), 0, 9, 0);
        eib(0);
        call!(l, X_DISPATCH, (u32, u32, u32) -> (), 1, 1, 1);
        grab(8);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        let got = (rd(&rb, 0), rd(&rb, 4), rd(&rb, 8), rd(&rb, 12));
        assert_eq!(got, (7, 0, 7, 2), "skipped bundle: the title state and no bundle work; executed bundle: its work, then the list draws with the PSO, root signature, constants and UAV the bundle set");
        crate::placed::forget(bb.res as *mut c_void);
    }
}

const IID_QUERY_HEAP: Guid = Guid { d1: 0x0d96_58ae, d2: 0xed45, d3: 0x469e, d4: [0xa6, 0x1d, 0x97, 0x0e, 0xc5, 0x83, 0xca, 0xb4] };
const X_END_QUERY: usize = 29;
const X_RESOLVE_QUERY_DATA: usize = 30;
const CONSOLE_HZ: u64 = 99_750_000;

unsafe fn title_clock(q: usize) -> (u64, u64) {
    let (mut g, mut c) = (0u64, 0u64);
    assert_eq!(call!(q, 17, (*mut u64, *mut u64) -> i32, &mut g, &mut c), 0);
    (g, c)
}

unsafe fn timestamp_heap(dev: usize, n: u32) -> *mut c_void {
    let desc: [u32; 3] = [1, n, 0];
    let mut h: *mut c_void = core::ptr::null_mut();
    assert!(call!(dev, 39, (*const u32, *const Guid, *mut *mut c_void) -> i32, desc.as_ptr(), &IID_QUERY_HEAP, &mut h) >= 0);
    h
}

fn qpf() -> u64 {
    let mut f = 0i64;
    unsafe { QueryPerformanceFrequency(&mut f) };
    f as u64
}

fn on_qpc_base(v: u64, qpc: u64, tol_ms: u64) -> bool {
    let want = (qpc as u128 * CONSOLE_HZ as u128 / qpf() as u128) as i128;
    (v as i128 - want).unsigned_abs() <= (CONSOLE_HZ / 1000 * tol_ms) as u128
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_queue_timestamp_frequency_and_calibration_follow_the_console_99_75_mhz_clock() {
    let g = gpu();
    unsafe {
        let mut f = 0u64;
        assert_eq!(call!(g.a, 16, (*mut u64) -> i32, &mut f), 0);
        assert_eq!(f, CONSOLE_HZ);
        let host: crate::timestamp::FreqFn = core::mem::transmute(crate::present::real_queue_slot(16));
        let mut hf = 0u64;
        assert!(host(g.a as *mut c_void, &mut hf) >= 0 && hf != 0);
        let (g0, c0) = title_clock(g.a);
        assert!(on_qpc_base(g0, c0, 5), "{}: calibration gpu {g0} is not QPC {c0} on the 99.75 MHz base", if g.warp { "WARP" } else { "hardware" });
        let h = xgs_kmd_emu::open();
        let (mut ig, mut ic) = (0u64, 0u64);
        let mut req = [0u8; 0x18];
        req[8..16].copy_from_slice(&(&mut ig as *mut u64 as u64).to_le_bytes());
        req[16..24].copy_from_slice(&(&mut ic as *mut u64 as u64).to_le_bytes());
        let mut ret = 0u32;
        assert!(xgs_kmd_emu::device_io_control(h, 0x7900_c213, req.as_ptr() as *const c_void, 0x18, core::ptr::null_mut(), 0, &mut ret));
        xgs_kmd_emu::close(h);
        std::thread::sleep(Duration::from_millis(200));
        let (g1, c1) = title_clock(g.a);
        assert!(g0 <= ig && ig <= g1 && c0 <= ic && ic <= c1, "IOCTL 0x7900c213 ({ig}, {ic}) not between slot 17 samples ({g0}, {c0}) and ({g1}, {c1})");
        let dg = (g1 - g0) as f64 / CONSOLE_HZ as f64;
        let dc = (c1 - c0) as f64 / qpf() as f64;
        assert!((dg - dc).abs() < 0.002, "GPU clock advanced {dg} s while QPC advanced {dc} s");
        let (mut tg, mut tc) = (0u64, 0u64);
        assert_eq!(call!(g.b, 17, (*mut u64, *mut u64) -> i32, &mut tg, &mut tc), 0);
        assert!(on_qpc_base(tg, tc, 5));
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_end_query_timestamp_resolve_is_on_the_console_base_bracketed_by_calibration_and_keeps_predication() {
    let g = gpu();
    unsafe {
        let heap = timestamp_heap(g.dev, 128);
        let out = buffer(g.dev, HEAP_READBACK, 4096, 0, ST_COPY_DEST);
        let pred = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        core::ptr::write_volatile(pred.cpu as *mut u64, 0);
        let src = buffer(g.dev, HEAP_UPLOAD, 256, 0, ST_GENERIC_READ);
        core::ptr::write_volatile(src.cpu as *mut u64, 0x5555_aaaa);
        let before = crate::ts99::converted();
        let l = title_list(g.dev);
        for i in 0..100u32 {
            call!(l, X_END_QUERY, (*mut c_void, u32, u32) -> (), heap, 2, i);
        }
        call!(l, X_PREDICATION, (*mut c_void, u64, u32) -> (), pred.res as *mut c_void, 0, 0);
        call!(l, X_RESOLVE_QUERY_DATA, (*mut c_void, u32, u32, u32, *mut c_void, u64) -> (), heap, 2, 3, 90, out.res as *mut c_void, 16);
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), out.res, 2048, src.res, 0, 8);
        call!(l, X_PREDICATION, (*mut c_void, u64, u32) -> (), core::ptr::null_mut::<c_void>(), 0, 0);
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), out.res, 2056, src.res, 0, 8);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let f = fence(g.dev);
        let (gb, _) = title_clock(g.a);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        let (ga, _) = title_clock(g.a);
        assert!(crate::ts99::converted() >= before + 90);
        let q = |o: usize| core::ptr::read_volatile(out.cpu.add(o) as *const u64);
        let who = if g.warp { "WARP" } else { "hardware" };
        assert_eq!((q(0), q(8)), (0, 0), "resolve wrote before its destination offset");
        let mut last = 0;
        for i in 0..90 {
            let v = q(16 + 8 * i);
            assert!(v + 2 >= gb && v <= ga + 2, "{who}: timestamp {i} = {v} outside the GetClockCalibration bracket [{gb}, {ga}]");
            assert!(v >= last, "{who}: timestamps not monotonic at {i}");
            last = v;
        }
        assert_eq!(q(16 + 8 * 90), 0, "resolve wrote past its count");
        assert_eq!(q(2048), 0, "the title predication was not restored after the conversion");
        assert_eq!(q(2056), 0x5555_aaaa);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_gpu_timestamp_writes_are_on_the_console_base_and_advance_with_wall_time() {
    let g = gpu();
    unsafe {
        let out = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        let bop = |l: *mut c_void, va: u64| call!(l, crate::cl_ext::CL_WRITE64_BOTTOM_OF_PIPE, (u64, u64, u32, u32) -> (), va, 0, 0, crate::cl_ext::BOP_FLAG_GPU_TIMESTAMP);
        let top = |l: *mut c_void, va: u64| call!(l, crate::console_slots::CL_WRITE64_TOP, (u64, u64, u32) -> (), va, 0, crate::console_slots::WRITE_TOP_FLAG_GPU_TIMESTAMP);
        let l = title_list(g.dev);
        top(l, out.va);
        bop(l, out.va + 8);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0);
        let l2 = title_list(g.dev);
        bop(l2, out.va + 16);
        top(l2, out.va + 24);
        assert!(call!(l2, X_CLOSE, () -> i32) >= 0);
        let f = fence(g.dev);
        let (g0, c0) = title_clock(g.a);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        let (g1, _) = title_clock(g.a);
        std::thread::sleep(Duration::from_millis(200));
        let (g2, _) = title_clock(g.a);
        submit(g.a, l2, f, 2);
        assert!(reached(f, 2, Duration::from_secs(5)));
        let (g3, c3) = title_clock(g.a);
        let q = |o: usize| core::ptr::read_volatile(out.cpu.add(o) as *const u64);
        let v = [q(0), q(8), q(16), q(24)];
        let who = if g.warp { "WARP" } else { "hardware" };
        assert!(v[0] + 2 >= g0 && v[1] <= g1 + 2 && v[0] <= v[1], "{who}: list 1 {v:?} outside [{g0}, {g1}]");
        assert!(v[2] + 2 >= g2 && v[3] <= g3 + 2 && v[2] <= v[3], "{who}: list 2 {v:?} outside [{g2}, {g3}]");
        let secs = (v[2] - v[1]) as f64 / CONSOLE_HZ as f64;
        let wall = (c3 - c0) as f64 / qpf() as f64;
        assert!(secs >= 0.19 && secs <= wall + 0.002, "{who}: timestamps advanced {secs} s across a 200 ms sleep (wall {wall} s)");
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_dma_list_timestamp_is_converted_after_the_copy_queue_submission() {
    let g = gpu();
    unsafe {
        let desc: [u32; 4] = [3, 0, 0, 0];
        let mut cq: *mut c_void = core::ptr::null_mut();
        assert!(call!(g.dev, 8, (*const u32, *const Guid, *mut *mut c_void) -> i32, desc.as_ptr(), &IID_QUEUE, &mut cq) >= 0);
        crate::present::apply_queue_vtable(cq);
        let mut a: *mut c_void = core::ptr::null_mut();
        assert!(call!(g.dev, 9, (u32, *const Guid, *mut *mut c_void) -> i32, 3, &IID_ALLOCATOR, &mut a) >= 0);
        let mut rl: *mut c_void = core::ptr::null_mut();
        assert!(call!(g.dev, 12, (u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32, 0, 3, a, core::ptr::null_mut(), &IID_LIST, &mut rl) >= 0);
        let l = crate::dma_list::wrap(rl);
        let out = buffer(g.dev, HEAP_READBACK, 256, 0, ST_COPY_DEST);
        call!(l, crate::dma_list::DMA_SLOT_TIMESTAMP, (u64, u32, u32, u32) -> (), out.va + 8, 0, 0, 0);
        call!(l, crate::dma_list::DMA_SLOT_TIMESTAMP, (u64, u32, u32, u32) -> (), out.va + 16, 0, 0, 0);
        assert!(call!(rl, 9, () -> i32) >= 0);
        let f = fence(g.dev);
        let (g0, _) = title_clock(g.a);
        submit(cq as usize, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)));
        let (g1, _) = title_clock(g.a);
        let q = |o: usize| core::ptr::read_volatile(out.cpu.add(o) as *const u64);
        let who = if g.warp { "WARP" } else { "hardware" };
        let (v0, v1) = (q(8), q(16));
        assert!(v0 != 0, "{who}: DMA timestamp not written");
        assert!(v0 + 2 >= g0 && v1 <= g1 + 2 && v0 <= v1, "{who}: DMA timestamps {v0} {v1} outside [{g0}, {g1}]");
        assert_eq!((q(0), q(24)), (0, 0));
        assert_eq!(crate::ts99::pending_of(&[rl]).len(), 2);
        call!(l, 10, (*mut c_void, *mut c_void) -> i32, a, core::ptr::null_mut());
        assert!(crate::ts99::pending_of(&[rl]).is_empty(), "Reset through the DMA list did not drop its pending timestamps");
    }
}

#[path = "cl_fw_hw.rs"]
mod cl_fw_hw;

#[path = "device_place_hw.rs"]
mod device_place_hw;

const IID_DESCRIPTOR_HEAP: Guid = Guid { d1: 0x8efb_471d, d2: 0x616c, d3: 0x4f49, d4: [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51] };
const ST_NON_PIXEL_SRV: u32 = 0x40;
const PC_SET_DESCRIPTOR_HEAPS: usize = 28;
const PC_SET_COMPUTE_ROOT_TABLE: usize = 31;
const PC_SET_COMPUTE_ROOT_SRV: usize = 39;

fn transition(res: usize, before: u32, after: u32) -> [u64; 4] {
    [0, res as u64, 0xffff_ffff | (before as u64) << 32, after as u64]
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_rgb9e5_typed_uav_texture_is_substituted() {
    use crate::uav_subst::{f16_to_f32, f32_to_f16, rgb9e5_decode, rgb9e5_encode, rgb9e5_step};
    const W: usize = 64;
    const H: usize = 16;
    let g = gpu();
    unsafe {
        let real = g.dev as *mut c_void;
        let support = crate::uav_subst::host_support1(real, 67).expect("CheckFeatureSupport(FORMAT_SUPPORT, R9G9B9E5)");
        if support & crate::uav_subst::SUPPORT1_TYPED_UAV != 0 {
            eprintln!("skipped: this adapter ({}) binds R9G9B9E5 as a typed UAV natively (Support1={support:#x})", if g.warp { "WARP" } else { "hardware" });
            return;
        }
        eprintln!("R9G9B9E5 Support1={support:#x}: no typed UAV on this adapter");
        let heap: [u32; 5] = [HEAP_DEFAULT, 0, 0, 1, 1];
        let mut d = [0u8; 0x38];
        d[0..4].copy_from_slice(&3u32.to_le_bytes());
        d[0x10..0x18].copy_from_slice(&(W as u64).to_le_bytes());
        d[0x18..0x1c].copy_from_slice(&(H as u32).to_le_bytes());
        d[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
        d[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
        d[0x20..0x24].copy_from_slice(&67u32.to_le_bytes());
        d[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
        d[0x30..0x34].copy_from_slice(&FLAG_UAV.to_le_bytes());
        let mut r: *mut c_void = core::ptr::null_mut();
        let hr = call!(real, 27, (*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
            heap.as_ptr(), 0, d.as_ptr(), ST_UAV, core::ptr::null(), &IID_RESOURCE, &mut r);
        assert_eq!(hr, 0x8007_0057u32 as i32, "the host accepted R9G9B9E5 + ALLOW_UAV");
        let dev = crate::device_wrap::wrap_device(real);
        let mut tex: *mut c_void = core::ptr::null_mut();
        let hr = call!(dev, crate::console_slots_dev2::SLOT_COMMITTED_X, (*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
            heap.as_ptr(), 0, d.as_ptr(), ST_UAV, core::ptr::null(), &IID_RESOURCE, &mut tex);
        assert!(hr >= 0 && !tex.is_null(), "d3d12_xs CreateCommittedResourceX hr={hr:#010x}");
        let mut got = [0u8; 0x38];
        call!(tex, 10, (*mut u8) -> *mut u8, got.as_mut_ptr());
        assert_eq!(u32::from_le_bytes(got[0x20..0x24].try_into().unwrap()), 10, "host texture is not RGBA16F");
        assert_eq!(crate::uav_subst::lookup(tex), Some(crate::uav_subst::TABLE[0]));

        let hd: [u32; 4] = [0, 4, 1, 0];
        let mut dh: *mut c_void = core::ptr::null_mut();
        assert!(call!(real, 14, (*const u32, *const Guid, *mut *mut c_void) -> i32, hd.as_ptr(), &IID_DESCRIPTOR_HEAP, &mut dh) >= 0);
        let mut cpu0 = 0usize;
        call!(dh, 9, (*mut usize) -> *mut usize, &mut cpu0);
        let mut gpu0 = 0u64;
        call!(dh, 10, (*mut u64) -> *mut u64, &mut gpu0);
        let inc = call!(real, 15, (u32) -> u32, 0) as usize;
        let mut uav = [0u8; 40];
        uav[0..4].copy_from_slice(&67u32.to_le_bytes());
        uav[4..8].copy_from_slice(&4u32.to_le_bytes());
        call!(dev, 19, (*mut c_void, *mut c_void, *const u8, usize) -> (), tex, core::ptr::null_mut(), uav.as_ptr(), cpu0);
        let mut srv = [0u8; 40];
        srv[0..4].copy_from_slice(&67u32.to_le_bytes());
        srv[4..8].copy_from_slice(&4u32.to_le_bytes());
        srv[8..12].copy_from_slice(&0x1688u32.to_le_bytes());
        srv[20..24].copy_from_slice(&1u32.to_le_bytes());
        call!(dev, 18, (*mut c_void, *const u8, usize) -> (), tex, srv.as_ptr(), cpu0 + inc);
        let mut null_uav = [0u8; 40];
        null_uav[0..4].copy_from_slice(&10u32.to_le_bytes());
        null_uav[4..8].copy_from_slice(&4u32.to_le_bytes());
        call!(real, 19, (*mut c_void, *mut c_void, *const u8, usize) -> (), core::ptr::null_mut(), core::ptr::null_mut(), null_uav.as_ptr(), cpu0 + 2 * inc);
        let mut null_srv = srv;
        null_srv[0..4].copy_from_slice(&10u32.to_le_bytes());
        call!(real, 18, (*mut c_void, *const u8, usize) -> (), core::ptr::null_mut(), null_srv.as_ptr(), cpu0 + 3 * inc);

        let n = W * H;
        let mut seed = 0x1234_5678u32;
        let mut next = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); seed };
        let mut input = vec![[0f32; 4]; n];
        for (i, t) in input.iter_mut().enumerate() {
            let rgb = if i < n / 2 {
                rgb9e5_decode(next())
            } else {
                [0; 3].map(|_| {
                    let u = next();
                    (2f32.powi((u >> 24) as i32 % 41 - 25) * (1.0 + (u & 0xffff) as f32 / 65536.0)).min(65408.0)
                })
            };
            *t = [rgb[0], rgb[1], rgb[2], 0.25];
        }
        input[n / 2] = [0.0, 0.0, 0.0, 0.25];
        input[n / 2 + 1] = [65408.0, 1.0, 3.0e-7, 0.25];
        let src = buffer(g.dev, HEAP_UPLOAD, (n * 16) as u64, 0, ST_GENERIC_READ);
        core::ptr::copy_nonoverlapping(input.as_ptr() as *const u8, src.cpu, n * 16);
        let out = buffer(g.dev, HEAP_DEFAULT, (n * 16) as u64, FLAG_UAV, ST_UAV);
        let rb = buffer(g.dev, HEAP_READBACK, (n * 16) as u64, 0, ST_COPY_DEST);

        let rs_decl = "#define RS \"DescriptorTable(UAV(u0)), DescriptorTable(SRV(t0)), SRV(t1), UAV(u1)\"\n\
            RWTexture2D<float4> tex_out : register(u0);\nTexture2D<float4> tex_in : register(t0);\n\
            ByteAddressBuffer inp : register(t1);\nRWByteAddressBuffer res : register(u1);\n";
        let write = format!("{rs_decl}[RootSignature(RS)] [numthreads(8, 8, 1)]\nvoid main(uint3 id : SV_DispatchThreadID) {{ uint i = id.y * {W} + id.x; tex_out[id.xy] = asfloat(inp.Load4(i * 16)); }}\n");
        let read = format!("{rs_decl}[RootSignature(RS)] [numthreads(8, 8, 1)]\nvoid main(uint3 id : SV_DispatchThreadID) {{ uint i = id.y * {W} + id.x; res.Store4(i * 16, asuint(tex_in.Load(int3(id.xy, 0)))); }}\n");
        let (rs, pso_w) = compute_pso(g.dev, &write, None);
        let (_, pso_r) = compute_pso(g.dev, &read, Some(rs));
        let l = raw_list(g.dev);
        let heaps = [dh];
        call!(l, PC_SET_DESCRIPTOR_HEAPS, (u32, *const *mut c_void) -> (), 1, heaps.as_ptr());
        call!(l, PC_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), rs);
        call!(l, PC_SET_COMPUTE_ROOT_TABLE, (u32, u64) -> (), 0, gpu0);
        call!(l, PC_SET_COMPUTE_ROOT_TABLE, (u32, u64) -> (), 1, gpu0 + 3 * inc as u64);
        call!(l, PC_SET_COMPUTE_ROOT_SRV, (u32, u64) -> (), 2, src.va);
        call!(l, PC_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 3, out.va);
        call!(l, X_SET_PSO, (*mut c_void) -> (), pso_w);
        call!(l, X_DISPATCH, (u32, u32, u32) -> (), (W / 8) as u32, (H / 8) as u32, 1);
        let b = transition(tex as usize, ST_UAV, ST_NON_PIXEL_SRV);
        call!(l, X_BARRIER, (u32, *const u64) -> (), 1, b.as_ptr());
        call!(l, PC_SET_COMPUTE_ROOT_TABLE, (u32, u64) -> (), 0, gpu0 + 2 * inc as u64);
        call!(l, PC_SET_COMPUTE_ROOT_TABLE, (u32, u64) -> (), 1, gpu0 + inc as u64);
        call!(l, X_SET_PSO, (*mut c_void) -> (), pso_r);
        call!(l, X_DISPATCH, (u32, u32, u32) -> (), (W / 8) as u32, (H / 8) as u32, 1);
        let b = transition(out.res, ST_UAV, ST_COPY_SOURCE);
        call!(l, X_BARRIER, (u32, *const u64) -> (), 1, b.as_ptr());
        call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, 0, out.res, 0, (n * 16) as u64);
        assert!(call!(l, X_CLOSE, () -> i32) >= 0, "compute list failed to close");
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)), "compute work did not finish");
        let back = core::slice::from_raw_parts(rb.cpu as *const [f32; 4], n);
        for (i, (want, got)) in input.iter().zip(back).enumerate() {
            let rgb = [want[0], want[1], want[2]];
            assert_eq!(got[3], 1.0, "texel {i}: SRV alpha of an RGB9E5 view must read 1.0, got {}", got[3]);
            if i < n / 2 {
                assert_eq!(&got[..3], &rgb, "texel {i}: RGB9E5 value not stored exactly");
                continue;
            }
            let q = rgb9e5_decode(rgb9e5_encode(rgb));
            let step = rgb9e5_step(rgb);
            for c in 0..3 {
                let h = f32_to_f16(rgb[c]);
                let (lo, hi) = (f16_to_f32(h.saturating_sub(1)).min(f16_to_f32(h)), f16_to_f32(h + 1));
                assert!(got[c] >= lo && got[c] <= hi, "texel {i}.{c}: {} is not an f16 neighbour of {}", got[c], rgb[c]);
                assert!((got[c] - q[c]).abs() <= step, "texel {i}.{c}: {} vs RGB9E5 {} (quantum {step})", got[c], q[c]);
            }
        }

        let pitch = (W * 8).next_multiple_of(256);
        let refused = buffer(g.dev, HEAP_READBACK, (pitch * H) as u64, 0, ST_COPY_DEST);
        let ok = buffer(g.dev, HEAP_READBACK, (pitch * H) as u64, 0, ST_COPY_DEST);
        core::ptr::write_bytes(refused.cpu, 0xcd, pitch * H);
        let footprint = |res: usize, fmt: u32, row: usize| {
            let mut loc = [0u8; 0x30];
            loc[0..8].copy_from_slice(&(res as u64).to_le_bytes());
            loc[8..12].copy_from_slice(&1u32.to_le_bytes());
            loc[0x18..0x1c].copy_from_slice(&fmt.to_le_bytes());
            loc[0x1c..0x20].copy_from_slice(&(W as u32).to_le_bytes());
            loc[0x20..0x24].copy_from_slice(&(H as u32).to_le_bytes());
            loc[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
            loc[0x28..0x2c].copy_from_slice(&(row as u32).to_le_bytes());
            loc
        };
        let mut sub = [0u8; 0x30];
        sub[0..8].copy_from_slice(&(tex as u64).to_le_bytes());
        let x = title_list(g.dev);
        let b = transition(tex as usize, ST_NON_PIXEL_SRV, ST_COPY_SOURCE);
        call!(x, X_BARRIER, (u32, *const u64) -> (), 1, b.as_ptr());
        let console = footprint(refused.res, 67, (W * 4).next_multiple_of(256));
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), console.as_ptr(), 0, 0, 0, sub.as_ptr(), core::ptr::null());
        let host = footprint(ok.res, 10, pitch);
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), host.as_ptr(), 0, 0, 0, sub.as_ptr(), core::ptr::null());
        assert!(call!(x, X_CLOSE, () -> i32) >= 0, "copy list failed to close");
        let f = fence(g.dev);
        submit(g.a, x, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)), "copy work did not finish");
        assert!(core::slice::from_raw_parts(refused.cpu, pitch * H).iter().all(|&b| b == 0xcd), "the R9G9B9E5-footprint copy was not refused");
        for (i, got) in back.iter().enumerate() {
            let at = (i / W) * pitch + (i % W) * 8;
            let h: Vec<u16> = (0..4).map(|c| core::ptr::read_unaligned(ok.cpu.add(at + 2 * c) as *const u16)).collect();
            assert_eq!(&h[..3], &[f32_to_f16(got[0]), f32_to_f16(got[1]), f32_to_f16(got[2])], "texel {i}: copied texel differs from the SRV read");
            assert_eq!(f16_to_f32(h[3]), 0.25, "texel {i}: stored alpha");
        }
        eprintln!("{n} texels verified on {}: {} exact RGB9E5, {} within one RGB9E5 quantum", if g.warp { "WARP" } else { "hardware" }, n / 2, n - n / 2);
    }
}
