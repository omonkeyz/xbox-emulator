use core::ffi::c_void;
use std::sync::Mutex;

use crate::{xmem_proc, wide, GetModuleHandleW, GetProcAddress, LoadLibraryW, XAUDIO_HEAP_ATTRIBUTES};

pub const ACP_STATE_SIZE: usize = 0x3c0;
pub const SHAPE_ALLOCATOR_SIZE: usize = 0x6c;
pub const POOL_SIZE: usize = 0x1c8;
pub const POOL_A_ELEMENT: u32 = 0x40;
pub const POOL_B_ELEMENT: u32 = 0x80;
pub const POOL_MAX: u64 = 0x8000;
pub const STATE_POOL_B: usize = 0x1c8;
pub const STATE_DEBUG: usize = 0x390;
pub const STATE_ALLOCATOR: usize = 0x3a8;
pub const STATE_HAL: usize = 0x3b0;
pub const STATE_INITIALIZED: usize = 0x3b8;
pub const STATE_FLAG: usize = 0x3b9;
pub const HAL_COMMANDS: u32 = 0x400;
pub const HAL_MESSAGES: u32 = 0x20;
pub const HAL_CMD_CONFIG: u32 = 2;
pub const HAL_CONFIG_VALUE: u32 = 0x80;
pub const MSG_DISCONNECTED: u32 = 0x80;
pub const DEFAULT_XMA_CONTEXTS: u32 = 0x200;
pub const CONTEXT_SHADOW_SIZES: [u32; 6] = [0x20, 0xa0, 0x20, 0x10, 0x40, 0x20];
pub const OWNER_SHARED: u8 = 0x04;
pub const OWNER_LOCAL: u8 = 0x02;
const E_OUTOFMEMORY: i32 = 0x8007_000Eu32 as i32;
const E_NOINTERFACE_HAL: i32 = 0x8007_007Eu32 as i32;

#[link(name = "kernel32")]
extern "system" {
    fn Sleep(ms: u32);
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ShapeTable {
    pub counts: [u32; 6],
    pub ptrs: [usize; 6],
    pub apus: [u32; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ShapeConfig {
    pub table: *const ShapeTable,
    pub base: [u32; 6],
    pub count: [u32; 6],
}

#[repr(C)]
pub struct ShapeAllocator {
    pub table: *mut ShapeTable,
    pub base: [u32; 6],
    pub count: [u32; 6],
    pub begin: [u32; 6],
    pub end: [u32; 6],
    pub local: u8,
}

struct Globals {
    table: ShapeTable,
    saved: [u32; 6],
    owners: [Vec<u8>; 6],
    shadow: [usize; 6],
    shadow_apu: [u32; 6],
    xma_read: usize,
    xma_read_apu: u32,
    xma_write: usize,
    xma_write_apu: u32,
    objects: u32,
    initialized: bool,
    allocated: bool,
}

struct Shared(Globals);
unsafe impl Send for Shared {}

static GLOBALS: Mutex<Shared> = Mutex::new(Shared(Globals {
    table: ShapeTable { counts: [0; 6], ptrs: [0; 6], apus: [0; 6] },
    saved: [0; 6],
    owners: [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
    shadow: [0; 6],
    shadow_apu: [0; 6],
    xma_read: 0,
    xma_read_apu: 0,
    xma_write: 0,
    xma_write_apu: 0,
    objects: 0,
    initialized: false,
    allocated: false,
}));

fn globals() -> std::sync::MutexGuard<'static, Shared> {
    GLOBALS.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn shape_table() -> ShapeTable {
    globals().0.table
}

pub fn shape_objects() -> u32 {
    globals().0.objects
}

pub fn owner_flags(kind: usize) -> Vec<u8> {
    globals().0.owners[kind].clone()
}

pub fn shadow_arrays() -> ([usize; 6], [u32; 6]) {
    let g = globals();
    (g.0.shadow, g.0.shadow_apu)
}

pub fn xma_side_arrays() -> (usize, usize) {
    let g = globals();
    (g.0.xma_read, g.0.xma_write)
}

pub fn shape_table_address() -> *mut ShapeTable {
    let g = globals();
    &g.0.table as *const ShapeTable as *mut ShapeTable
}

struct Hal {
    alloc: unsafe extern "system" fn(*mut *mut c_void, *mut u32, u32, u32, u32) -> i32,
    free: unsafe extern "system" fn(*mut c_void) -> i32,
    map_va: unsafe extern "system" fn(*const c_void) -> u32,
    alloc_shape: unsafe extern "system" fn(*mut u8) -> i32,
    release_shape: unsafe extern "system" fn() -> i32,
    create: unsafe extern "system" fn(*mut *mut c_void) -> i32,
}

static HAL: std::sync::OnceLock<Option<Hal>> = std::sync::OnceLock::new();

fn hal() -> Option<&'static Hal> {
    HAL.get_or_init(|| unsafe {
        let name = wide("AcpHal.dll");
        let mut m = GetModuleHandleW(name.as_ptr());
        if m == 0 {
            m = LoadLibraryW(name.as_ptr());
        }
        if m == 0 {
            return None;
        }
        let p = |n: &[u8]| GetProcAddress(m, n.as_ptr()) as usize;
        let v = [p(b"ApuAlloc\0"), p(b"ApuFree\0"), p(b"ApuMapVirtualAddress\0"), p(b"AcpHalAllocateShapeContexts\0"), p(b"AcpHalReleaseShapeContexts\0"), p(b"AcpHalCreate\0")];
        if v.iter().any(|&x| x == 0) {
            return None;
        }
        Some(Hal {
            alloc: core::mem::transmute(v[0]),
            free: core::mem::transmute(v[1]),
            map_va: core::mem::transmute(v[2]),
            alloc_shape: core::mem::transmute(v[3]),
            release_shape: core::mem::transmute(v[4]),
            create: core::mem::transmute(v[5]),
        })
    })
    .as_ref()
}

pub unsafe fn heap_alloc(size: usize) -> *mut u8 {
    let f = xmem_proc(b"XMemAlloc\0");
    if f.is_null() {
        return core::ptr::null_mut();
    }
    let f: unsafe extern "system" fn(usize, u64) -> *mut c_void = core::mem::transmute(f);
    f(size, XAUDIO_HEAP_ATTRIBUTES) as *mut u8
}

pub unsafe fn heap_free(p: *mut u8) {
    let f = xmem_proc(b"XMemFree\0");
    if !f.is_null() && !p.is_null() {
        let f: unsafe extern "system" fn(*mut c_void, u64) = core::mem::transmute(f);
        f(p as *mut c_void, XAUDIO_HEAP_ATTRIBUTES);
    }
}

unsafe fn vcall(o: *mut c_void, slot: usize) -> usize {
    *(*(o as *const *const usize)).add(slot)
}

unsafe fn init_pool(p: *mut u8, element: u32, max: u64) {
    core::ptr::write_bytes(p, 0, POOL_SIZE);
    let w = |o: usize, v: u64| core::ptr::write_unaligned(p.add(o) as *mut u64, v);
    w(0x198, 0);
    w(0x1a0, 0);
    w(0x1a8, 0);
    w(0x1b0, 0x10);
    let block = p.add(0x10) as u64;
    w(8, block);
    core::ptr::write_unaligned(block as *mut u64, 0);
    for i in 0..0x10usize {
        let node = block as usize + i * 0x18 + 8;
        let head = core::ptr::read_unaligned(p as *const u64);
        core::ptr::write_unaligned((node + 8) as *mut u64, head);
        w(0, node as u64);
    }
    core::ptr::write_unaligned(p.add(0x1b8) as *mut u32, element);
    core::ptr::write_unaligned(p.add(0x1c0) as *mut u64, max);
}

unsafe fn release_globals(g: &mut Globals) {
    let Some(h) = hal() else { return };
    if g.allocated {
        (h.release_shape)();
    }
    if g.xma_read != 0 {
        (h.free)(g.xma_read as *mut c_void);
        g.xma_read = 0;
        g.xma_read_apu = 0;
    }
    if g.xma_write != 0 {
        (h.free)(g.xma_write as *mut c_void);
        g.xma_write = 0;
        g.xma_write_apu = 0;
    }
    for k in 0..6 {
        if g.shadow[k] != 0 {
            (h.free)(g.shadow[k] as *mut c_void);
        }
        g.owners[k] = Vec::new();
    }
    g.shadow = [0; 6];
    g.shadow_apu = [0; 6];
    g.table = ShapeTable::default();
    g.saved = [0; 6];
    g.allocated = false;
    g.initialized = false;
}

unsafe fn alloc_shadow(h: &Hal, g: &mut Globals, k: usize) -> i32 {
    let mut p: *mut c_void = core::ptr::null_mut();
    let mut a = 0u32;
    let hr = (h.alloc)(&mut p, &mut a, g.table.counts[k].wrapping_mul(CONTEXT_SHADOW_SIZES[k]), 0x100, 1);
    g.shadow[k] = p as usize;
    g.shadow_apu[k] = a;
    if hr < 0 || k != 4 {
        return hr;
    }
    let bytes = g.table.counts[4].wrapping_mul(4);
    let mut r: *mut c_void = core::ptr::null_mut();
    let hr = (h.alloc)(&mut r, &mut g.xma_read_apu, bytes, 4, 2);
    g.xma_read = r as usize;
    if hr < 0 {
        return hr;
    }
    core::ptr::write_bytes(r as *mut u8, 0, bytes as usize);
    let mut w: *mut c_void = core::ptr::null_mut();
    let hr = (h.alloc)(&mut w, &mut g.xma_write_apu, bytes, 4, 2);
    g.xma_write = w as usize;
    if hr < 0 {
        return hr;
    }
    core::ptr::write_bytes(w as *mut u8, 0, bytes as usize);
    hr
}

fn owner_value(local: bool) -> u8 {
    OWNER_SHARED | if local { OWNER_LOCAL } else { 0 }
}

unsafe fn delete_allocator(g: &mut Globals, obj: *mut ShapeAllocator) {
    if obj.is_null() {
        return;
    }
    heap_free(obj as *mut u8);
    g.objects = g.objects.saturating_sub(1);
    if g.objects == 0 {
        release_globals(g);
    }
}

pub unsafe fn create_shape_allocator(out: *mut *mut ShapeAllocator, config: *const ShapeConfig, flag: bool) -> i32 {
    let Some(h) = hal() else { return E_NOINTERFACE_HAL };
    let mut sh = globals();
    let g = &mut sh.0;
    let local = !flag;
    let obj = heap_alloc(SHAPE_ALLOCATOR_SIZE) as *mut ShapeAllocator;
    if obj.is_null() {
        return E_OUTOFMEMORY;
    }
    g.objects += 1;
    core::ptr::write_bytes(obj as *mut u8, 0, SHAPE_ALLOCATOR_SIZE);
    let o = &mut *obj;
    o.local = local as u8;
    let mut hr = 0;
    if !config.is_null() {
        let c = &*config;
        o.table = &mut g.table;
        o.base = c.base;
        o.count = c.count;
        let ct = &*c.table;
        for k in 0..6 {
            if hr < 0 {
                break;
            }
            if g.table.ptrs[k] == 0 && ct.ptrs[k] != 0 && ct.counts[k] > 0 {
                g.table.ptrs[k] = ct.ptrs[k];
                g.table.counts[k] = ct.counts[k];
                g.table.apus[k] = if ct.apus[k] != 0 { ct.apus[k] } else { (h.map_va)(ct.ptrs[k] as *const c_void) };
                hr = alloc_shadow(h, g, k);
            }
        }
        for k in 0..6 {
            if hr < 0 || c.count[k] == 0 {
                continue;
            }
            let lo = c.base[k];
            let hi = c.base[k].wrapping_add(c.count[k]).wrapping_sub(1);
            if g.owners[k].is_empty() {
                let n = g.table.counts[k] as usize;
                g.owners[k] = (0..n as u32).map(|i| if i >= lo && i <= hi { owner_value(local) } else { 0 }).collect();
            } else {
                let n = g.owners[k].len() as u32;
                for i in lo..=hi.min(n.saturating_sub(1)) {
                    g.owners[k][i as usize] |= OWNER_SHARED;
                }
            }
        }
        if hr >= 0 {
            for k in 0..6 {
                if c.count[k] > 0 {
                    o.begin[k] = c.base[k];
                    o.end[k] = c.base[k].wrapping_add(c.count[k]);
                }
            }
        }
    } else {
        if !g.initialized {
            g.table.counts[4] = DEFAULT_XMA_CONTEXTS;
            g.saved = g.table.counts;
            for k in 0..6 {
                if hr < 0 || g.table.counts[k] == 0 {
                    continue;
                }
                g.owners[k] = vec![owner_value(local); g.table.counts[k] as usize];
                hr = alloc_shadow(h, g, k);
            }
            if hr >= 0 {
                hr = (h.alloc_shape)(&mut g.table as *mut ShapeTable as *mut u8);
                if hr >= 0 {
                    g.allocated = true;
                }
            }
        }
        if hr >= 0 {
            o.table = &mut g.table;
            o.count = g.table.counts;
            for k in 0..6 {
                if g.table.counts[k] > 0 {
                    o.end[k] = g.table.counts[k];
                }
            }
        }
    }
    if hr >= 0 {
        g.initialized = true;
        *out = obj;
    } else {
        delete_allocator(g, obj);
    }
    hr
}

pub unsafe fn destroy_shape_allocator(obj: *mut ShapeAllocator) {
    let mut sh = globals();
    delete_allocator(&mut sh.0, obj);
}

pub unsafe fn construct_state(s: *mut u8) {
    core::ptr::write_bytes(s, 0, ACP_STATE_SIZE);
    init_pool(s, POOL_A_ELEMENT, POOL_MAX);
    init_pool(s.add(STATE_POOL_B), POOL_B_ELEMENT, POOL_MAX);
}

pub unsafe fn initialize_state(s: *mut u8, config: *const ShapeConfig) -> i32 {
    if *s.add(STATE_INITIALIZED) != 0 {
        return 0;
    }
    let Some(h) = hal() else { return E_NOINTERFACE_HAL };
    let alloc_slot = s.add(STATE_ALLOCATOR) as *mut *mut ShapeAllocator;
    let hal_slot = s.add(STATE_HAL) as *mut *mut c_void;
    let mut hr = create_shape_allocator(alloc_slot, config, *s.add(STATE_FLAG) != 0);
    if hr >= 0 {
        hr = (h.create)(hal_slot);
    }
    if hr >= 0 {
        let o = *hal_slot;
        let connect: unsafe extern "system" fn(*mut c_void, u32, u32) -> i32 = core::mem::transmute(vcall(o, 0));
        hr = connect(o, HAL_COMMANDS, HAL_MESSAGES);
    }
    if hr >= 0 {
        *s.add(STATE_INITIALIZED) = 1;
        let o = *hal_slot;
        let payload: [u32; 2] = [HAL_CONFIG_VALUE, 0];
        let submit: unsafe extern "system" fn(*mut c_void, u32, u64, u32, *const u8, u32) -> i32 = core::mem::transmute(vcall(o, 2));
        hr = submit(o, HAL_CMD_CONFIG, 0, 0xffff_ffff, payload.as_ptr() as *const u8, 0);
    }
    if hr < 0 && *s.add(STATE_INITIALIZED) == 0 {
        release_hal(hal_slot);
        let a = *alloc_slot;
        if !a.is_null() {
            destroy_shape_allocator(a);
            *alloc_slot = core::ptr::null_mut();
        }
    }
    hr
}

unsafe fn release_hal(slot: *mut *mut c_void) {
    let o = *slot;
    if !o.is_null() {
        let rel: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(vcall(o, 5));
        rel(o);
        *slot = core::ptr::null_mut();
    }
}

pub unsafe fn destruct_state(s: *mut u8) {
    let hal_slot = s.add(STATE_HAL) as *mut *mut c_void;
    if *s.add(STATE_INITIALIZED) != 0 {
        let o = *hal_slot;
        let disconnect: unsafe extern "system" fn(*mut c_void) -> i32 = core::mem::transmute(vcall(o, 1));
        if disconnect(o) >= 0 {
            let pop: unsafe extern "system" fn(*mut c_void, *mut u8) -> i32 = core::mem::transmute(vcall(o, 3));
            let mut msg = [0u8; 0x1c];
            let mut tries = 1000u32;
            loop {
                tries -= 1;
                if tries == 0 {
                    break;
                }
                if pop(o, msg.as_mut_ptr()) != 0 && u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]) == MSG_DISCONNECTED {
                    break;
                }
                Sleep(1);
            }
        }
        *s.add(STATE_INITIALIZED) = 0;
    }
    release_hal(hal_slot);
    let alloc_slot = s.add(STATE_ALLOCATOR) as *mut *mut ShapeAllocator;
    if !(*alloc_slot).is_null() {
        destroy_shape_allocator(*alloc_slot);
    }
    *alloc_slot = core::ptr::null_mut();
}

#[no_mangle]
pub unsafe extern "system" fn CreateXAudioAcpState(config: *const ShapeConfig, flag: u32, out: *mut *mut u8) -> i32 {
    *out = core::ptr::null_mut();
    let s = heap_alloc(ACP_STATE_SIZE);
    if s.is_null() {
        return E_OUTOFMEMORY;
    }
    construct_state(s);
    *s.add(STATE_FLAG) = (flag != 0) as u8;
    let hr = if config.is_null() { 0 } else { initialize_state(s, config) };
    if hr >= 0 {
        *out = s;
    } else {
        DestroyXAudioAcpState(s);
    }
    hr
}

#[no_mangle]
pub unsafe extern "system" fn DestroyXAudioAcpState(s: *mut u8) {
    if s.is_null() {
        return;
    }
    destruct_state(s);
    heap_free(s);
}
