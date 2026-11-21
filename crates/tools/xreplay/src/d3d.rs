use core::ffi::c_void;

#[repr(C)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

pub const IID_DEVICE: Guid = Guid(0x189819f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
pub const IID_QUEUE: Guid = Guid(0x0ec870a6, 0x5d7e, 0x4c22, [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed]);
pub const IID_ALLOCATOR: Guid = Guid(0x6102dee4, 0xaf59, 0x4b09, [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24]);
pub const IID_GFX_LIST: Guid = Guid(0x5b160d0f, 0xac1b, 0x4185, [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55]);
pub const IID_FENCE: Guid = Guid(0x0a753dcf, 0xc4d8, 0x4b91, [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76]);
pub const IID_RESOURCE: Guid = Guid(0x696442be, 0xa72e, 0x4059, [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad]);
pub const IID_HEAP: Guid = Guid(0x8efb471d, 0x616c, 0x4f49, [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51]);
pub const IID_ROOTSIG: Guid = Guid(0xc54a6b66, 0x72df, 0x4ee8, [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14]);
pub const IID_PSO: Guid = Guid(0x765a30f3, 0xf624, 0x4c6f, [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45]);
pub const IID_QUERYHEAP: Guid = Guid(0x0d9658ae, 0xed45, 0x469e, [0xa6, 0x1d, 0x97, 0x0e, 0xc5, 0x83, 0xca, 0xb4]);
pub const IID_CMDSIG: Guid = Guid(0xc36a797c, 0xec80, 0x4f0a, [0x89, 0x85, 0xa7, 0xb2, 0x47, 0x50, 0x82, 0xd1]);
pub const IID_DEBUG: Guid = Guid(0x344488b7, 0x6846, 0x474b, [0xb9, 0x89, 0xf0, 0x27, 0x44, 0x82, 0x45, 0xe0]);
pub const IID_INFOQUEUE: Guid = Guid(0x0742a90b, 0xc387, 0x483f, [0xb9, 0x46, 0x30, 0xa7, 0xe4, 0xe6, 0x14, 0x58]);

pub const COMMON: u32 = 0;
pub const GENERIC_READ: u32 = 0xac3;
pub const COPY_DEST: u32 = 0x400;
pub const COPY_SOURCE: u32 = 0x800;

extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
    pub fn Sleep(ms: u32);
}

pub unsafe fn vt(obj: *mut c_void) -> *const usize {
    *(obj as *const *const usize)
}

#[macro_export]
macro_rules! vcall {
    ($obj:expr, $slot:expr, fn($($at:ty),*) -> $rt:ty $(, $arg:expr)*) => {{
        let o: *mut core::ffi::c_void = $obj as *mut core::ffi::c_void;
        let f: unsafe extern "system" fn(*mut core::ffi::c_void $(, $at)*) -> $rt = core::mem::transmute(*$crate::d3d::vt(o).add($slot));
        f(o $(, $arg)*)
    }};
}

pub unsafe fn release(obj: *mut c_void) {
    if !obj.is_null() {
        vcall!(obj, 2, fn() -> u32);
    }
}

pub struct Gpu {
    pub device: *mut c_void,
    pub queue: *mut c_void,
    pub alloc: *mut c_void,
    pub list: *mut c_void,
    pub fence: *mut c_void,
    pub value: u64,
    pub info: *mut c_void,
    pub open: bool,
}

fn hr_ok(hr: i32, what: &str) -> Result<(), String> {
    if hr < 0 { Err(format!("{what} failed hr={:#010x}", hr as u32)) } else { Ok(()) }
}

impl Gpu {
    pub unsafe fn new(debug: bool) -> Result<Gpu, String> {
        let d3d = LoadLibraryA(b"d3d12.dll\0".as_ptr());
        if d3d.is_null() {
            return Err("d3d12.dll not found".into());
        }
        if debug {
            let p = GetProcAddress(d3d, b"D3D12GetDebugInterface\0".as_ptr());
            if !p.is_null() {
                let f: unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(p);
                let mut dbg: *mut c_void = core::ptr::null_mut();
                if f(&IID_DEBUG, &mut dbg) >= 0 && !dbg.is_null() {
                    vcall!(dbg, 3, fn() -> ());
                    release(dbg);
                }
            }
        }
        let p = GetProcAddress(d3d, b"D3D12CreateDevice\0".as_ptr());
        if p.is_null() {
            return Err("D3D12CreateDevice missing".into());
        }
        let create: unsafe extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(p);
        let mut device: *mut c_void = core::ptr::null_mut();
        // Feature level 12_1 first, then whatever the device reaches. vkd3d-proton reports
        // a lower level for Mesa's llvmpipe, which is still usable here.
        let mut hr = -1;
        for level in [0xc100, 0xc000, 0xb100, 0xb000] {
            hr = create(core::ptr::null_mut(), level, &IID_DEVICE, &mut device);
            if hr >= 0 {
                break;
            }
        }
        hr_ok(hr, "D3D12CreateDevice")?;
        let qd: [u32; 4] = [0, 0, 0, 0];
        let mut queue: *mut c_void = core::ptr::null_mut();
        hr_ok(vcall!(device, 8, fn(*const u32, *const Guid, *mut *mut c_void) -> i32, qd.as_ptr(), &IID_QUEUE, &mut queue), "CreateCommandQueue")?;
        let mut alloc: *mut c_void = core::ptr::null_mut();
        hr_ok(vcall!(device, 9, fn(u32, *const Guid, *mut *mut c_void) -> i32, 0, &IID_ALLOCATOR, &mut alloc), "CreateCommandAllocator")?;
        let mut list: *mut c_void = core::ptr::null_mut();
        hr_ok(vcall!(device, 12, fn(u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32, 0, 0, alloc, core::ptr::null_mut(), &IID_GFX_LIST, &mut list), "CreateCommandList")?;
        vcall!(list, 9, fn() -> i32);
        let mut fence: *mut c_void = core::ptr::null_mut();
        hr_ok(vcall!(device, 36, fn(u64, u32, *const Guid, *mut *mut c_void) -> i32, 0, 0, &IID_FENCE, &mut fence), "CreateFence")?;
        let mut info: *mut c_void = core::ptr::null_mut();
        if debug {
            vcall!(device, 0, fn(*const Guid, *mut *mut c_void) -> i32, &IID_INFOQUEUE, &mut info);
        }
        Ok(Gpu { device, queue, alloc, list, fence, value: 0, info, open: true })
    }

    pub unsafe fn begin(&mut self) {
        if self.open {
            return;
        }
        self.open = true;
        vcall!(self.alloc, 8, fn() -> i32);
        vcall!(self.list, 10, fn(*mut c_void, *mut c_void) -> i32, self.alloc, core::ptr::null_mut());
    }

    pub unsafe fn submit(&mut self) -> Result<(), String> {
        self.open = false;
        let hr = vcall!(self.list, 9, fn() -> i32);
        if hr < 0 {
            return Err(format!("Close failed hr={:#010x}{}", hr as u32, self.messages()));
        }
        let lists = [self.list];
        vcall!(self.queue, 10, fn(u32, *const *mut c_void) -> (), 1, lists.as_ptr());
        self.value += 1;
        vcall!(self.queue, 14, fn(*mut c_void, u64) -> i32, self.fence, self.value);
        let mut spins = 0u32;
        while vcall!(self.fence, 8, fn() -> u64) < self.value {
            Sleep(1);
            spins += 1;
            if spins > 20_000 {
                return Err(format!("GPU timeout (removed reason {:#010x}){}", self.removed() as u32, self.messages()));
            }
        }
        let r = self.removed();
        if r < 0 {
            return Err(format!("device removed {:#010x}{}", r as u32, self.messages()));
        }
        Ok(())
    }

    pub unsafe fn removed(&self) -> i32 {
        vcall!(self.device, 37, fn() -> i32)
    }

    pub unsafe fn messages(&self) -> String {
        if self.info.is_null() {
            return String::new();
        }
        let n = vcall!(self.info, 8, fn() -> u64);
        let mut s = String::new();
        for i in 0..n.min(40) {
            let mut len = 0usize;
            vcall!(self.info, 5, fn(u64, *mut u8, *mut usize) -> i32, i, core::ptr::null_mut(), &mut len);
            if len == 0 {
                continue;
            }
            let mut buf = vec![0u64; len / 8 + 1];
            if vcall!(self.info, 5, fn(u64, *mut u8, *mut usize) -> i32, i, buf.as_mut_ptr() as *mut u8, &mut len) >= 0 {
                let b = buf.as_ptr() as *const u8;
                let desc = core::ptr::read_unaligned(b.add(16) as *const *const u8);
                let dl = core::ptr::read_unaligned(b.add(24) as *const usize);
                if !desc.is_null() && dl > 0 {
                    let t = String::from_utf8_lossy(core::slice::from_raw_parts(desc, dl.saturating_sub(1)));
                    s.push_str("\n    d3d12: ");
                    s.push_str(&t);
                }
            }
        }
        vcall!(self.info, 4, fn() -> ());
        s
    }

    pub unsafe fn buffer(&self, size: u64, heap: u32, state: u32, uav: bool) -> Result<*mut c_void, String> {
        let hp: [u32; 5] = [heap, 0, 0, 1, 1];
        let mut bd = [0u8; 0x38];
        bd[0..4].copy_from_slice(&1u32.to_le_bytes());
        bd[0x10..0x18].copy_from_slice(&size.max(1).to_le_bytes());
        bd[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
        bd[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
        bd[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
        bd[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
        bd[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
        if uav {
            bd[0x30..0x34].copy_from_slice(&4u32.to_le_bytes());
        }
        self.committed(&bd, heap_props(hp), state)
    }

    pub unsafe fn committed(&self, desc: &[u8; 0x38], hp: [u32; 5], state: u32) -> Result<*mut c_void, String> {
        let mut r: *mut c_void = core::ptr::null_mut();
        let hr = vcall!(self.device, 27, fn(*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
            hp.as_ptr(), 0, desc.as_ptr(), state, core::ptr::null(), &IID_RESOURCE, &mut r);
        if hr < 0 || r.is_null() {
            return Err(format!("CreateCommittedResource hr={:#010x}{}", hr as u32, self.messages()));
        }
        Ok(r)
    }

    pub unsafe fn footprints(&self, desc: &[u8; 0x38], subs: u32) -> (Vec<[u8; 32]>, u64) {
        let mut fps = vec![[0u8; 32]; subs as usize];
        let mut total = 0u64;
        vcall!(self.device, 38, fn(*const u8, u32, u32, u64, *mut [u8; 32], *mut u32, *mut u64, *mut u64) -> (),
            desc.as_ptr(), 0, subs, 0, fps.as_mut_ptr(), core::ptr::null_mut(), core::ptr::null_mut(), &mut total);
        (fps, total)
    }

    pub unsafe fn barrier(&self, res: *mut c_void, before: u32, after: u32) {
        if before == after {
            return;
        }
        let b = [0u64, res as u64, 0xffff_ffffu64 | ((before as u64) << 32), after as u64];
        vcall!(self.list, 26, fn(u32, *const u64) -> (), 1, b.as_ptr());
    }

    pub unsafe fn uav_barrier(&self) {
        let b = [2u64, 0, 0, 0];
        vcall!(self.list, 26, fn(u32, *const u64) -> (), 1, b.as_ptr());
    }

    pub unsafe fn map_read(res: *mut c_void, size: usize) -> Option<Vec<u8>> {
        let mut p: *mut u8 = core::ptr::null_mut();
        if vcall!(res, 8, fn(u32, *const c_void, *mut *mut u8) -> i32, 0, core::ptr::null(), &mut p) < 0 || p.is_null() {
            return None;
        }
        let v = core::slice::from_raw_parts(p, size).to_vec();
        vcall!(res, 9, fn(u32, *const c_void) -> (), 0, core::ptr::null());
        Some(v)
    }

    pub unsafe fn map_write(res: *mut c_void, data: &[u8]) -> bool {
        let mut p: *mut u8 = core::ptr::null_mut();
        let range = [0usize, 0usize];
        if vcall!(res, 8, fn(u32, *const usize, *mut *mut u8) -> i32, 0, range.as_ptr(), &mut p) < 0 || p.is_null() {
            return false;
        }
        core::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        vcall!(res, 9, fn(u32, *const c_void) -> (), 0, core::ptr::null());
        true
    }

    pub unsafe fn gpu_va(res: *mut c_void) -> u64 {
        vcall!(res, 11, fn() -> u64)
    }

    pub unsafe fn desc_of(res: *mut c_void) -> [u8; 0x38] {
        let mut d = [0u8; 0x38];
        vcall!(res, 10, fn(*mut u8) -> *mut u8, d.as_mut_ptr());
        d
    }

    pub unsafe fn heap(&self, ty: u32, count: u32, visible: bool) -> Result<(*mut c_void, usize, u64, usize), String> {
        let d: [u32; 4] = [ty, count.max(1), if visible { 1 } else { 0 }, 0];
        let mut h: *mut c_void = core::ptr::null_mut();
        let hr = vcall!(self.device, 14, fn(*const u32, *const Guid, *mut *mut c_void) -> i32, d.as_ptr(), &IID_HEAP_DESC, &mut h);
        if hr < 0 || h.is_null() {
            return Err(format!("CreateDescriptorHeap(type {ty}, {count}) hr={:#010x}{}", hr as u32, self.messages()));
        }
        let mut cpu = 0usize;
        let f: unsafe extern "system" fn(*mut c_void, *mut usize) -> *mut usize = core::mem::transmute(*vt(h).add(9));
        f(h, &mut cpu);
        let mut gpu = 0u64;
        if visible {
            let g: unsafe extern "system" fn(*mut c_void, *mut u64) -> *mut u64 = core::mem::transmute(*vt(h).add(10));
            g(h, &mut gpu);
        }
        let inc = vcall!(self.device, 15, fn(u32) -> u32, ty) as usize;
        Ok((h, cpu, gpu, inc))
    }
}

pub const IID_HEAP_DESC: Guid = Guid(0x8efb471d, 0x616c, 0x4f49, [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51]);

pub fn heap_props(h: [u32; 5]) -> [u32; 5] {
    h
}

pub fn default_heap() -> [u32; 5] {
    [1, 0, 0, 1, 1]
}

pub fn upload_heap() -> [u32; 5] {
    [2, 0, 0, 1, 1]
}

pub fn readback_heap() -> [u32; 5] {
    [3, 0, 0, 1, 1]
}
