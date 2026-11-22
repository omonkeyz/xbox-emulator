//! Xbox `CreatePlacedResourceX` (device slot 44): resources placed at a title-chosen
//! virtual address in unified (CPU == GPU) memory.
//!
//! Evidence: `FXboxCommonD3D12Adapter::CreatePlacedResourceX` (EXE+0x5c4610) calls slot 44 as
//!   (this, VirtualAddress, const D3D12_RESOURCE_DESC*, InitialState, pClearValue = null,
//!    riid, ppvResource)
//! which matches the public GDK signature. Its caller `CreateVirtualTexture` (EXE+0x5c4524)
//! obtains the address either by reserving a 64 KiB-aligned virtual block (streamed
//! textures; pages are mapped later) or from a small-texture pool.
//!
//! On console the texel memory at that address *is* the texture. A PC GPU cannot sample
//! title memory, so each placed resource gets a committed PC resource plus a registry entry
//! remembering the address; texel transfer from title memory is a separate step (upload.rs).
//!
//! Status: resource creation is implemented; see `Placed` for what is recorded.

use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct Placed {
    pub va: u64,
    /// The title's original (Xbox) D3D12_RESOURCE_DESC, 0x38 bytes.
    pub desc: [u8; 0x38],
    pub init_state: u32,
}

impl Placed {
    fn u32at(&self, o: usize) -> u32 { u32::from_le_bytes(self.desc[o..o + 4].try_into().unwrap()) }
    pub fn dim(&self) -> u32 { self.u32at(0) }
    pub fn width(&self) -> u64 { u64::from_le_bytes(self.desc[0x10..0x18].try_into().unwrap()) }
    pub fn height(&self) -> u32 { self.u32at(0x18) }
    pub fn depth_or_array(&self) -> u16 { u16::from_le_bytes(self.desc[0x1c..0x1e].try_into().unwrap()) }
    pub fn mips(&self) -> u16 { u16::from_le_bytes(self.desc[0x1e..0x20].try_into().unwrap()) }
    pub fn format(&self) -> u32 { self.u32at(0x20) }
    pub fn layout(&self) -> u32 { self.u32at(0x2c) }
    pub fn flags(&self) -> u32 { self.u32at(0x30) }
}

/// PC resource pointer -> placement record.
static REG: Mutex<Option<HashMap<usize, Placed>>> = Mutex::new(None);

/// Placements not yet seen by the uploader.
static NEW: Mutex<Vec<(usize, Placed)>> = Mutex::new(Vec::new());

/// Record a placed resource. The caller has taken one COM reference on `res` for the
/// registry; it is released by the uploader when the title drops the resource (or when the
/// resource is not mirrored).
const COMMITTED_TAG: [u8; 16] = [0x5b, 0x1e, 0x7a, 0x3c, 0x91, 0x0d, 0x4f, 0x62, 0xa8, 0x33, 0x17, 0xc4, 0x6e, 0x02, 0xd9, 0xb5];

pub fn is_committed_placement(res: *mut c_void) -> bool {
    if res.is_null() || crate::alias::is_aliased(res) {
        return false;
    }
    if lookup(res).is_some() {
        return true;
    }
    unsafe {
        let get: unsafe extern "system" fn(*mut c_void, *const u8, *mut u32, *mut c_void) -> i32 =
            core::mem::transmute(*(*(res as *const *const usize)).add(3));
        let mut v = 0u32;
        let mut n = 4u32;
        get(res, COMMITTED_TAG.as_ptr(), &mut n, &mut v as *mut u32 as *mut c_void) >= 0 && v == 1
    }
}

pub fn register(res: *mut c_void, p: Placed) {
    if !res.is_null() {
        unsafe {
            let set: unsafe extern "system" fn(*mut c_void, *const u8, u32, *const c_void) -> i32 =
                core::mem::transmute(*(*(res as *const *const usize)).add(4));
            let one = 1u32;
            set(res, COMMITTED_TAG.as_ptr(), 4, &one as *const u32 as *const c_void);
        }
    }
    REG.lock().unwrap().get_or_insert_with(HashMap::new).insert(res as usize, p.clone());
    NEW.lock().unwrap().push((res as usize, p));
}

pub fn drain_new() -> Vec<(usize, Placed)> {
    core::mem::take(&mut *NEW.lock().unwrap())
}

pub fn has_new() -> bool {
    NEW.lock().map(|g| !g.is_empty()).unwrap_or(false)
}

pub fn forget(res: *mut c_void) {
    if let Some(m) = REG.lock().unwrap().as_mut() {
        m.remove(&(res as usize));
    }
    if let Some(s) = GPU_WRITTEN.lock().unwrap().as_mut() {
        s.remove(&(res as usize));
    }
}

pub fn lookup(res: *mut c_void) -> Option<Placed> {
    REG.lock().unwrap().as_ref().and_then(|m| m.get(&(res as usize)).cloned())
}

pub fn count() -> usize {
    REG.lock().unwrap().as_ref().map_or(0, |m| m.len())
}

static GPU_WRITTEN: Mutex<Option<std::collections::HashSet<usize>>> = Mutex::new(None);
static GPU_WRITE_HITS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn gpu_copy_owned_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TOMBOX_GPU_COPY_OWNED").map(|v| v != "0").unwrap_or(true))
}

pub fn note_gpu_write(res: *mut c_void) {
    if res.is_null() || !gpu_copy_owned_enabled() {
        return;
    }
    let known = REG.lock().unwrap().as_ref().is_some_and(|m| m.contains_key(&(res as usize)));
    if !known {
        return;
    }
    GPU_WRITE_HITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    GPU_WRITTEN.lock().unwrap().get_or_insert_with(std::collections::HashSet::new).insert(res as usize);
}

pub fn gpu_written(res: usize) -> bool {
    GPU_WRITTEN.lock().unwrap().as_ref().is_some_and(|s| s.contains(&res))
}

pub fn gpu_write_stats() -> (u64, usize) {
    (GPU_WRITE_HITS.load(core::sync::atomic::Ordering::Relaxed), GPU_WRITTEN.lock().unwrap().as_ref().map_or(0, |s| s.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_new_tracking() {
        let _ = drain_new();
        assert!(!has_new());

        let p = Placed {
            va: 0x1234_0000,
            desc: [0u8; 0x38],
            init_state: 0,
        };
        register(core::ptr::null_mut(), p);
        assert!(has_new());

        let drained = drain_new();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].1.va, 0x1234_0000);
        assert!(!has_new());
    }
}
