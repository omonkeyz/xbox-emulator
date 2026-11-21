use crate::com::*;
use crate::guids::*;
use core::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

extern "system" {
    fn LoadLibraryExW(p: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn LoadLibraryW(p: *const u16) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
    fn GetModuleHandleExW(flags: u32, name: *const u16, out: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(h: *mut c_void, buf: *mut u16, n: u32) -> u32;
}

const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;
const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;

pub type CreateFn = unsafe extern "system" fn(*const Guid, *const Guid, *mut *mut c_void) -> Hr;
pub type Create2Fn = unsafe extern "system" fn(*mut c_void, *const Guid, *const Guid, *mut *mut c_void) -> Hr;

#[derive(Clone, Debug)]
pub struct Host {
    pub path: PathBuf,
    pub module: usize,
    pub create: usize,
    pub create2: usize,
}

impl Host {
    pub unsafe fn create(&self, clsid: *const Guid, iid: *const Guid, ppv: *mut *mut c_void) -> Hr {
        let f: CreateFn = core::mem::transmute(self.create);
        f(clsid, iid, ppv)
    }

    pub unsafe fn create2(&self, malloc: *mut c_void, clsid: *const Guid, iid: *const Guid, ppv: *mut *mut c_void) -> Hr {
        if malloc.is_null() || self.create2 == 0 {
            return self.create(clsid, iid, ppv);
        }
        let f: Create2Fn = core::mem::transmute(self.create2);
        f(malloc, clsid, iid, ppv)
    }

    pub unsafe fn supports_utils(&self) -> bool {
        let mut p = core::ptr::null_mut();
        let hr = self.create(&CLSID_DXC_UTILS, &IID_IDXC_UTILS, &mut p);
        if hr >= 0 && !p.is_null() {
            release(p);
            true
        } else {
            false
        }
    }
}

fn own_module() -> usize {
    let mut h = core::ptr::null_mut();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            own_module as *const u16,
            &mut h,
        );
    }
    h as usize
}

fn module_dir(h: usize) -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetModuleFileNameW(h as *mut c_void, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    PathBuf::from(String::from_utf16_lossy(&buf[..n])).parent().map(Path::to_path_buf)
}

pub use compat_common::dxc::sdk_dirs;

pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(d) = std::env::var("TOMBOX_DXC_DIR") {
        v.push(PathBuf::from(d));
    }
    if let Some(d) = compat_common::cfg("dxc_dir") {
        v.push(PathBuf::from(d));
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        v.push(d);
    }
    if let Some(d) = module_dir(own_module()) {
        v.push(d);
    }
    v.extend(sdk_dirs());
    let mut seen = Vec::new();
    v.retain(|p| {
        let k = p.to_string_lossy().to_lowercase();
        let fresh = !seen.contains(&k);
        seen.push(k);
        fresh
    });
    v
}

pub fn load_from_dir(dir: &Path) -> Option<Host> {
    let comp = dir.join("dxcompiler.dll");
    if !comp.is_file() {
        return None;
    }
    let dxil = dir.join("dxil.dll");
    unsafe {
        if dxil.is_file() {
            LoadLibraryExW(wide(&dxil.to_string_lossy()).as_ptr(), core::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH);
        }
        let h = LoadLibraryExW(wide(&comp.to_string_lossy()).as_ptr(), core::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH);
        from_module(h, comp)
    }
}

unsafe fn from_module(h: *mut c_void, path: PathBuf) -> Option<Host> {
    if h.is_null() || h as usize == own_module() {
        return None;
    }
    let create = GetProcAddress(h, b"DxcCreateInstance\0".as_ptr()) as usize;
    if create == 0 {
        return None;
    }
    let create2 = GetProcAddress(h, b"DxcCreateInstance2\0".as_ptr()) as usize;
    Some(Host { path, module: h as usize, create, create2 })
}

fn load_from_search_path() -> Option<Host> {
    unsafe { from_module(LoadLibraryW(wide("dxcompiler.dll").as_ptr()), PathBuf::from("dxcompiler.dll")) }
}

pub fn discover() -> Option<Host> {
    if compat_common::cfg("dxc").map(|v| v.eq_ignore_ascii_case("native")).unwrap_or(false)
        || std::env::var("TOMBOX_DXC").map(|v| v.eq_ignore_ascii_case("native")).unwrap_or(false)
    {
        return None;
    }
    let mut tried = Vec::new();
    for dir in candidate_dirs() {
        if let Some(h) = load_from_dir(&dir) {
            if unsafe { h.supports_utils() } {
                return Some(h);
            }
            tried.push(h.path.display().to_string());
        }
    }
    if let Some(h) = load_from_search_path() {
        if unsafe { h.supports_utils() } {
            return Some(h);
        }
    }
    if !tried.is_empty() {
        crate::note("host", &format!("rejected (no IDxcUtils): {}", tried.join("; ")));
    }
    None
}

pub fn global() -> Option<&'static Host> {
    static HOST: OnceLock<Option<Host>> = OnceLock::new();
    HOST.get_or_init(|| {
        let h = discover();
        match &h {
            Some(h) => crate::note("host", &format!("host dxcompiler.dll = {} (DxcCreateInstance2 {})", h.path.display(), if h.create2 != 0 { "present" } else { "absent" })),
            None => crate::note("host", "no usable host dxcompiler.dll; IDxcUtils/IDxcLibrary served natively, other classes unavailable"),
        }
        h
    })
    .as_ref()
}
