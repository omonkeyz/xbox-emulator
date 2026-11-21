use compat_common::{record, E_NOTIMPL};
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

const M: &str = "dxcompiler_x";
const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;

extern "system" {
    fn LoadLibraryExW(p: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
    fn GetModuleHandleExW(flags: u32, addr: *const c_void, out: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(h: *mut c_void, buf: *mut u16, n: u32) -> u32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn own_dir() -> Option<String> {
    let mut h: *mut c_void = core::ptr::null_mut();
    if unsafe { GetModuleHandleExW(0x4 | 0x2, own_dir as *const c_void, &mut h) } == 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(h, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let p = String::from_utf16_lossy(&buf[..n]);
    p.rfind('\\').map(|i| p[..i].to_string())
}

pub fn candidates() -> Vec<String> {
    let mut v = Vec::new();
    if let Some(d) = own_dir() {
        v.push(format!("{d}\\dxcompiler.dll"));
    }
    if let Ok(d) = std::env::var("TOMBOX_DXC_DIR") {
        v.push(format!("{d}\\dxcompiler.dll"));
    }
    v.extend(compat_common::dxc::sdk_dirs().into_iter().map(|dir| dir.join("dxcompiler.dll").display().to_string()));
    v
}

fn host_dxc(name: &[u8]) -> usize {
    static CELL: AtomicUsize = AtomicUsize::new(0);
    let mut h = CELL.load(Ordering::Acquire);
    if h == 0 {
        let mut found = 0usize;
        let mut from = String::new();
        for c in candidates() {
            if !std::path::Path::new(&c).is_file() {
                continue;
            }
            let l = unsafe { LoadLibraryExW(wide(&c).as_ptr(), core::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) } as usize;
            if l != 0 {
                found = l;
                from = c;
                break;
            }
        }
        h = if found == 0 { 1 } else { found };
        CELL.store(h, Ordering::Release);
        record(M, "init", &if found == 0 { "no dxcompiler.dll found next to dxcompiler_x, in TOMBOX_DXC_DIR or the Windows SDK".to_string() } else { format!("DXC {from} = {found:#x}") });
    }
    if h == 1 {
        return 0;
    }
    unsafe { GetProcAddress(h as *mut c_void, name.as_ptr()) as usize }
}

fn clear(ppv: *mut *mut c_void) {
    if !ppv.is_null() {
        unsafe { *ppv = core::ptr::null_mut() };
    }
}

#[no_mangle]
pub extern "system" fn DxcCreateInstance(rclsid: *const c_void, riid: *const c_void, ppv: *mut *mut c_void) -> i32 {
    clear(ppv);
    let p = host_dxc(b"DxcCreateInstance\0");
    if p == 0 {
        record(M, "DxcCreateInstance", "no DXC available -> E_NOTIMPL");
        return E_NOTIMPL;
    }
    let f: unsafe extern "system" fn(*const c_void, *const c_void, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(p) };
    unsafe { f(rclsid, riid, ppv) }
}

#[no_mangle]
pub extern "system" fn DxcCreateInstance2(malloc: *mut c_void, rclsid: *const c_void, riid: *const c_void, ppv: *mut *mut c_void) -> i32 {
    clear(ppv);
    let p = host_dxc(b"DxcCreateInstance2\0");
    if p == 0 {
        record(M, "DxcCreateInstance2", "no DXC available -> E_NOTIMPL");
        return E_NOTIMPL;
    }
    let f: unsafe extern "system" fn(*mut c_void, *const c_void, *const c_void, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(p) };
    unsafe { f(malloc, rclsid, riid, ppv) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPORTS: &str = include_str!("../exports.txt");

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const CLSID_DXC_COMPILER: Guid = Guid(0x73e2_2d93, 0xe6ce, 0x47f3, [0xb5, 0xbf, 0xf0, 0x66, 0x4f, 0x39, 0xc1, 0xb0]);
    const IID_IDXC_COMPILER3: Guid = Guid(0x228b_4687, 0x5a6a, 0x4730, [0x90, 0x0c, 0x97, 0x02, 0xb2, 0x20, 0x3f, 0x54]);
    const IID_IDXC_RESULT: Guid = Guid(0x5834_6cda, 0xdde7, 0x4497, [0x94, 0x61, 0x6f, 0x87, 0xaf, 0x5e, 0x06, 0x59]);

    #[repr(C)]
    struct DxcBuffer {
        ptr: *const c_void,
        size: usize,
        encoding: u32,
    }

    unsafe fn slot(o: *mut c_void, i: usize) -> usize {
        *(*(o as *const *const usize)).add(i)
    }

    unsafe fn compile(src: &str, profile: &str) -> (i32, Vec<u8>) {
        let mut c: *mut c_void = core::ptr::null_mut();
        let hr = DxcCreateInstance(&CLSID_DXC_COMPILER as *const _ as *const c_void, &IID_IDXC_COMPILER3 as *const _ as *const c_void, &mut c);
        assert_eq!(hr, 0, "DxcCreateInstance(CLSID_DxcCompiler) failed {hr:#x}");
        let args: Vec<Vec<u16>> = ["-E", "main", "-T", profile].iter().map(|s| wide(s)).collect();
        let argp: Vec<*const u16> = args.iter().map(|a| a.as_ptr()).collect();
        let buf = DxcBuffer { ptr: src.as_ptr() as *const c_void, size: src.len(), encoding: 65001 };
        let compile: unsafe extern "system" fn(*mut c_void, *const DxcBuffer, *const *const u16, u32, *mut c_void, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(slot(c, 3));
        let mut r: *mut c_void = core::ptr::null_mut();
        assert_eq!(compile(c, &buf, argp.as_ptr(), argp.len() as u32, core::ptr::null_mut(), &IID_IDXC_RESULT, &mut r), 0);
        let status: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 = core::mem::transmute(slot(r, 3));
        let mut st = 0i32;
        status(r, &mut st);
        let get: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 = core::mem::transmute(slot(r, 4));
        let mut blob: *mut c_void = core::ptr::null_mut();
        get(r, &mut blob);
        let mut out = Vec::new();
        if !blob.is_null() {
            let ptr: unsafe extern "system" fn(*mut c_void) -> *const u8 = core::mem::transmute(slot(blob, 3));
            let len: unsafe extern "system" fn(*mut c_void) -> usize = core::mem::transmute(slot(blob, 4));
            let (p, n) = (ptr(blob), len(blob));
            if !p.is_null() && n != 0 {
                out = core::slice::from_raw_parts(p, n).to_vec();
            }
            let rel: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(slot(blob, 2));
            rel(blob);
        }
        let rel: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(slot(r, 2));
        rel(r);
        rel(c);
        (st, out)
    }

    #[test]
    fn export_table_matches_reference_dll() {
        xone_exports::check_crate(env!("CARGO_MANIFEST_DIR"), "dxcompiler_x.dll", EXPORTS);
    }

    #[test]
    fn built_dll_has_reference_ordinals() {
        xone_exports::check_built(env!("CARGO_MANIFEST_DIR"), "dxcompiler_x.dll", EXPORTS);
    }

    #[test]
    fn compiles_sm60_shaders_to_signed_dxil() {
        let ps = "Texture2D t : register(t0); SamplerState s : register(s0); float4 main(float2 uv : TEXCOORD0) : SV_Target { return t.Sample(s, uv) * 0.5; }";
        let (st, blob) = unsafe { compile(ps, "ps_6_0") };
        assert_eq!(st, 0);
        assert_eq!(&blob[..4], b"DXBC");
        assert!(blob.windows(4).any(|w| w == b"DXIL"));
        assert!(blob[4..20].iter().any(|&b| b != 0), "container not hashed by dxil.dll");
        let cs = "RWStructuredBuffer<uint> b : register(u0); [numthreads(64,1,1)] void main(uint i : SV_DispatchThreadID) { b[i] = i * 3; }";
        let (st, blob) = unsafe { compile(cs, "cs_6_0") };
        assert_eq!(st, 0);
        assert_eq!(&blob[..4], b"DXBC");
    }

    #[test]
    fn second_factory_creates_host_compiler_with_default_allocator() {
        let mut compiler = core::ptr::null_mut();
        let hr = DxcCreateInstance2(
            core::ptr::null_mut(),
            &CLSID_DXC_COMPILER as *const _ as *const c_void,
            &IID_IDXC_COMPILER3 as *const _ as *const c_void,
            &mut compiler,
        );
        assert_eq!(hr, 0);
        assert!(!compiler.is_null());
        unsafe {
            let release: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(slot(compiler, 2));
            release(compiler);
        }
    }

    #[test]
    fn bad_source_reports_failure_status() {
        let (st, _) = unsafe { compile("float4 main() : SV_Target { return undefined_thing; }", "ps_6_0") };
        assert!(st < 0);
    }

    #[test]
    fn null_output_is_cleared_before_forwarding() {
        let mut p = 8usize as *mut c_void;
        let zero = [0u8; 16];
        let hr = DxcCreateInstance(zero.as_ptr() as *const c_void, zero.as_ptr() as *const c_void, &mut p);
        assert!(hr < 0);
        assert!(p.is_null());
    }
}
