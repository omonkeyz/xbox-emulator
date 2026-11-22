use core::ffi::c_void;

extern "system" {
    fn CreateEventW(sa: *mut c_void, manual: i32, initial: i32, name: *const u16) -> isize;
    fn WaitForSingleObject(h: isize, ms: u32) -> u32;
}

thread_local! {
    static EV: isize = unsafe { CreateEventW(core::ptr::null_mut(), 0, 0, core::ptr::null()) };
}

pub unsafe fn wait_fence(fence: *mut c_void, value: u64, timeout_ms: u32) -> bool {
    let vt = *(fence as *const *const usize);
    let completed: unsafe extern "system" fn(*mut c_void) -> u64 = core::mem::transmute(*vt.add(8));
    let mut current = completed(fence);
    if current >= value {
        return current != u64::MAX;
    }
    let set_event: unsafe extern "system" fn(*mut c_void, u64, isize) -> i32 = core::mem::transmute(*vt.add(9));
    let ev = EV.with(|e| *e);
    let registered = ev != 0 && set_event(fence, value, ev) >= 0;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms.into());
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            current = completed(fence);
            return current != u64::MAX && current >= value;
        }
        // A registration from an earlier, timed-out call can signal this same event.
        // Only the fence value authorizes reuse of the command allocator.
        let woke = if registered {
            WaitForSingleObject(ev, remaining.as_nanos().div_ceil(1_000_000) as u32) == 0
        } else {
            std::thread::yield_now();
            true
        };
        current = completed(fence);
        if current >= value {
            return current != u64::MAX;
        }
        if !woke {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct FakeFence {
        table: *const usize,
        value: u64,
    }

    unsafe extern "system" fn completed(fence: *mut c_void) -> u64 {
        (*(fence as *const FakeFence)).value
    }

    unsafe extern "system" fn register(_fence: *mut c_void, _value: u64, event: isize) -> i32 {
        extern "system" { fn SetEvent(event: isize) -> i32; }
        assert_ne!(SetEvent(event), 0);
        0
    }

    #[test]
    fn stale_event_and_removed_device_do_not_complete_copy() {
        let mut table = [0usize; 10];
        table[8] = completed as *const () as usize;
        table[9] = register as *const () as usize;
        let mut fence = FakeFence { table: table.as_ptr(), value: 4 };
        unsafe {
            assert!(wait_fence((&raw mut fence).cast(), 4, 10));
            assert!(!wait_fence((&raw mut fence).cast(), 5, 10));
            fence.value = u64::MAX;
            assert!(!wait_fence((&raw mut fence).cast(), 5, 10));
        }
    }
}
