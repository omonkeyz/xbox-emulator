use std::sync::atomic::{AtomicUsize, Ordering};

extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut core::ffi::c_void;
    fn SuspendThread(h: *mut core::ffi::c_void) -> u32;
    fn ResumeThread(h: *mut core::ffi::c_void) -> u32;
    fn GetThreadContext(h: *mut core::ffi::c_void, c: *mut u8) -> i32;
    fn SetThreadContext(h: *mut core::ffi::c_void, c: *const u8) -> i32;
    fn CloseHandle(h: *mut core::ffi::c_void) -> i32;
    fn AddVectoredExceptionHandler(first: u32, h: unsafe extern "system" fn(*mut *mut u8) -> i32) -> *mut core::ffi::c_void;
    fn RemoveVectoredExceptionHandler(h: *mut core::ffi::c_void) -> u32;
    fn GetModuleHandleExW(flags: u32, addr: usize, out: *mut usize) -> i32;
    fn GetModuleFileNameW(h: usize, buf: *mut u16, n: u32) -> u32;
}

pub(super) const LOG: usize = 4096;
pub(super) static POKE: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
pub(super) static VAL: [AtomicUsize; LOG] = [const { AtomicUsize::new(0) }; LOG];
pub(super) static N: AtomicUsize = AtomicUsize::new(0);
pub(super) static EV: [AtomicUsize; LOG * 2] = [const { AtomicUsize::new(0) }; LOG * 2];
pub(super) static STK: [AtomicUsize; LOG * 64] = [const { AtomicUsize::new(0) }; LOG * 64];

unsafe extern "system" fn veh(p: *mut *mut u8) -> i32 {
    let rec = *p;
    let ctx = *p.add(1);
    if *(rec as *const u32) != 0x8000_0004 {
        return 0;
    }
    let dr6 = *(ctx.add(0x68) as *const u64);
    let rip = *(ctx.add(0xf8) as *const u64) as usize;
    let i = N.fetch_add(1, Ordering::SeqCst);
    if i < LOG {
        EV[i * 2].store(rip, Ordering::SeqCst);
        EV[i * 2 + 1].store((dr6 & 0xf) as usize, Ordering::SeqCst);
        if let Some(k) = (0..3).find(|k| dr6 >> k & 1 != 0) {
            let a = *(ctx.add(0x48 + 8 * k) as *const usize);
            VAL[i].store(*(a as *const usize), Ordering::SeqCst);
        }
        let rsp = *(ctx.add(0x98) as *const u64) as *const usize;
        for k in 0..64 {
            STK[i * 64 + k].store(*rsp.add(k), Ordering::SeqCst);
        }
    }
    *(ctx.add(0x68) as *mut u64) = 0;
    let mask = POKE[2].load(Ordering::SeqCst);
    if dr6 & 8 != 0 && mask != 0 {
        let base = (*(ctx.add(0x98) as *const u64) as usize).wrapping_sub(POKE[0].load(Ordering::SeqCst)) as *mut u8;
        let pat = POKE[1].load(Ordering::SeqCst);
        for q in 0..8 {
            if mask >> q & 1 != 0 {
                for k in 0..8 {
                    let j = q * 8 + k;
                    *base.add(j) = (pat.wrapping_mul(0x9e37_79b9).wrapping_add(j * 0x45d9_f3b) >> 7) as u8;
                }
            }
        }
    }
    if dr6 & 8 != 0 {
        *(ctx.add(0x44) as *mut u32) |= 0x10000;
    }
    -1
}

#[repr(C, align(16))]
struct Ctx([u8; 1232]);

fn set_drs(tid: u32, dr: [usize; 4], dr7: u64) {
    std::thread::spawn(move || unsafe {
        let h = OpenThread(0x1fffff, 0, tid);
        assert!(!h.is_null());
        assert!(SuspendThread(h) != u32::MAX);
        let mut c = Box::new(Ctx([0; 1232]));
        *(c.0.as_mut_ptr().add(0x30) as *mut u32) = 0x0010_0010;
        assert!(GetThreadContext(h, c.0.as_mut_ptr()) != 0);
        for k in 0..4 {
            *(c.0.as_mut_ptr().add(0x48 + 8 * k) as *mut usize) = dr[k];
        }
        *(c.0.as_mut_ptr().add(0x68) as *mut u64) = 0;
        *(c.0.as_mut_ptr().add(0x70) as *mut u64) = dr7;
        assert!(SetThreadContext(h, c.0.as_ptr()) != 0);
        ResumeThread(h);
        CloseHandle(h);
    })
    .join()
    .unwrap();
}

pub(super) struct Watch {
    v: *mut core::ffi::c_void,
    tid: u32,
}

pub(super) fn rearm(data: &[usize]) {
    let tid = unsafe { GetCurrentThreadId() };
    let mut dr = [0usize; 4];
    let mut dr7 = 0u64;
    for (k, &a) in data.iter().take(3).enumerate() {
        dr[k] = a & !7;
        dr7 |= 1 << (2 * k) | 0b1101 << (16 + 4 * k);
    }
    set_drs(tid, dr, dr7);
}

impl Watch {
    pub(super) fn arm(data: &[usize], exec: Option<usize>) -> Watch {
        assert!(data.len() <= 3);
        N.store(0, Ordering::SeqCst);
        let v = unsafe { AddVectoredExceptionHandler(1, veh) };
        let tid = unsafe { GetCurrentThreadId() };
        let mut dr = [0usize; 4];
        let mut dr7 = 0u64;
        for (k, &a) in data.iter().enumerate() {
            dr[k] = a & !7;
            dr7 |= 1 << (2 * k) | 0b1101 << (16 + 4 * k);
        }
        if let Some(x) = exec {
            dr[3] = x;
            dr7 |= 1 << 6;
        }
        set_drs(tid, dr, dr7);
        Watch { v, tid }
    }
    pub(super) fn stack(&self, i: usize) -> [usize; 64] {
        core::array::from_fn(|k| STK[i * 64 + k].load(Ordering::SeqCst))
    }
    pub(super) fn events(&self) -> Vec<(usize, usize)> {
        let n = N.load(Ordering::SeqCst).min(LOG);
        (0..n).map(|i| (EV[i * 2].load(Ordering::SeqCst), EV[i * 2 + 1].load(Ordering::SeqCst))).collect()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        set_drs(self.tid, [0; 4], 0);
        unsafe { RemoveVectoredExceptionHandler(self.v) };
    }
}

pub(super) fn where_is(a: usize) -> String {
    let mut h = 0usize;
    if unsafe { GetModuleHandleExW(6, a, &mut h) } == 0 {
        return format!("{a:#x}");
    }
    let mut b = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(h, b.as_mut_ptr(), 260) } as usize;
    let name = String::from_utf16_lossy(&b[..n]);
    let short = name.rsplit('\\').next().unwrap_or(&name).to_string();
    format!("{short}+{:#x}", a - h)
}
