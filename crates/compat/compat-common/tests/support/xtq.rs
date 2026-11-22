// XTaskQueue / XAsync conformance scenarios, called through the 073b7dcb
// QueryApiImpl vtable the way a title calls them.
//
// Each scenario ports the behaviour checked by a test in Microsoft's
// libHttpClient (MIT), which ships the reference XTaskQueue/XAsync
// implementation: Tests/UnitTests/Tests/TaskQueueTests.cpp and
// AsyncBlockTests.cpp at commit c5007fd9cbdc785518ea6dccc5751b125d451cc1. Only
// the scenarios are ported, not the C++ code. The libHttpClient test name is
// given on each function. Behaviour that the tests do not pin down is taken
// from Source/Task/TaskQueue.cpp and AsyncLib.cpp at the same commit.
//
// The same scenarios run against the emulator (gdk_xtaskqueue_conformance.rs)
// and against the PC Gaming Runtime's xgameruntime.dll when it is installed
// (gdk_xtaskqueue_conformance_real.rs).
#![allow(dead_code)]

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const MANUAL: u32 = 0;
pub const THREAD_POOL: u32 = 1;
pub const SERIALIZED: u32 = 2;
pub const IMMEDIATE: u32 = 3;
pub const WORK: u32 = 0;
pub const COMPLETION: u32 = 1;
pub const INFINITE: u32 = u32::MAX;

pub const E_ABORT: i32 = 0x8000_4004u32 as i32;
pub const E_FAIL: i32 = 0x8000_4005u32 as i32;
pub const E_PENDING: i32 = 0x8000_000Au32 as i32;
pub const E_UNEXPECTED: i32 = 0x8000_FFFFu32 as i32;
pub const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
pub const E_ILLEGAL_METHOD_CALL: i32 = 0x8000_000Eu32 as i32;
pub const E_NOT_SUFFICIENT_BUFFER: i32 = 0x8007_007Au32 as i32;
pub const E_NO_TASK_QUEUE: i32 = 0x8007_01ABu32 as i32;
pub const E_ACCESSDENIED: i32 = 0x8007_0005u32 as i32;

pub type TaskCb = unsafe extern "system" fn(*mut c_void, bool);
pub type MonitorCb = unsafe extern "system" fn(*mut c_void, usize, u32);
pub type TermCb = unsafe extern "system" fn(*mut c_void);
pub type Provider = unsafe extern "system" fn(u32, *const Data) -> i32;
pub type Completion = unsafe extern "system" fn(*mut Block);
pub type Work = unsafe extern "system" fn(*mut Block) -> i32;

pub const OP_BEGIN: u32 = 0;
pub const OP_DO_WORK: u32 = 1;
pub const OP_GET_RESULT: u32 = 2;
pub const OP_CANCEL: u32 = 3;
pub const OP_CLEANUP: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Block {
    pub queue: usize,
    pub context: usize,
    pub callback: Option<Completion>,
    pub internal: [u8; 32],
}

impl Block {
    pub fn new(queue: usize) -> Box<Block> {
        Box::new(Block { queue, context: 0, callback: None, internal: [0; 32] })
    }
    fn bytes(&self) -> [u8; 56] {
        unsafe { core::mem::transmute_copy(self) }
    }
}

#[repr(C)]
pub struct Data {
    pub block: *mut Block,
    pub size: usize,
    pub buffer: *mut u8,
    pub context: usize,
}

extern "system" {
    fn CreateEventW(attrs: *const c_void, manual: i32, initial: i32, name: *const u16) -> usize;
    fn SetEvent(h: usize) -> i32;
    fn ResetEvent(h: usize) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn WaitForSingleObject(h: usize, ms: u32) -> u32;
}

pub struct Event(pub usize);
impl Event {
    pub fn new(manual: bool) -> Event {
        let h = unsafe { CreateEventW(core::ptr::null(), manual as i32, 0, core::ptr::null()) };
        assert_ne!(h, 0);
        Event(h)
    }
    pub fn set(&self) {
        unsafe { SetEvent(self.0) };
    }
    pub fn reset(&self) {
        unsafe { ResetEvent(self.0) };
    }
    pub fn wait(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.0, ms) == 0 }
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[derive(Clone, Copy)]
pub struct Api {
    obj: usize,
}

static API: OnceLock<Api> = OnceLock::new();

pub fn install(obj: *mut c_void) {
    assert!(!obj.is_null());
    let _ = API.set(Api { obj: obj as usize });
}

pub fn api() -> Api {
    *API.get().expect("XAsync interface not installed")
}

// Set by the emulator's test binary. The few places where the console runtime (as checked by the
// maintainer against the console DLL, tests/gdk_verifier_fixes.rs) differs from libHttpClient and
// the PC runtime read it; everything else is asserted identically on both.
static CONSOLE_MODEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_console_model(on: bool) {
    CONSOLE_MODEL.store(on, SeqCst);
}

fn console_model() -> bool {
    CONSOLE_MODEL.load(SeqCst)
}

type Once = Box<dyn FnOnce(bool) + Send>;
unsafe extern "system" fn once(ctx: *mut c_void, canceled: bool) {
    Box::from_raw(ctx as *mut Once)(canceled)
}
type Many = dyn Fn(bool) + Send + Sync;
unsafe extern "system" fn many(ctx: *mut c_void, canceled: bool) {
    (*(ctx as *const Box<Many>))(canceled)
}
type Mon = Box<dyn Fn(usize, u32) + Send + Sync>;
unsafe extern "system" fn monitor_tramp(ctx: *mut c_void, queue: usize, port: u32) {
    (*(ctx as *const Mon))(queue, port)
}
type Term = Box<dyn FnOnce() + Send>;
unsafe extern "system" fn term_tramp(ctx: *mut c_void) {
    Box::from_raw(ctx as *mut Term)()
}

fn leak<T>(v: T) -> *mut c_void {
    Box::into_raw(Box::new(v)) as *mut c_void
}

impl Api {
    fn f(&self, slot: usize) -> usize {
        unsafe { *(*(self.obj as *const *const usize)).add(slot) }
    }
    fn o(&self) -> *mut c_void {
        self.obj as *mut c_void
    }

    pub fn try_create(&self, work: u32, completion: u32, out: &mut usize) -> i32 {
        let f: unsafe extern "system" fn(*mut c_void, u32, u32, *mut usize) -> i32 = unsafe { core::mem::transmute(self.f(12)) };
        unsafe { f(self.o(), work, completion, out) }
    }
    pub fn create(&self, work: u32, completion: u32) -> usize {
        let mut q = 0;
        assert_eq!(self.try_create(work, completion, &mut q), 0);
        assert_ne!(q, 0);
        q
    }
    pub fn try_composite(&self, work: usize, completion: usize, out: &mut usize) -> i32 {
        let f: unsafe extern "system" fn(*mut c_void, usize, usize, *mut usize) -> i32 = unsafe { core::mem::transmute(self.f(13)) };
        unsafe { f(self.o(), work, completion, out) }
    }
    pub fn composite(&self, work: usize, completion: usize) -> usize {
        let mut q = 0;
        assert_eq!(self.try_composite(work, completion, &mut q), 0);
        q
    }
    pub fn port(&self, q: usize, port: u32) -> usize {
        let f: unsafe extern "system" fn(*mut c_void, usize, u32, *mut usize) -> i32 = unsafe { core::mem::transmute(self.f(14)) };
        let mut p = 0;
        assert_eq!(unsafe { f(self.o(), q, port, &mut p) }, 0);
        assert_ne!(p, 0);
        p
    }
    pub fn try_dup(&self, q: usize, out: &mut usize) -> i32 {
        let f: unsafe extern "system" fn(*mut c_void, usize, *mut usize) -> i32 = unsafe { core::mem::transmute(self.f(15)) };
        unsafe { f(self.o(), q, out) }
    }
    pub fn dup(&self, q: usize) -> usize {
        let mut d = 0;
        assert_eq!(self.try_dup(q, &mut d), 0);
        d
    }
    pub fn dispatch(&self, q: usize, port: u32, timeout: u32) -> bool {
        let f: unsafe extern "system" fn(*mut c_void, usize, u32, u32) -> bool = unsafe { core::mem::transmute(self.f(16)) };
        unsafe { f(self.o(), q, port, timeout) }
    }
    pub fn close(&self, q: usize) {
        let f: unsafe extern "system" fn(*mut c_void, usize) = unsafe { core::mem::transmute(self.f(17)) };
        unsafe { f(self.o(), q) }
    }
    pub fn submit_raw(&self, q: usize, port: u32, delay: u32, ctx: *mut c_void, cb: TaskCb) -> i32 {
        if delay == 0 {
            let f: unsafe extern "system" fn(*mut c_void, usize, u32, *mut c_void, TaskCb) -> i32 = unsafe { core::mem::transmute(self.f(18)) };
            unsafe { f(self.o(), q, port, ctx, cb) }
        } else {
            let f: unsafe extern "system" fn(*mut c_void, usize, u32, u32, *mut c_void, TaskCb) -> i32 = unsafe { core::mem::transmute(self.f(19)) };
            unsafe { f(self.o(), q, port, delay, ctx, cb) }
        }
    }
    // XTaskQueueSubmitDelayedCallback with delay 0 is XTaskQueueSubmitCallback in
    // the reference; both entry points are exercised by `delay`.
    pub fn submit(&self, q: usize, port: u32, delay: u32, f: impl FnOnce(bool) + Send + 'static) -> i32 {
        let ctx = leak::<Once>(Box::new(f));
        let hr = self.submit_raw(q, port, delay, ctx, once);
        if hr < 0 {
            drop(unsafe { Box::from_raw(ctx as *mut Once) });
        }
        hr
    }
    pub fn submit_delayed_slot(&self, q: usize, port: u32, delay: u32, f: impl FnOnce(bool) + Send + 'static) -> i32 {
        let f2: unsafe extern "system" fn(*mut c_void, usize, u32, u32, *mut c_void, TaskCb) -> i32 = unsafe { core::mem::transmute(self.f(19)) };
        let ctx = leak::<Once>(Box::new(f));
        let hr = unsafe { f2(self.o(), q, port, delay, ctx, once) };
        if hr < 0 {
            drop(unsafe { Box::from_raw(ctx as *mut Once) });
        }
        hr
    }
    pub fn register_waiter(&self, q: usize, port: u32, handle: usize, f: impl Fn(bool) + Send + Sync + 'static) -> Result<u64, i32> {
        let g: unsafe extern "system" fn(*mut c_void, usize, u32, usize, *mut c_void, TaskCb, *mut u64) -> i32 = unsafe { core::mem::transmute(self.f(20)) };
        let ctx = leak::<Box<Many>>(Box::new(f));
        let mut token = 0u64;
        let hr = unsafe { g(self.o(), q, port, handle, ctx, many, &mut token) };
        if hr < 0 {
            Err(hr)
        } else {
            Ok(token)
        }
    }
    pub fn unregister_waiter(&self, q: usize, token: u64) {
        let g: unsafe extern "system" fn(*mut c_void, usize, u64) = unsafe { core::mem::transmute(self.f(21)) };
        unsafe { g(self.o(), q, token) }
    }
    pub fn terminate(&self, q: usize, wait: bool, f: Option<Box<dyn FnOnce() + Send>>) -> i32 {
        let g: unsafe extern "system" fn(*mut c_void, usize, bool, *mut c_void, Option<TermCb>) -> i32 = unsafe { core::mem::transmute(self.f(22)) };
        match f {
            Some(f) => unsafe { g(self.o(), q, wait, leak::<Term>(f), Some(term_tramp)) },
            None => unsafe { g(self.o(), q, wait, core::ptr::null_mut(), None) },
        }
    }
    pub fn register_monitor(&self, q: usize, f: impl Fn(usize, u32) + Send + Sync + 'static) -> u64 {
        let g: unsafe extern "system" fn(*mut c_void, usize, *mut c_void, MonitorCb, *mut u64) -> i32 = unsafe { core::mem::transmute(self.f(23)) };
        let ctx = leak::<Mon>(Box::new(f));
        let mut token = 0u64;
        assert_eq!(unsafe { g(self.o(), q, ctx, monitor_tramp, &mut token) }, 0);
        token
    }
    pub fn unregister_monitor(&self, q: usize, token: u64) {
        let g: unsafe extern "system" fn(*mut c_void, usize, u64) = unsafe { core::mem::transmute(self.f(24)) };
        unsafe { g(self.o(), q, token) }
    }
    pub fn process_queue(&self) -> (bool, usize) {
        let g: unsafe extern "system" fn(*mut c_void, *mut usize) -> bool = unsafe { core::mem::transmute(self.f(25)) };
        let mut q = 0usize;
        let ok = unsafe { g(self.o(), &mut q) };
        (ok, q)
    }
    pub fn set_process_queue(&self, q: usize) {
        let g: unsafe extern "system" fn(*mut c_void, usize) = unsafe { core::mem::transmute(self.f(26)) };
        unsafe { g(self.o(), q) }
    }

    pub fn status(&self, b: *mut Block, wait: bool) -> i32 {
        let g: unsafe extern "system" fn(*mut c_void, *mut Block, bool) -> i32 = unsafe { core::mem::transmute(self.f(3)) };
        unsafe { g(self.o(), b, wait) }
    }
    pub fn result_size(&self, b: *mut Block, out: &mut usize) -> i32 {
        let g: unsafe extern "system" fn(*mut c_void, *mut Block, *mut usize) -> i32 = unsafe { core::mem::transmute(self.f(4)) };
        unsafe { g(self.o(), b, out) }
    }
    pub fn cancel(&self, b: *mut Block) {
        let g: unsafe extern "system" fn(*mut c_void, *mut Block) = unsafe { core::mem::transmute(self.f(5)) };
        unsafe { g(self.o(), b) }
    }
    pub fn run(&self, b: *mut Block, w: Work) -> i32 {
        let g: unsafe extern "system" fn(*mut c_void, *mut Block, Work) -> i32 = unsafe { core::mem::transmute(self.f(6)) };
        unsafe { g(self.o(), b, w) }
    }
    pub fn begin(&self, b: *mut Block, ctx: usize, identity: usize, p: Provider) -> i32 {
        let g: unsafe extern "system" fn(*mut c_void, *mut Block, usize, usize, *const u8, Provider) -> i32 = unsafe { core::mem::transmute(self.f(7)) };
        unsafe { g(self.o(), b, ctx, identity, core::ptr::null(), p) }
    }
    pub fn schedule(&self, b: *mut Block, delay: u32) -> i32 {
        let g: unsafe extern "system" fn(*mut c_void, *mut Block, u32) -> i32 = unsafe { core::mem::transmute(self.f(9)) };
        unsafe { g(self.o(), b, delay) }
    }
    pub fn complete(&self, b: *mut Block, hr: i32, size: usize) {
        let g: unsafe extern "system" fn(*mut c_void, *mut Block, i32, usize) = unsafe { core::mem::transmute(self.f(10)) };
        unsafe { g(self.o(), b, hr, size) }
    }
    pub fn result(&self, b: *mut Block, identity: usize, size: usize, buf: *mut c_void, used: *mut usize) -> i32 {
        let g: unsafe extern "system" fn(*mut c_void, *mut Block, usize, usize, *mut c_void, *mut usize) -> i32 = unsafe { core::mem::transmute(self.f(11)) };
        unsafe { g(self.o(), b, identity, size, buf, used) }
    }
}

fn until(ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    f()
}

fn counter() -> Arc<AtomicU32> {
    Arc::new(AtomicU32::new(0))
}

fn bump(c: &Arc<AtomicU32>) -> impl FnOnce(bool) + Send + 'static {
    let c = c.clone();
    move |_| {
        c.fetch_add(1, SeqCst);
    }
}

fn drain(q: usize) {
    let a = api();
    while a.dispatch(q, WORK, 0) | a.dispatch(q, COMPLETION, 0) {}
}

// TaskQueueTests::VerifyStockQueue
pub fn verify_stock_queue() {
    let a = api();
    let q = a.create(THREAD_POOL, MANUAL);
    let (work, done) = (counter(), counter());
    let (w, d) = (work.clone(), bump(&done));
    assert_eq!(
        a.submit(q, WORK, 0, move |_| {
            w.fetch_add(1, SeqCst);
            assert_eq!(api().submit(q, COMPLETION, 0, d), 0);
        }),
        0
    );
    assert!(a.dispatch(q, COMPLETION, 5000), "completion submitted from a thread-pool work callback never reached the manual port");
    assert_eq!((work.load(SeqCst), done.load(SeqCst)), (1, 1));
    a.close(q);
}

// TaskQueueTests::VerifyCompositeQueue
pub fn verify_composite_queue() {
    let a = api();
    let q = a.create(THREAD_POOL, MANUAL);
    let calls = counter();
    let c = calls.clone();
    assert_eq!(
        a.submit(q, WORK, 0, move |_| {
            c.fetch_add(1, SeqCst);
            let a = api();
            let wp = a.port(q, WORK);
            let comp = a.composite(wp, wp);
            assert_eq!(a.submit(comp, WORK, 0, bump(&c)), 0);
            a.close(comp);
        }),
        0
    );
    assert!(until(1000, || calls.load(SeqCst) == 2), "work submitted to a composite over the work port did not run on the thread pool");
    assert!(!a.dispatch(q, COMPLETION, 0), "nothing may be queued on the completion port");
    a.close(q);
}

// TaskQueueTests::VerifyDuplicateQueueHandle
pub fn verify_duplicate_queue_handle() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let dups: Vec<usize> = (0..10).map(|_| a.dup(q)).collect();
    for d in &dups {
        assert_ne!(*d, q, "XTaskQueueDuplicateHandle must return a new handle");
    }
    // Every duplicate is a working reference to the same queue.
    let c = counter();
    assert_eq!(a.submit(dups[3], WORK, 0, bump(&c)), 0);
    assert!(a.dispatch(dups[7], WORK, 0));
    assert_eq!(c.load(SeqCst), 1);
    for d in dups {
        a.close(d);
    }
    let fake = Box::new([0usize; 8]);
    let mut out = 0usize;
    assert!(a.try_dup(fake.as_ptr() as usize, &mut out) < 0, "a fake handle must not duplicate");
    // The queue is still usable after closing every duplicate.
    assert_eq!(a.submit(q, WORK, 0, bump(&c)), 0);
    assert!(a.dispatch(q, WORK, 0));
    assert_eq!(c.load(SeqCst), 2);
    a.close(q);
}

// TaskQueueTests::VerifyDispatch
pub fn verify_dispatch() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let (work, comp) = (counter(), counter());
    for _ in 0..10 {
        assert_eq!(a.submit(q, WORK, 0, bump(&work)), 0);
    }
    for _ in 0..10 {
        assert_eq!(a.submit(q, COMPLETION, 0, bump(&comp)), 0);
    }
    let mut n = 0;
    while a.dispatch(q, WORK, 0) {
        n += 1;
    }
    assert_eq!((n, work.load(SeqCst), comp.load(SeqCst)), (10, 10, 0));
    n = 0;
    while a.dispatch(q, COMPLETION, 0) {
        n += 1;
    }
    assert_eq!((n, work.load(SeqCst), comp.load(SeqCst)), (10, 10, 10));
    a.close(q);

    // Manual work port and thread-pool completion port.
    let q = a.create(MANUAL, THREAD_POOL);
    let me = std::thread::current().id();
    let threads = Arc::new(Mutex::new(Vec::new()));
    for _ in 0..2 {
        let t = threads.clone();
        assert_eq!(a.submit(q, WORK, 0, move |_| t.lock().unwrap().push(("work", std::thread::current().id()))), 0);
    }
    assert!(a.dispatch(q, WORK, 0));
    assert_eq!(threads.lock().unwrap().len(), 1);
    assert!(a.dispatch(q, WORK, 0));
    assert_eq!(threads.lock().unwrap().len(), 2);
    assert!(threads.lock().unwrap().iter().all(|(_, id)| *id == me), "manual work must run on the dispatching thread");
    let t = threads.clone();
    assert_eq!(a.submit(q, COMPLETION, 0, move |_| t.lock().unwrap().push(("completion", std::thread::current().id()))), 0);
    assert!(until(1000, || threads.lock().unwrap().len() == 3));
    assert_ne!(threads.lock().unwrap()[2].1, me, "thread-pool completion ran on the test thread");

    // A thread-pool completion that hands work back to the manual port.
    threads.lock().unwrap().clear();
    let t = threads.clone();
    assert_eq!(
        a.submit(q, COMPLETION, 0, move |_| {
            let t2 = t.clone();
            assert_eq!(api().submit(q, WORK, 0, move |_| t2.lock().unwrap().push(("work", std::thread::current().id()))), 0);
        }),
        0
    );
    assert!(a.dispatch(q, WORK, 1000), "work handed off from the completion port did not wake the manual dispatcher");
    assert_eq!(threads.lock().unwrap()[0], ("work", me));
    a.close(q);
}

// TaskQueueTests::VerifySubmittedCallback
pub fn verify_submitted_callback() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let (w, c) = (counter(), counter());
    let (w2, c2) = (w.clone(), c.clone());
    let token = a.register_monitor(q, move |_, port| {
        if port == WORK {
            w2.fetch_add(1, SeqCst);
        } else {
            c2.fetch_add(1, SeqCst);
        }
    });
    for _ in 0..4 {
        assert_eq!(a.submit(q, WORK, 0, |_| {}), 0);
    }
    for _ in 0..7 {
        assert_eq!(a.submit(q, COMPLETION, 0, |_| {}), 0);
    }
    assert_eq!((w.load(SeqCst), c.load(SeqCst)), (4, 7), "monitor must fire synchronously on every submit");
    a.unregister_monitor(q, token);
    drain(q);
    a.close(q);
}

// TaskQueueTests::VerifySubmitCallbackWithWait
pub fn verify_submit_callback_with_wait() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    for port in [WORK, COMPLETION] {
        let base = Instant::now();
        let times = Arc::new(Mutex::new([None::<Duration>; 3]));
        for (i, delay) in [(0usize, 1000u32), (1, 0), (2, 500)] {
            let t = times.clone();
            assert_eq!(a.submit(q, port, delay, move |_| t.lock().unwrap()[i] = Some(base.elapsed())), 0);
        }
        assert!(a.dispatch(q, port, 0));
        assert!(!a.dispatch(q, port, 0), "a delayed callback was dispatchable before its delay");
        assert!(a.dispatch(q, port, 700), "500 ms callback did not wake a 700 ms dispatch");
        assert!(a.dispatch(q, port, 1200), "1000 ms callback did not wake a 1200 ms dispatch");
        assert!(!a.dispatch(q, port, 0));
        let t = *times.lock().unwrap();
        let ms = |d: Option<Duration>| d.expect("callback never ran").as_millis();
        assert!(ms(t[0]) >= 1000 && ms(t[0]) < 1600, "1000 ms callback at {} ms", ms(t[0]));
        assert!(ms(t[1]) < 500, "immediate callback at {} ms", ms(t[1]));
        assert!(ms(t[2]) >= 500 && ms(t[2]) < 1000, "500 ms callback at {} ms", ms(t[2]));
    }
    a.close(q);
}

// TaskQueueTests::VerifyRegisterCallbackSubmitted
pub fn verify_register_callback_submitted() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let calls: Vec<Arc<AtomicU32>> = (0..5).map(|_| counter()).collect();
    let tokens: Vec<u64> = calls
        .iter()
        .map(|c| {
            let c = c.clone();
            a.register_monitor(q, move |_, _| {
                c.fetch_add(1, SeqCst);
            })
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    assert!(tokens.iter().all(|t| seen.insert(*t)), "registration tokens must be unique: {tokens:?}");
    for _ in 0..3 {
        assert_eq!(a.submit(q, WORK, 0, |_| {}), 0);
    }
    assert!(calls.iter().all(|c| c.load(SeqCst) == 3));
    for i in (1..5).step_by(2) {
        a.unregister_monitor(q, tokens[i]);
    }
    for _ in 0..3 {
        assert_eq!(a.submit(q, WORK, 0, |_| {}), 0);
    }
    let got: Vec<u32> = calls.iter().map(|c| c.load(SeqCst)).collect();
    assert_eq!(got, vec![6, 3, 6, 3, 6]);
    drain(q);
    a.close(q);
}

// TaskQueueTests::VerifyImmediateDispatch
pub fn verify_immediate_dispatch() {
    let a = api();
    let q = a.create(MANUAL, IMMEDIATE);
    let c = counter();
    for i in 1..=10 {
        assert_eq!(a.submit(q, COMPLETION, 0, bump(&c)), 0);
        assert_eq!(c.load(SeqCst), i, "immediate port must run the callback inside the submit");
    }
    assert_eq!(a.submit(q, COMPLETION, 200, bump(&c)), 0);
    assert_eq!(c.load(SeqCst), 10, "a delayed callback on an immediate port ran early");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(c.load(SeqCst), 11, "a delayed callback on an immediate port never ran");
    a.close(q);
}

// TaskQueueTests::VerifySerializedThreadPoolDispatch
pub fn verify_serialized_thread_pool_dispatch() {
    let a = api();
    let q = a.create(THREAD_POOL, SERIALIZED);
    let order = Arc::new(Mutex::new(Vec::new()));
    let active = Arc::new(AtomicUsize::new(0));
    let overlap = Arc::new(AtomicBool::new(false));
    for i in 0..100u32 {
        let (o, act, ov) = (order.clone(), active.clone(), overlap.clone());
        assert_eq!(
            a.submit(q, COMPLETION, 0, move |_| {
                if act.fetch_add(1, SeqCst) != 0 {
                    ov.store(true, SeqCst);
                }
                o.lock().unwrap().push(i);
                std::thread::yield_now();
                act.fetch_sub(1, SeqCst);
            }),
            0
        );
    }
    assert!(until(2000, || order.lock().unwrap().len() == 100));
    assert!(!overlap.load(SeqCst), "serialized thread-pool callbacks overlapped");
    assert_eq!(*order.lock().unwrap(), (0..100).collect::<Vec<_>>(), "serialized thread-pool callbacks ran out of order");
    a.close(q);
}

// TaskQueueTests::VerifySerializedThreadPoolDoesNotStrandItems
pub fn verify_serialized_thread_pool_does_not_strand_items() {
    let a = api();
    let q = a.create(SERIALIZED, SERIALIZED);
    let ev = Arc::new(Event::new(false));
    for round in 0..5000 {
        let e = ev.clone();
        assert_eq!(a.submit(q, COMPLETION, 0, move |_| e.set()), 0);
        assert!(ev.wait(5000), "item {round} was not dispatched");
    }
    a.close(q);
}

// TaskQueueTests::VerifyMultithreadManualDispatch
pub fn verify_multithread_manual_dispatch() {
    let a = api();
    let q = a.create(MANUAL, IMMEDIATE);
    let ran = counter();
    let spawn = || {
        std::thread::spawn(move || {
            let a = api();
            while a.dispatch(q, WORK, INFINITE) {}
        })
    };
    let (t1, t2) = (spawn(), spawn());
    for _ in 0..5 {
        assert_eq!(a.submit(q, WORK, 0, bump(&ran)), 0);
    }
    std::thread::sleep(Duration::from_millis(750));
    assert!(!t1.is_finished() && !t2.is_finished(), "XTaskQueueDispatch(INFINITE) returned without termination");
    assert_eq!(ran.load(SeqCst), 5);

    let comp = a.composite(a.port(q, WORK), a.port(q, COMPLETION));
    for _ in 0..5 {
        assert_eq!(a.submit(comp, WORK, 0, bump(&ran)), 0);
    }
    assert_eq!(a.terminate(comp, true, None), 0);
    std::thread::sleep(Duration::from_millis(750));
    assert!(!t1.is_finished() && !t2.is_finished(), "terminating a composite stopped the base queue's dispatchers");

    assert_eq!(a.terminate(q, true, None), 0);
    assert!(until(750, || t1.is_finished() && t2.is_finished()), "terminating the queue did not release XTaskQueueDispatch(INFINITE)");
    t1.join().unwrap();
    t2.join().unwrap();
    a.close(comp);
    a.close(q);
}

// A manual queue whose ports are pumped by threads woken from a monitor
// callback (TaskQueueTests' PumpedTaskQueue helper).
pub struct Pumped {
    pub q: usize,
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    events: [Arc<Event>; 2],
}

impl Pumped {
    pub fn new() -> Pumped {
        let a = api();
        let q = a.create(MANUAL, MANUAL);
        let events = [Arc::new(Event::new(false)), Arc::new(Event::new(false))];
        let ev = events.clone();
        a.register_monitor(q, move |_, port| ev[port as usize].set());
        let stop = Arc::new(AtomicBool::new(false));
        let threads = (0..2u32)
            .map(|port| {
                let (e, s) = (events[port as usize].clone(), stop.clone());
                std::thread::spawn(move || loop {
                    e.wait(INFINITE);
                    if s.load(SeqCst) {
                        return;
                    }
                    while api().dispatch(q, port, 0) {}
                })
            })
            .collect();
        Pumped { q, stop, threads, events }
    }
}

impl Drop for Pumped {
    fn drop(&mut self) {
        self.stop.store(true, SeqCst);
        for e in &self.events {
            e.set();
        }
        for t in self.threads.drain(..) {
            t.join().unwrap();
        }
        api().close(self.q);
    }
}

fn register_with_auto_reset(q: usize) {
    let a = api();
    for port in [WORK, COMPLETION] {
        let wait = Event::new(false);
        let signaled = Arc::new(Event::new(false));
        let count = counter();
        let (s, c) = (signaled.clone(), count.clone());
        let token = a
            .register_waiter(q, port, wait.0, move |_| {
                c.fetch_add(1, SeqCst);
                s.set();
            })
            .expect("XTaskQueueRegisterWaiter");
        for i in 1..=5 {
            wait.set();
            assert!(signaled.wait(1000), "port {port}: waiter callback did not run");
            assert!(!signaled.wait(100), "port {port}: an auto-reset event ran the waiter twice");
            assert_eq!(count.load(SeqCst), i);
        }
        a.unregister_waiter(q, token);
    }
}

// TaskQueueTests::VerifyRegisterWithAutoReset
pub fn verify_register_with_auto_reset() {
    let a = api();
    let q = a.create(THREAD_POOL, THREAD_POOL);
    register_with_auto_reset(q);
    a.close(q);
    let pumped = Pumped::new();
    register_with_auto_reset(pumped.q);
}

// TaskQueueTests::VerifySharedPortQueueRundownKeepsOtherWaiters
pub fn verify_shared_port_queue_rundown_keeps_other_waiters() {
    let a = api();
    for terminate in [true, false] {
        let q = a.create(THREAD_POOL, THREAD_POOL);
        let signaled = Arc::new(Event::new(false));
        let wait = Event::new(false);
        let count = counter();
        let (s, c) = (signaled.clone(), count.clone());
        let token = a
            .register_waiter(q, WORK, wait.0, move |canceled| {
                if !canceled {
                    c.fetch_add(1, SeqCst);
                    s.set();
                }
            })
            .unwrap();
        wait.set();
        assert!(signaled.wait(1000));
        let wp = a.port(q, WORK);
        let sibling = a.composite(wp, wp);
        if terminate {
            assert_eq!(a.terminate(sibling, true, None), 0);
        }
        a.close(sibling);
        wait.set();
        assert!(signaled.wait(1000), "terminate={terminate}: a sibling's rundown broke this queue's waiter");
        assert_eq!(count.load(SeqCst), 2);
        a.unregister_waiter(q, token);
        wait.set();
        assert!(!signaled.wait(300), "terminate={terminate}: waiter fired after XTaskQueueUnregisterWaiter");
        assert_eq!(count.load(SeqCst), 2);
        a.close(q);
    }
}

// TaskQueueTests::VerifySharedPortQueueRundownDropsItsWaitHandles
pub fn verify_shared_port_queue_rundown_drops_its_wait_handles() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let wp = a.port(q, WORK);
    let sibling = a.composite(wp, wp);
    let wait = Event::new(true);
    a.register_waiter(sibling, WORK, wait.0, |_| {}).unwrap();
    a.close(sibling);
    wait.set();
    let returned = Arc::new(AtomicBool::new(false));
    let r = returned.clone();
    let t = std::thread::spawn(move || {
        api().dispatch(q, WORK, 100);
        r.store(true, SeqCst);
    });
    let ok = until(2000, || returned.load(SeqCst));
    wait.reset();
    t.join().unwrap();
    assert!(ok, "Dispatch(100 ms) kept spinning on a closed sibling's wait handle");
    a.close(q);
}

fn queue_termination(work_mode: u32, wait: bool, serialized: bool, empty: bool) {
    let a = api();
    let q = a.create(work_mode, work_mode);
    struct D {
        q: usize,
        work: AtomicU32,
        completion: AtomicU32,
        serialized: bool,
        failure: Mutex<Option<(u32, i32)>>,
    }
    let d = Arc::new(D { q, work: AtomicU32::new(0), completion: AtomicU32::new(0), serialized, failure: Mutex::new(None) });
    let fail = |d: &D, line: u32, hr: i32| {
        d.failure.lock().unwrap().get_or_insert((line, hr));
    };
    unsafe extern "system" fn completion_cb(ctx: *mut c_void, cancel: bool) {
        let d = &*(ctx as *const D);
        if d.serialized && !cancel {
            d.failure.lock().unwrap().get_or_insert((line!(), E_UNEXPECTED));
        }
        d.completion.fetch_add(1, SeqCst);
    }
    unsafe extern "system" fn work_cb(ctx: *mut c_void, cancel: bool) {
        let d = &*(ctx as *const D);
        let a = api();
        if d.serialized {
            if !cancel && d.work.load(SeqCst) != 0 {
                d.failure.lock().unwrap().get_or_insert((line!(), E_UNEXPECTED));
            }
            let start = Instant::now();
            while a.submit(d.q, WORK, 0, |_| {}) != E_ABORT {
                if start.elapsed() > Duration::from_secs(30) {
                    d.failure.lock().unwrap().get_or_insert((line!(), E_UNEXPECTED));
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        d.work.fetch_add(1, SeqCst);
        let hr = a.submit_raw(d.q, COMPLETION, 0, ctx, completion_cb);
        if hr < 0 {
            d.failure.lock().unwrap().get_or_insert((line!(), hr));
        }
    }
    let ctx = Arc::as_ptr(&d) as *mut c_void;
    let (normal, future, events) = if empty { (0, 0, 0) } else { (5, 5, 5) };
    for _ in 0..normal {
        assert_eq!(a.submit_raw(q, WORK, 0, ctx, work_cb), 0);
    }
    for _ in 0..future {
        assert_eq!(a.submit_raw(q, WORK, 10000, ctx, work_cb), 0);
    }
    let handles: Vec<Event> = (0..events).map(|_| Event::new(false)).collect();
    for h in &handles {
        let dd = d.clone();
        a.register_waiter(q, WORK, h.0, move |cancel| unsafe { work_cb(Arc::as_ptr(&dd) as *mut c_void, cancel) }).unwrap();
    }
    if wait {
        assert_eq!(a.terminate(q, true, None), 0);
    } else {
        let done = Arc::new(Event::new(true));
        let dn = done.clone();
        assert_eq!(a.terminate(q, false, Some(Box::new(move || dn.set()))), 0);
        assert!(done.wait(5000), "termination callback never ran");
    }
    assert_eq!(a.submit_raw(q, WORK, 0, ctx, work_cb), E_ABORT, "submit to a terminated work port");
    assert_eq!(a.submit_raw(q, COMPLETION, 0, ctx, completion_cb), E_ABORT, "submit to a terminated completion port");
    let expected = normal + future + events;
    if !wait {
        until(5000, || d.work.load(SeqCst) == expected && d.completion.load(SeqCst) == expected);
    }
    assert_eq!(*d.failure.lock().unwrap(), None);
    assert_eq!(d.work.load(SeqCst), expected, "every pending, delayed and waiter work item must run (canceled) before termination completes");
    assert_eq!(d.completion.load(SeqCst), expected);
    let _ = fail;
    a.close(q);
}

// TaskQueueTests::VerifyQueueTermination
pub fn verify_queue_termination() {
    queue_termination(THREAD_POOL, false, false, false);
    queue_termination(THREAD_POOL, true, false, false);
    queue_termination(SERIALIZED, true, true, false);
}

// TaskQueueTests::VerifyEmptyQueueTermination
pub fn verify_empty_queue_termination() {
    queue_termination(THREAD_POOL, false, false, true);
    queue_termination(THREAD_POOL, true, false, true);
    queue_termination(SERIALIZED, true, true, true);
}

// TaskQueueTests::VerifyGlobalQueue, AsyncBlockTests::VerifyGlobalQueueUsage and
// the first half of TaskQueueTests::VerifyGlobalQueueTermination. One test
// because they change the process-wide queue.
pub fn verify_global_queue() {
    let a = api();
    let (ok, global) = a.process_queue();
    assert!(ok);
    assert_ne!(global, 0);
    assert_eq!(a.submit(global, WORK, 0, |_| {}), 0);

    let ours = a.create(MANUAL, MANUAL);
    a.set_process_queue(ours);
    let (ok, q) = a.process_queue();
    assert!(ok);
    let c = counter();
    assert_eq!(a.submit(q, WORK, 0, bump(&c)), 0);
    assert!(a.dispatch(ours, WORK, 0), "the queue set with XTaskQueueSetCurrentProcessTaskQueue is not the process queue");
    assert_eq!(c.load(SeqCst), 1);
    a.close(q);
    a.set_process_queue(0);
    let (ok, q) = a.process_queue();
    assert!(!ok);
    assert_eq!(q, 0);

    // AsyncBlockTests::VerifyGlobalQueueUsage: a null XAsyncBlock queue uses
    // the process queue, and with no process queue XAsyncBegin fails.
    let mut b = Block::new(0);
    assert_eq!(a.begin(&mut *b, 0, 0, abort_on_cancel), E_NO_TASK_QUEUE, "XAsyncBegin with no process queue");
    a.set_process_queue(global);
    let mut b = Block::new(0);
    assert_eq!(a.begin(&mut *b, 0, 0, abort_on_cancel), 0);
    a.cancel(&mut *b);
    assert_eq!(a.status(&mut *b, true), E_ABORT);

    // TaskQueueTests::VerifyGlobalQueueTermination: a composite over the
    // process queue can be terminated without affecting it. libHttpClient and
    // the PC runtime then let the process queue itself be terminated; the
    // console runtime refuses with E_ACCESSDENIED and the queue stays usable.
    let p = a.port(global, WORK);
    let comp = a.composite(p, p);
    assert_eq!(a.terminate(comp, true, None), 0);
    a.close(comp);
    let ev = Arc::new(Event::new(false));
    let e = ev.clone();
    assert_eq!(a.submit(global, WORK, 0, move |_| e.set()), 0);
    assert!(ev.wait(5000));
    if console_model() {
        assert_eq!(a.terminate(global, true, None), E_ACCESSDENIED);
        assert_eq!(a.submit(global, WORK, 0, |_| {}), 0);
    } else {
        assert_eq!(a.terminate(global, true, None), 0);
        assert_eq!(a.submit(global, WORK, 0, |_| {}), E_ABORT);
    }
    a.close(global);
    a.close(ours);
}

// TaskQueueTests::VerifyCloseInTerminationForThreadPool
pub fn verify_close_in_termination_for_thread_pool() {
    let a = api();
    let q = a.create(THREAD_POOL, THREAD_POOL);
    let called = Arc::new(AtomicBool::new(false));
    let c = called.clone();
    assert_eq!(
        a.terminate(
            q,
            false,
            Some(Box::new(move || {
                api().close(q);
                c.store(true, SeqCst);
            }))
        ),
        0
    );
    assert!(until(5000, || called.load(SeqCst)));
}

// TaskQueueTests::VerifyCloseInTerminationForManual
pub fn verify_close_in_termination_for_manual() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let called = Arc::new(AtomicBool::new(false));
    let c = called.clone();
    assert_eq!(
        a.terminate(
            q,
            false,
            Some(Box::new(move || {
                c.store(true, SeqCst);
                api().close(q);
            }))
        ),
        0
    );
    assert!(!called.load(SeqCst), "a manual queue's termination callback must wait for dispatch");
    let end = Instant::now() + Duration::from_secs(5);
    while !called.load(SeqCst) && Instant::now() < end {
        a.dispatch(q, WORK, 0);
        if !called.load(SeqCst) {
            a.dispatch(q, COMPLETION, 0);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(called.load(SeqCst));
}

// TaskQueueTests::VerifyWaitTermination
pub fn verify_wait_termination() {
    let a = api();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        let q = a.create(THREAD_POOL, THREAD_POOL);
        assert_eq!(a.terminate(q, true, None), 0);
        a.close(q);
    }
}

// TaskQueueTests::VerifyManualDispatchAtTermination
pub fn verify_manual_dispatch_at_termination() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    a.register_monitor(q, |queue, port| {
        api().dispatch(queue, port, 0);
    });
    // Terminate(wait) on a manual queue completes only because the monitor
    // dispatches the termination items as they are queued.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(api().terminate(q, true, None)).unwrap());
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(0), "Terminate(wait) on a monitor-dispatched manual queue did not finish");
    a.close(q);
}

#[derive(Default)]
struct CompositeData {
    q: AtomicUsize,
    work: AtomicI32,
    completion: AtomicI32,
    future: AtomicI32,
    waiter: AtomicI32,
    terminated: AtomicBool,
}

fn record(slot: &AtomicI32, canceled: bool) {
    slot.store(1 + canceled as i32, SeqCst);
}

// TaskQueueTests::VerifyTerminationOfCompositeQueue
pub fn verify_termination_of_composite_queue() {
    let a = api();
    let data = Arc::new(CompositeData::default());
    let comp = Arc::new(CompositeData::default());
    let wait = Event::new(false);
    let cwait = Event::new(false);
    let q = a.create(MANUAL, MANUAL);
    data.q.store(q, SeqCst);
    let wp = a.port(q, WORK);
    let cq = a.composite(wp, wp);
    comp.q.store(cq, SeqCst);

    let submit_all = |d: &Arc<CompositeData>, ev: &Event| {
        let q = d.q.load(SeqCst);
        let d1 = d.clone();
        assert_eq!(
            a.submit(q, WORK, 0, move |canceled| {
                record(&d1.work, canceled);
                let d2 = d1.clone();
                assert_eq!(api().submit(d1.q.load(SeqCst), COMPLETION, 0, move |c| record(&d2.completion, c)), 0);
            }),
            0
        );
        let d1 = d.clone();
        assert_eq!(a.submit(q, WORK, 300, move |c| record(&d1.future, c)), 0);
        let d1 = d.clone();
        a.register_waiter(q, WORK, ev.0, move |c| record(&d1.waiter, c)).unwrap();
    };
    submit_all(&data, &wait);
    submit_all(&comp, &cwait);
    let c1 = comp.clone();
    assert_eq!(
        a.terminate(
            cq,
            false,
            Some(Box::new(move || {
                c1.terminated.store(true, SeqCst);
                api().close(c1.q.load(SeqCst));
            }))
        ),
        0
    );
    while a.dispatch(q, WORK, 500) | a.dispatch(q, COMPLETION, 500) {}

    // 1 = ran, 2 = ran canceled, 0 = never ran.
    assert_eq!(
        (data.work.load(SeqCst), data.completion.load(SeqCst), data.future.load(SeqCst), data.waiter.load(SeqCst)),
        (1, 1, 1, 0),
        "base queue callbacks must run normally"
    );
    assert_eq!(
        (comp.work.load(SeqCst), comp.completion.load(SeqCst), comp.future.load(SeqCst), comp.waiter.load(SeqCst)),
        (2, 2, 2, 2),
        "composite callbacks must all run canceled"
    );
    assert!(comp.terminated.load(SeqCst));

    data.work.store(0, SeqCst);
    data.waiter.store(0, SeqCst);
    comp.waiter.store(0, SeqCst);
    let d1 = data.clone();
    assert_eq!(a.submit(q, WORK, 0, move |c| record(&d1.work, c)), 0);
    wait.set();
    cwait.set();
    while a.dispatch(q, WORK, 500) | a.dispatch(q, COMPLETION, 500) {}
    assert_eq!(data.work.load(SeqCst), 1);
    assert_eq!(data.waiter.load(SeqCst), 1, "the base queue's waiter must survive the composite's termination");
    assert_eq!(comp.waiter.load(SeqCst), 0, "the terminated composite's waiter must be gone");
    a.close(q);
}

// Not a libHttpClient test. Terminating a queue ends that queue only: a composite queue built on
// its ports keeps its pending callback, still accepts submits, and new composites can be created
// on those ports. Measured on xgameruntime.dll 10.0.26100.9656.
pub fn verify_termination_of_base_queue_keeps_composites() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let (w, c) = (a.port(q, WORK), a.port(q, COMPLETION));
    let comp = a.composite(w, c);
    // 0: queued on the composite before the termination, 1: queued on the base queue, 2: queued
    // on the composite after it.
    let slots: Arc<[AtomicI32; 3]> = Arc::new(Default::default());
    let s = slots.clone();
    assert_eq!(a.submit(comp, WORK, 0, move |cn| record(&s[0], cn)), 0);
    let s = slots.clone();
    assert_eq!(a.submit(q, WORK, 0, move |cn| record(&s[1], cn)), 0);
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    assert_eq!(a.terminate(q, false, Some(Box::new(move || d.store(true, SeqCst)))), 0);
    let delivered = until(2000, || {
        a.dispatch(q, WORK, 0);
        a.dispatch(q, COMPLETION, 0);
        done.load(SeqCst)
    });
    assert!(delivered, "the termination callback was not delivered");
    // 1 = ran, 2 = ran canceled, 0 = never ran.
    assert_eq!(slots[0].load(SeqCst), 1, "the composite's pending callback must run normally");
    assert_eq!(slots[1].load(SeqCst), 2, "the terminated queue's pending callback must run canceled");
    assert_eq!(a.submit(q, WORK, 0, |_| {}), E_ABORT);
    let s = slots.clone();
    assert_eq!(a.submit(comp, WORK, 0, move |cn| record(&s[2], cn)), 0);
    assert_eq!(a.submit(comp, COMPLETION, 0, |_| {}), 0);
    assert_eq!(a.submit(comp, WORK, 10, |_| {}), 0);
    let mut comp2 = 0;
    assert_eq!(a.try_composite(w, c, &mut comp2), 0);
    assert!(until(2000, || {
        a.dispatch(comp, WORK, 0);
        a.dispatch(comp, COMPLETION, 0);
        slots[2].load(SeqCst) != 0
    }));
    assert_eq!(slots[2].load(SeqCst), 1, "a callback queued on the composite afterwards must run normally");
    std::thread::sleep(Duration::from_millis(30));
    drain(comp);
    a.close(comp2);
    a.close(comp);
    a.close(q);
}

// TaskQueueTests::VerifyQueueMonitorHasCorrectPorts
pub fn verify_queue_monitor_has_correct_ports() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let wp = a.port(q, WORK);
    let cq = a.composite(wp, wp);
    let qp = Arc::new(AtomicU32::new(u32::MAX));
    let cp = Arc::new(AtomicU32::new(u32::MAX));
    let (q1, c1) = (qp.clone(), cp.clone());
    a.register_monitor(q, move |_, port| q1.store(port, SeqCst));
    a.register_monitor(cq, move |_, port| c1.store(port, SeqCst));
    assert_eq!(a.submit(cq, COMPLETION, 0, |_| {}), 0);
    while a.dispatch(cq, COMPLETION, 100) {}
    assert_eq!(qp.load(SeqCst), WORK, "base queue monitor must report its own port for the shared port");
    assert_eq!(cp.load(SeqCst), COMPLETION);
    a.close(cq);
    a.close(q);
}

// TaskQueueTests::VerifyNestedQueueDispatchOrder
pub fn verify_nested_queue_dispatch_order() {
    let a = api();
    let nest = |q: usize| {
        let p = a.port(q, WORK);
        a.composite(p, p)
    };
    let q = a.create(MANUAL, MANUAL);
    let outer = nest(q);
    let inner = nest(outer);
    let step = Arc::new(Mutex::new(Vec::new()));
    let s = step.clone();
    assert_eq!(a.submit(outer, WORK, 3000, move |_| s.lock().unwrap().push("delayed outer")), 0);
    let s = step.clone();
    assert_eq!(a.submit(inner, WORK, 0, move |_| s.lock().unwrap().push("inner")), 0);
    a.dispatch(outer, WORK, 0);
    a.dispatch(outer, COMPLETION, 0);
    assert_eq!(*step.lock().unwrap(), ["inner"]);
    a.close(inner);
    a.dispatch(outer, WORK, 0);
    a.dispatch(outer, COMPLETION, 0);
    assert_eq!(*step.lock().unwrap(), ["inner"]);
    a.dispatch(outer, WORK, 4000);
    a.dispatch(outer, COMPLETION, 1000);
    assert_eq!(*step.lock().unwrap(), ["inner", "delayed outer"]);
    a.close(outer);
    a.close(q);
}

// TaskQueueTests::VerifyTerminateWaitsForSuspends. XTaskQueueSuspendTermination
// is not in the title-facing interface; an XAsync call in flight is how a
// title suspends termination (AsyncLib.cpp AllocStateNoCompletion), so this
// holds one open with a provider that only completes when told to.
pub fn verify_terminate_waits_for_async_in_flight() {
    let a = api();
    let q = a.create(THREAD_POOL, THREAD_POOL);
    let mut b = Block::new(q);
    assert_eq!(a.begin(&mut *b, 0, 0, nop_provider), 0);
    let done = Arc::new(Event::new(true));
    let d = done.clone();
    assert_eq!(a.terminate(q, false, Some(Box::new(move || d.set()))), 0);
    assert!(!done.wait(1000), "termination finished while an XAsync call on the queue was still pending");
    a.complete(&mut *b, 0, 0);
    assert!(done.wait(2000), "termination did not finish after the pending XAsync call completed");
    assert_eq!(a.status(&mut *b, true), 0);
    a.close(q);
}

// TaskQueueTests::VerifySingleQueueDelayedPollStrand (and the lost-wake
// regressions VerifyDelayedCallbackTimerRaceOnManualQueue /
// VerifyDelayedSubmissionRescuesOverdueTimer, whose test hooks are not
// reachable through the title interface): a self-rearming delayed poll on a
// manual port must keep advancing after a burst of 1 ms delayed callbacks.
pub fn verify_single_queue_delayed_poll_strand() {
    let a = api();
    for iter in 0..10 {
        let q = a.create(MANUAL, MANUAL);
        let pumping = Arc::new(AtomicBool::new(true));
        let p = pumping.clone();
        let pump = std::thread::spawn(move || {
            while p.load(SeqCst) {
                api().dispatch(q, WORK, 50);
            }
        });
        let keep = Arc::new(AtomicBool::new(true));
        let polls = counter();
        fn poll(q: usize, keep: Arc<AtomicBool>, polls: Arc<AtomicU32>) -> impl FnOnce(bool) + Send {
            move |canceled| {
                if canceled {
                    return;
                }
                polls.fetch_add(1, SeqCst);
                if keep.load(SeqCst) {
                    let (k, p) = (keep.clone(), polls.clone());
                    api().submit(q, WORK, 2, poll(q, k, p));
                }
            }
        }
        assert_eq!(a.submit(q, WORK, 2, poll(q, keep.clone(), polls.clone())), 0);
        assert!(until(1000, || polls.load(SeqCst) > 0));
        let outstanding = Arc::new(AtomicI32::new(0));
        let burst: Vec<_> = (0..4)
            .map(|_| {
                let o = outstanding.clone();
                std::thread::spawn(move || {
                    for _ in 0..250 {
                        o.fetch_add(1, SeqCst);
                        let o2 = o.clone();
                        if api().submit(q, WORK, 1, move |_| {
                            o2.fetch_sub(1, SeqCst);
                        }) < 0
                        {
                            o.fetch_sub(1, SeqCst);
                        }
                    }
                })
            })
            .collect();
        for t in burst {
            t.join().unwrap();
        }
        assert!(until(5000, || outstanding.load(SeqCst) == 0), "iteration {iter}: burst callbacks did not drain");
        let target = polls.load(SeqCst) + 30;
        let progressed = until(5000, || polls.load(SeqCst) >= target);
        keep.store(false, SeqCst);
        pumping.store(false, SeqCst);
        pump.join().unwrap();
        a.terminate(q, false, None);
        drain(q);
        a.close(q);
        assert!(progressed, "iteration {iter}: delayed poll loop stranded at {} polls", polls.load(SeqCst));
    }
}

// TaskQueueTests::VerifyCompositeQueueDelayedCallbackStarvation (the shared
// CompositeQueueStarvationScenario, fewer iterations).
pub fn verify_composite_queue_delayed_callback_starvation() {
    struct Req {
        comp: usize,
        target: u32,
        delay: u32,
        polls: AtomicU32,
        done: AtomicBool,
        term: AtomicBool,
        completed: Arc<AtomicU32>,
    }
    impl Req {
        fn mark(&self) {
            if !self.done.swap(true, SeqCst) {
                self.completed.fetch_add(1, SeqCst);
            }
        }
        fn terminate(&self) {
            if !self.term.swap(true, SeqCst) {
                api().terminate(self.comp, false, None);
            }
        }
    }
    unsafe extern "system" fn poll(ctx: *mut c_void, canceled: bool) {
        let r = &*(ctx as *const Req);
        if canceled {
            return r.mark();
        }
        let n = r.polls.fetch_add(1, SeqCst) + 1;
        if n < r.target {
            if api().submit_raw(r.comp, WORK, r.delay, ctx, poll) < 0 {
                r.mark();
            }
        } else {
            r.terminate();
            r.mark();
        }
    }
    let a = api();
    let (threads, per) = (6usize, 24usize);
    let total = threads * per;
    for iter in 0..5 {
        let q = a.create(MANUAL, MANUAL);
        let wp = a.port(q, WORK);
        let pumping = Arc::new(AtomicBool::new(true));
        let pumps: Vec<_> = (0..2)
            .map(|_| {
                let p = pumping.clone();
                std::thread::spawn(move || {
                    while p.load(SeqCst) {
                        api().dispatch(q, WORK, 50);
                    }
                })
            })
            .collect();
        let completed = Arc::new(AtomicU32::new(0));
        let slots: Arc<Vec<AtomicUsize>> = Arc::new((0..total).map(|_| AtomicUsize::new(0)).collect());
        let submitters: Vec<_> = (0..threads)
            .map(|t| {
                let (c, s) = (completed.clone(), slots.clone());
                std::thread::spawn(move || {
                    let a = api();
                    for i in 0..per {
                        let idx = t * per + i;
                        let r = Box::into_raw(Box::new(Req {
                            comp: a.composite(wp, wp),
                            target: if idx % 3 == 0 { u32::MAX } else { 20 },
                            delay: if i & 1 == 1 { 1 } else { 2 },
                            polls: AtomicU32::new(0),
                            done: AtomicBool::new(false),
                            term: AtomicBool::new(false),
                            completed: c.clone(),
                        }));
                        s[idx].store(r as usize, SeqCst);
                        if a.submit_raw(unsafe { (*r).comp }, WORK, 0, r as *mut c_void, poll) < 0 {
                            unsafe { (*r).mark() };
                        }
                    }
                })
            })
            .collect();
        let s = slots.clone();
        let canceller = std::thread::spawn(move || loop {
            let mut handled = 0;
            for i in 0..total {
                if i % 3 != 0 {
                    handled += 1;
                    continue;
                }
                let r = s[i].load(SeqCst) as *const Req;
                if !r.is_null() {
                    unsafe { (*r).terminate() };
                    handled += 1;
                }
            }
            if handled == total {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        });
        for t in submitters {
            t.join().unwrap();
        }
        canceller.join().unwrap();
        let ok = until(15000, || completed.load(SeqCst) as usize == total);
        pumping.store(false, SeqCst);
        for p in pumps {
            p.join().unwrap();
        }
        for slot in slots.iter() {
            let r = slot.load(SeqCst) as *const Req;
            unsafe {
                (*r).terminate();
                a.close((*r).comp);
            }
        }
        let fin = Arc::new(AtomicBool::new(false));
        let f = fin.clone();
        a.terminate(q, false, Some(Box::new(move || f.store(true, SeqCst))));
        let end = Instant::now() + Duration::from_secs(5);
        while !fin.load(SeqCst) && Instant::now() < end {
            a.dispatch(q, WORK, 10);
            a.dispatch(q, COMPLETION, 0);
        }
        assert!(fin.load(SeqCst), "iteration {iter}: queue termination stalled");
        a.close(q);
        assert!(ok, "iteration {iter}: starvation, {} of {total} poll loops finished", completed.load(SeqCst));
    }
}

// The delayed half of TaskQueueTests::VerifyQueueTermination (its 10 s
// "future" items): termination cancels delayed callbacks at once
// (TaskQueue.cpp CancelPendingEntries) and they run, canceled, before the
// termination callback instead of waiting out their delay.
pub fn verify_termination_cancels_delayed_callbacks() {
    let a = api();
    for mode in [THREAD_POOL, MANUAL] {
        let q = a.create(mode, mode);
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        assert_eq!(a.submit(q, WORK, 10000, move |canceled| l.lock().unwrap().push(if canceled { "delayed canceled" } else { "delayed ran" })), 0);
        let l = log.clone();
        assert_eq!(a.terminate(q, false, Some(Box::new(move || l.lock().unwrap().push("terminated")))), 0);
        let end = Instant::now() + Duration::from_secs(2);
        while !log.lock().unwrap().contains(&"terminated") && Instant::now() < end {
            if mode == MANUAL {
                a.dispatch(q, WORK, 10);
                a.dispatch(q, COMPLETION, 10);
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert_eq!(*log.lock().unwrap(), ["delayed canceled", "terminated"], "mode {mode}");
        a.close(q);
    }
}

// XTaskQueueCloseHandle: "The queue is not actually destroyed until all
// handles are closed and there are no outstanding callbacks enqueued"
// (XTaskQueue.h; TaskQueueTests::VerifyCompositeQueue and VerifyUninitialize
// close queues with work still queued or delayed). Closing is not
// terminating: queued and delayed callbacks still run, uncanceled.
pub fn verify_close_keeps_queued_callbacks() {
    let a = api();
    let q = a.create(SERIALIZED, SERIALIZED);
    let gate = Arc::new(Event::new(true));
    let (ran, canceled) = (counter(), counter());
    let g = gate.clone();
    assert_eq!(a.submit(q, WORK, 0, move |_| { g.wait(5000); }), 0);
    for delay in std::iter::repeat(0).take(20).chain([200]) {
        let (r, c) = (ran.clone(), canceled.clone());
        assert_eq!(
            a.submit(q, WORK, delay, move |cx| {
                if cx { c.fetch_add(1, SeqCst) } else { r.fetch_add(1, SeqCst) };
            }),
            0
        );
    }
    a.close(q);
    gate.set();
    assert!(until(3000, || ran.load(SeqCst) + canceled.load(SeqCst) == 21));
    assert_eq!((ran.load(SeqCst), canceled.load(SeqCst)), (21, 0), "closing the handle canceled queued callbacks");
}

// XTaskQueueDispatch dispatches the port whatever its mode (TaskQueue.cpp
// XTaskQueueDispatch; AsyncBlockTests::VerifyMultipleCalls dispatches a
// thread-pool completion port). Here the serialized drainer is held in its
// first callback, so only the dispatching thread can run the second.
pub fn verify_dispatch_on_pool_port() {
    let a = api();
    let q = a.create(MANUAL, SERIALIZED);
    let gate = Arc::new(Event::new(true));
    let started = Arc::new(Event::new(true));
    let (g, s) = (gate.clone(), started.clone());
    assert_eq!(
        a.submit(q, COMPLETION, 0, move |_| {
            s.set();
            g.wait(5000);
        }),
        0
    );
    assert!(started.wait(2000));
    let ran_on = Arc::new(Mutex::new(None));
    let r = ran_on.clone();
    assert_eq!(a.submit(q, COMPLETION, 0, move |_| *r.lock().unwrap() = Some(std::thread::current().id())), 0);
    assert!(a.dispatch(q, COMPLETION, 0), "dispatch on a thread-pool port with a queued item");
    assert_eq!(*ran_on.lock().unwrap(), Some(std::thread::current().id()));
    gate.set();
    a.close(q);
}

// Thread-pool ports run on the system thread pool (ThreadPool_win32.cpp), which
// adds threads when callbacks block, so callbacks may wait on each other.
pub fn verify_thread_pool_callbacks_can_block() {
    let a = api();
    let q = a.create(THREAD_POOL, THREAD_POOL);
    const N: u32 = 24;
    let pair = Arc::new((Mutex::new(0u32), std::sync::Condvar::new()));
    let met = counter();
    for _ in 0..N {
        let (p, m) = (pair.clone(), met.clone());
        assert_eq!(
            a.submit(q, WORK, 0, move |_| {
                let (lock, cv) = &*p;
                let mut n = lock.lock().unwrap();
                *n += 1;
                cv.notify_all();
                let (n, _) = cv.wait_timeout_while(n, Duration::from_secs(10), |n| *n < N).unwrap();
                if *n >= N {
                    m.fetch_add(1, SeqCst);
                }
            }),
            0
        );
    }
    assert!(until(12000, || met.load(SeqCst) == N), "{} thread-pool callbacks could not all run at once", N);
    a.close(q);
}

#[repr(C)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    tid: u32,
    pid: u32,
    base: i32,
    delta: i32,
    flags: u32,
}

extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> usize;
    fn Thread32First(snap: usize, e: *mut ThreadEntry) -> i32;
    fn Thread32Next(snap: usize, e: *mut ThreadEntry) -> i32;
    fn GetCurrentProcessId() -> u32;
}

fn thread_count() -> usize {
    unsafe {
        let snap = CreateToolhelp32Snapshot(4, 0);
        let mut e = ThreadEntry { size: 28, usage: 0, tid: 0, pid: 0, base: 0, delta: 0, flags: 0 };
        let me = GetCurrentProcessId();
        let mut n = 0;
        let mut ok = Thread32First(snap, &mut e);
        while ok != 0 {
            n += (e.pid == me) as usize;
            ok = Thread32Next(snap, &mut e);
        }
        CloseHandle(snap);
        n
    }
}

// Serialized ports share the system thread pool too; a closed queue leaves no
// thread behind. (A thread per port would leave 512 here; the margin absorbs
// threads other tests in this process start meanwhile.)
pub fn verify_serialized_queues_do_not_keep_threads() {
    let a = api();
    let before = thread_count();
    let ran = counter();
    for _ in 0..256 {
        let q = a.create(SERIALIZED, SERIALIZED);
        assert_eq!(a.submit(q, WORK, 0, bump(&ran)), 0);
        assert_eq!(a.submit(q, COMPLETION, 0, bump(&ran)), 0);
        a.close(q);
    }
    assert!(until(5000, || ran.load(SeqCst) == 512));
    std::thread::sleep(Duration::from_millis(200));
    let after = thread_count();
    assert!(after < before + 192, "threads went from {before} to {after} for 256 closed serialized queues");
}

// The four serialized-port scenarios of omonkeyz/xbox-emulator PR #28 (zerkend),
// re-expressed through the public interface: a port's users are the queue's
// handles, the completion marker is the termination callback.

// PR #28 "final release": once the last handle of a serialized queue that ran
// work is closed, its port keeps no worker alive.
pub fn verify_serialized_port_final_release() {
    let a = api();
    let before = thread_count();
    for _ in 0..64 {
        let q = a.create(SERIALIZED, SERIALIZED);
        let ev = Arc::new(Event::new(true));
        let e = ev.clone();
        assert_eq!(a.submit(q, WORK, 0, move |canceled| if !canceled { e.set() }), 0);
        assert!(ev.wait(5000), "serialized callback never ran");
        let d = a.dup(q);
        a.close(q);
        a.close(d);
    }
    std::thread::sleep(Duration::from_millis(200));
    let after = thread_count();
    assert!(after < before + 48, "threads went from {before} to {after} after releasing 64 serialized queues");
}

// PR #28 "shared ownership": closing one of two handles of a serialized queue
// leaves its port running for the other.
pub fn verify_serialized_port_survives_one_release() {
    let a = api();
    let q = a.create(SERIALIZED, SERIALIZED);
    let d = a.dup(q);
    let ev = Arc::new(Event::new(true));
    let e = ev.clone();
    assert_eq!(a.submit(q, WORK, 0, move |canceled| if !canceled { e.set() }), 0);
    assert!(ev.wait(5000));
    a.close(q);
    ev.reset();
    let got = Arc::new(Mutex::new(None));
    let (e, g) = (ev.clone(), got.clone());
    assert_eq!(a.submit(d, WORK, 0, move |canceled| { *g.lock().unwrap() = Some(canceled); e.set() }), 0);
    assert!(ev.wait(5000), "the remaining handle's serialized port stopped running");
    assert_eq!(*got.lock().unwrap(), Some(false), "a callback through the remaining handle was canceled");
    a.close(d);
}

// PR #28 "late completion marker": terminating a serialized queue whose port
// has gone idle still delivers the termination callback.
pub fn verify_serialized_idle_port_delivers_termination() {
    let a = api();
    let q = a.create(SERIALIZED, SERIALIZED);
    let ev = Arc::new(Event::new(true));
    let e = ev.clone();
    assert_eq!(a.submit(q, COMPLETION, 0, move |_| e.set()), 0);
    assert!(ev.wait(5000));
    std::thread::sleep(Duration::from_millis(50));
    let done = Arc::new(Event::new(true));
    let dn = done.clone();
    assert_eq!(a.terminate(q, false, Some(Box::new(move || dn.set()))), 0);
    assert!(done.wait(5000), "termination callback of an idle serialized queue never ran");
    a.close(q);
}

// PR #28 "cancellation and marker ordering": terminating a serialized queue
// while a callback runs cancels the queued one, and the termination callback
// waits for the running callback.
pub fn verify_serialized_termination_during_callback() {
    let a = api();
    let q = a.create(SERIALIZED, SERIALIZED);
    let order = Arc::new(Mutex::new(Vec::new()));
    let (started, gate) = (Arc::new(Event::new(true)), Arc::new(Event::new(true)));
    let (s, g, o) = (started.clone(), gate.clone(), order.clone());
    assert_eq!(a.submit(q, WORK, 0, move |_| { s.set(); g.wait(5000); o.lock().unwrap().push("first") }), 0);
    assert!(started.wait(5000));
    let o = order.clone();
    assert_eq!(a.submit(q, WORK, 0, move |canceled| o.lock().unwrap().push(if canceled { "second canceled" } else { "second ran" })), 0);
    let done = Arc::new(Event::new(true));
    let (dn, o) = (done.clone(), order.clone());
    assert_eq!(a.terminate(q, false, Some(Box::new(move || { o.lock().unwrap().push("terminated"); dn.set() }))), 0);
    assert!(!done.wait(100), "termination completed while a callback was still running");
    gate.set();
    assert!(done.wait(5000), "termination callback never ran");
    let order = order.lock().unwrap().clone();
    assert_eq!(order.len(), 3, "{order:?}");
    assert!(order.contains(&"first") && order.contains(&"second canceled"), "{order:?}");
    assert_eq!(order[2], "terminated", "the termination callback ran before the port's callbacks: {order:?}");
    a.close(q);
}

// ---- XAsync ----

const FACTORIAL_ID: usize = 0xFAC7;

#[derive(Default)]
pub struct Factorial {
    pub value: AtomicU32,
    pub result: AtomicU32,
    pub wait_ms: u32,
    pub work_thread: Mutex<Option<std::thread::ThreadId>>,
    pub ops: Mutex<Vec<u32>>,
    pub in_work: AtomicI32,
    pub canceled: AtomicBool,
    pub cleanup_in_work: AtomicBool,
}

impl Factorial {
    pub fn new(value: u32, wait_ms: u32) -> Arc<Factorial> {
        Arc::new(Factorial { value: AtomicU32::new(value), wait_ms, ..Default::default() })
    }
    fn ops(&self) -> Vec<u32> {
        self.ops.lock().unwrap().clone()
    }
    fn common(&self, op: u32, d: *const Data) -> Option<i32> {
        self.ops.lock().unwrap().push(op);
        match op {
            OP_CANCEL => self.canceled.store(true, SeqCst),
            OP_CLEANUP => {
                if self.in_work.load(SeqCst) != 0 {
                    self.cleanup_in_work.store(true, SeqCst);
                }
            }
            OP_GET_RESULT => unsafe { ((*d).buffer as *mut u32).write_unaligned(self.result.load(SeqCst)) },
            OP_DO_WORK => return None,
            _ => {}
        }
        Some(0)
    }
}

unsafe extern "system" fn factorial_simple(op: u32, d: *const Data) -> i32 {
    let f = &*((*d).context as *const Factorial);
    if let Some(hr) = f.common(op, d) {
        return hr;
    }
    f.in_work.fetch_add(1, SeqCst);
    *f.work_thread.lock().unwrap() = Some(std::thread::current().id());
    let mut r = 1u32;
    let mut v = f.value.load(SeqCst);
    while v != 0 {
        r = r.wrapping_mul(v);
        v -= 1;
    }
    f.result.store(r, SeqCst);
    if f.wait_ms != 0 {
        std::thread::sleep(Duration::from_millis(f.wait_ms as u64));
    }
    f.in_work.fetch_sub(1, SeqCst);
    api().complete((*d).block, if f.canceled.load(SeqCst) { E_ABORT } else { 0 }, 4);
    0
}

unsafe extern "system" fn factorial_distributed(op: u32, d: *const Data) -> i32 {
    let f = &*((*d).context as *const Factorial);
    if let Some(hr) = f.common(op, d) {
        return hr;
    }
    f.in_work.fetch_add(1, SeqCst);
    *f.work_thread.lock().unwrap() = Some(std::thread::current().id());
    if f.result.load(SeqCst) == 0 {
        f.result.store(1, SeqCst);
    }
    let v = f.value.load(SeqCst);
    if v != 0 {
        if f.canceled.load(SeqCst) {
            f.in_work.fetch_sub(1, SeqCst);
            return E_ABORT;
        }
        f.result.store(f.result.load(SeqCst) * v, SeqCst);
        f.value.store(v - 1, SeqCst);
        let hr = api().schedule((*d).block, f.wait_ms);
        f.in_work.fetch_sub(1, SeqCst);
        return if hr >= 0 { E_PENDING } else { hr };
    }
    f.in_work.fetch_sub(1, SeqCst);
    api().complete((*d).block, 0, 4);
    0
}

fn factorial_async(f: &Arc<Factorial>, b: &mut Block, distributed: bool) -> i32 {
    let a = api();
    let p: Provider = if distributed { factorial_distributed } else { factorial_simple };
    let hr = a.begin(b, Arc::as_ptr(f) as usize, FACTORIAL_ID, p);
    if hr < 0 {
        return hr;
    }
    a.schedule(b, 0)
}

fn factorial_result(b: *mut Block, out: &mut u32) -> i32 {
    let mut used = 0usize;
    let hr = api().result(b, FACTORIAL_ID, 4, out as *mut u32 as *mut c_void, &mut used);
    if hr >= 0 {
        assert_eq!(used, 4);
    }
    hr
}

// Completion callback whose context is a leaked Box<dyn Fn(*mut Block)>.
type Done = Box<dyn Fn(*mut Block) + Send + Sync>;
unsafe extern "system" fn done_tramp(b: *mut Block) {
    (*((*b).context as *const Done))(b)
}
fn on_done(b: &mut Block, f: impl Fn(*mut Block) + Send + Sync + 'static) {
    b.context = leak::<Done>(Box::new(f)) as usize;
    b.callback = Some(done_tramp);
}

unsafe extern "system" fn nop_provider(op: u32, d: *const Data) -> i32 {
    let _ = (op, d);
    0
}

unsafe extern "system" fn abort_on_cancel(op: u32, d: *const Data) -> i32 {
    if op == OP_CANCEL {
        api().complete((*d).block, E_ABORT, 0);
    }
    0
}

fn thread_pool_queue() -> usize {
    api().create(THREAD_POOL, THREAD_POOL)
}

fn settle(q: usize) {
    // Lets queued cleanup run on thread-pool ports.
    let _ = q;
    std::thread::sleep(Duration::from_millis(50));
}

// AsyncBlockTests::VerifySimpleAsyncCall
pub fn verify_simple_async_call() {
    let a = api();
    let q = thread_pool_queue();
    let f = Factorial::new(5, 0);
    let mut b = Block::new(q);
    let result = Arc::new(AtomicI32::new(-1));
    let r = result.clone();
    on_done(&mut b, move |b| {
        let mut v = 0;
        let hr = factorial_result(b, &mut v);
        r.store(if hr >= 0 { v as i32 } else { hr }, SeqCst);
    });
    assert_eq!(factorial_async(&f, &mut b, false), 0);
    assert_eq!(a.status(&mut *b, true), 0);
    assert_eq!(result.load(SeqCst), 120, "XAsyncGetStatus(wait) returned before the completion callback ran");
    settle(q);
    assert_eq!(f.ops(), vec![OP_BEGIN, OP_DO_WORK, OP_GET_RESULT, OP_CLEANUP]);
    a.close(q);
}

// AsyncBlockTests::VerifyMultipleCalls
pub fn verify_multiple_calls() {
    let a = api();
    let q = thread_pool_queue();
    let done = counter();
    let mut blocks = Vec::new();
    for i in 0..10u32 {
        let f = Factorial::new(5 * (i + 1), 0);
        let mut b = Block::new(q);
        let (d, f2) = (done.clone(), f.clone());
        on_done(&mut b, move |b| {
            let mut v = 0;
            if factorial_result(b, &mut v) >= 0 && v == f2.result.load(SeqCst) {
                d.fetch_add(1, SeqCst);
            }
        });
        assert_eq!(factorial_async(&f, &mut b, false), 0);
        blocks.push((b, f));
    }
    assert!(until(5000, || done.load(SeqCst) == 10));
    a.close(q);
}

// AsyncBlockTests::VerifyDistributedAsyncCall
pub fn verify_distributed_async_call() {
    let a = api();
    let q = thread_pool_queue();
    let f = Factorial::new(5, 100);
    let mut b = Block::new(q);
    let result = Arc::new(AtomicI32::new(-1));
    let r = result.clone();
    on_done(&mut b, move |b| {
        let mut v = 0;
        let hr = factorial_result(b, &mut v);
        r.store(if hr >= 0 { v as i32 } else { hr }, SeqCst);
    });
    let start = Instant::now();
    assert_eq!(factorial_async(&f, &mut b, true), 0);
    assert_eq!(a.status(&mut *b, true), 0);
    let took = start.elapsed();
    assert_eq!(result.load(SeqCst), 120);
    assert!(took >= Duration::from_millis(400), "rescheduled DoWork ignored its delay ({took:?})");
    settle(q);
    let mut ops = vec![OP_BEGIN];
    ops.extend([OP_DO_WORK; 6]);
    ops.extend([OP_GET_RESULT, OP_CLEANUP]);
    assert_eq!(f.ops(), ops);
    a.close(q);
}

// AsyncBlockTests::VerifyAsyncBlockReuse
pub fn verify_async_block_reuse() {
    let a = api();
    let q = thread_pool_queue();
    let f = Factorial::new(5, 0);
    let mut b = Box::new(Block { queue: q, context: 0, callback: None, internal: [0xCD; 32] });
    let result = Arc::new(AtomicI32::new(-1));
    let r = result.clone();
    on_done(&mut b, move |b| {
        let mut v = 0;
        let hr = factorial_result(b, &mut v);
        r.store(if hr >= 0 { v as i32 } else { hr }, SeqCst);
    });
    assert_eq!(factorial_async(&f, &mut b, false), 0);
    assert_eq!(a.status(&mut *b, true), 0);
    assert_eq!(result.load(SeqCst), 120);
    b.callback = None;
    f.value.store(6, SeqCst);
    assert_eq!(factorial_async(&f, &mut b, false), 0);
    assert_eq!(factorial_async(&f, &mut b, false), E_INVALIDARG, "an async block in use must not be reusable");
    assert_eq!(a.status(&mut *b, true), 0);
    let mut v = 0;
    assert_eq!(factorial_result(&mut *b, &mut v), 0);
    assert_eq!(v, 720);
    a.close(q);
}

// AsyncBlockTests::VerifyCancellation
pub fn verify_cancellation() {
    let a = api();
    let q = thread_pool_queue();
    let f = Factorial::new(5, 100);
    let mut b = Block::new(q);
    let in_cb = Arc::new(AtomicI32::new(1));
    let c = in_cb.clone();
    on_done(&mut b, move |b| c.store(api().status(b, false), SeqCst));
    assert_eq!(factorial_async(&f, &mut b, true), 0);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(a.status(&mut *b, false), E_PENDING);
    a.cancel(&mut *b);
    assert_eq!(a.status(&mut *b, true), E_ABORT);
    assert_eq!(in_cb.load(SeqCst), E_ABORT);
    settle(q);
    let ops = f.ops();
    assert!(ops.contains(&OP_CANCEL) && ops.contains(&OP_CLEANUP), "{ops:?}");
    a.close(q);
}

fn cleanup_waits_for_work(distributed: bool) {
    let a = api();
    let q = thread_pool_queue();
    let f = Factorial::new(5, 500);
    let mut b = Block::new(q);
    let in_cb = Arc::new(AtomicI32::new(1));
    let c = in_cb.clone();
    on_done(&mut b, move |b| {
        let mut v = 0;
        c.store(factorial_result(b, &mut v), SeqCst)
    });
    assert_eq!(factorial_async(&f, &mut b, distributed), 0);
    std::thread::sleep(Duration::from_millis(if distributed { 700 } else { 50 }));
    assert_eq!(a.status(&mut *b, false), E_PENDING);
    a.cancel(&mut *b);
    assert_eq!(a.status(&mut *b, true), E_ABORT);
    assert_eq!(in_cb.load(SeqCst), E_ABORT);
    assert!(until(2000, || f.ops().contains(&OP_CLEANUP)));
    let ops = f.ops();
    assert!(ops.contains(&OP_CANCEL) && ops.contains(&OP_DO_WORK), "{ops:?}");
    assert!(!f.cleanup_in_work.load(SeqCst), "XAsyncOp::Cleanup ran while DoWork was still running: {ops:?}");
    a.close(q);
}

// AsyncBlockTests::VerifyCleanupWaitsForWork
pub fn verify_cleanup_waits_for_work() {
    cleanup_waits_for_work(false);
}

// AsyncBlockTests::VerifyCleanupWaitsForWorkDistributed
pub fn verify_cleanup_waits_for_work_distributed() {
    cleanup_waits_for_work(true);
}

static RUN_EXPECTED: AtomicI32 = AtomicI32::new(0);
unsafe extern "system" fn run_work(_b: *mut Block) -> i32 {
    RUN_EXPECTED.load(SeqCst)
}

// AsyncBlockTests::VerifyRunAsync
pub fn verify_run_async() {
    let a = api();
    let q = thread_pool_queue();
    RUN_EXPECTED.store(0x12345678, SeqCst);
    let mut b = Block::new(q);
    assert_eq!(a.run(&mut *b, run_work), 0);
    assert_eq!(a.status(&mut *b, true), 0x12345678);
    a.close(q);
}

// AsyncBlockTests::VerifyCustomQueue
pub fn verify_custom_queue() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let f = Factorial::new(5, 0);
    let mut b = Block::new(q);
    let seen = Arc::new(Mutex::new(None));
    let s = seen.clone();
    on_done(&mut b, move |b| {
        let mut v = 0;
        let hr = factorial_result(b, &mut v);
        *s.lock().unwrap() = Some((hr, v, std::thread::current().id()));
    });
    assert_eq!(factorial_async(&f, &mut b, false), 0);
    assert!(a.dispatch(q, WORK, 100), "XAsyncSchedule did not post DoWork to the work port");
    assert_eq!(f.result.load(SeqCst), 120);
    assert_eq!(*f.work_thread.lock().unwrap(), Some(std::thread::current().id()));
    assert!(seen.lock().unwrap().is_none(), "completion ran before its port was dispatched");
    assert!(a.dispatch(q, COMPLETION, 100), "completion was not posted to the completion port");
    assert_eq!(*seen.lock().unwrap(), Some((0, 120, std::thread::current().id())));
    assert!(!a.dispatch(q, WORK, 0) && !a.dispatch(q, COMPLETION, 0));
    a.close(q);
}

// AsyncBlockTests::VerifyCantScheduleTwice
pub fn verify_cant_schedule_twice() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    let f = Factorial::new(5, 0);
    let mut b = Block::new(q);
    assert_eq!(a.begin(&mut *b, Arc::as_ptr(&f) as usize, 0, factorial_simple), 0);
    assert_eq!(a.schedule(&mut *b, 0), 0);
    assert_eq!(a.schedule(&mut *b, 0), E_UNEXPECTED);
    a.cancel(&mut *b);
    drain(q);
    assert_ne!(a.status(&mut *b, false), E_PENDING);
    a.close(q);
}

// AsyncBlockTests::VerifyWaitForCompletion
pub fn verify_wait_for_completion() {
    let a = api();
    let q = thread_pool_queue();
    let f = Factorial::new(5, 0);
    let mut b = Block::new(q);
    let finished = Arc::new(AtomicBool::new(false));
    let fin = finished.clone();
    on_done(&mut b, move |b| {
        std::thread::sleep(Duration::from_millis(300));
        let mut v = 0;
        if factorial_result(b, &mut v) >= 0 && v == 120 {
            fin.store(true, SeqCst);
        }
    });
    assert_eq!(factorial_async(&f, &mut b, false), 0);
    assert_eq!(a.status(&mut *b, true), 0);
    assert!(finished.load(SeqCst), "XAsyncGetStatus(wait) must wait for the completion callback to finish");
    a.close(q);
}

// AsyncBlockTests::VerifyPeriodicPattern
pub fn verify_periodic_pattern() {
    let a = api();
    let q = thread_pool_queue();
    struct Ctl {
        q: usize,
        enabled: AtomicBool,
        count: AtomicU32,
    }
    unsafe extern "system" fn provider(op: u32, d: *const Data) -> i32 {
        if op == OP_DO_WORK {
            api().complete((*d).block, 0, 0);
        }
        0
    }
    unsafe extern "system" fn done(b: *mut Block) {
        let c = &*((*b).context as *const Ctl);
        c.count.fetch_add(1, SeqCst);
        if c.enabled.load(SeqCst) {
            schedule(c);
        }
        drop(Box::from_raw(b));
    }
    fn schedule(c: &Ctl) {
        let b = Box::into_raw(Box::new(Block { queue: c.q, context: c as *const Ctl as usize, callback: Some(done), internal: [0; 32] }));
        assert_eq!(api().begin(b, 0, 0, provider), 0);
        assert_eq!(api().schedule(b, 30), 0);
    }
    let c = Arc::new(Ctl { q, enabled: AtomicBool::new(true), count: AtomicU32::new(0) });
    schedule(&c);
    assert!(until(10000, || c.count.load(SeqCst) >= 10));
    c.enabled.store(false, SeqCst);
    // The last scheduled call still references `c`; let it finish first.
    assert_eq!(a.terminate(q, true, None), 0);
    a.close(q);
}

unsafe extern "system" fn zero_work(_b: *mut Block) -> i32 {
    0
}

// AsyncBlockTests::VerifyRunAlotAsync
pub fn verify_run_alot_async() {
    let a = api();
    let q = thread_pool_queue();
    let mut blocks: Vec<Block> = (0..20000).map(|_| *Block::new(q)).collect();
    for b in blocks.iter_mut() {
        assert_eq!(a.run(b, zero_work), 0);
    }
    for b in blocks.iter_mut() {
        assert_eq!(a.status(b, true), 0);
    }
    a.close(q);
}

unsafe extern "system" fn short_work(_b: *mut Block) -> i32 {
    std::thread::sleep(Duration::from_millis(1));
    0
}

// AsyncBlockTests::VerifyGetAsyncStatusNoDeadlock
pub fn verify_get_async_status_no_deadlock() {
    let a = api();
    let q = thread_pool_queue();
    let mut b = Block::new(q);
    for _ in 0..300 {
        assert_eq!(a.run(&mut *b, short_work), 0);
        while a.status(&mut *b, false) == E_PENDING {
            std::thread::yield_now();
        }
    }
    a.close(q);
}

// AsyncBlockTests::VerifyFailedBeginCompletes
pub fn verify_failed_begin_completes() {
    let a = api();
    let q = thread_pool_queue();
    unsafe extern "system" fn fail(op: u32, _d: *const Data) -> i32 {
        if op == OP_BEGIN {
            E_FAIL
        } else {
            E_UNEXPECTED
        }
    }
    let mut b = Block::new(q);
    let called = Arc::new(AtomicI32::new(1));
    let c = called.clone();
    on_done(&mut b, move |b| c.store(api().status(b, false), SeqCst));
    assert_eq!(a.begin(&mut *b, 0, 0, fail), 0, "XAsyncBegin succeeds even when the Begin op fails");
    assert_eq!(a.status(&mut *b, true), E_FAIL);
    // The call ended inside XAsyncBegin with no payload, so the block was
    // already released and the wait above does not cover the callback.
    assert!(until(2000, || called.load(SeqCst) == E_FAIL), "a failed Begin still completes through the callback");
    a.close(q);
}

// AsyncBlockTests::VerifyZeroPayloadCleansUpFast
pub fn verify_zero_payload_cleans_up_fast() {
    let a = api();
    let q = a.create(MANUAL, MANUAL);
    unsafe extern "system" fn empty(op: u32, d: *const Data) -> i32 {
        match op {
            OP_BEGIN => {
                assert_eq!(api().schedule((*d).block, 0), 0);
            }
            OP_DO_WORK => api().complete((*d).block, 0, 0),
            OP_CLEANUP => {
                (*((*d).context as *const AtomicU32)).fetch_add(1, SeqCst);
            }
            _ => {}
        }
        0
    }
    let cleanups = AtomicU32::new(0);
    let mut b = Block::new(q);
    assert_eq!(a.begin(&mut *b, &cleanups as *const AtomicU32 as usize, 0, empty), 0);
    assert!(a.dispatch(q, WORK, 0));
    assert_eq!(cleanups.load(SeqCst), 1, "a zero-payload call must clean up when DoWork completes it");
    drain(q);
    assert_eq!(cleanups.load(SeqCst), 1, "cleanup ran twice");
    a.close(q);
}

// AsyncBlockTests::VerifyCompleteInBegin
pub fn verify_complete_in_begin() {
    let a = api();
    let q = thread_pool_queue();
    unsafe extern "system" fn provider(op: u32, d: *const Data) -> i32 {
        if op == OP_BEGIN {
            api().complete((*d).block, 0, 0);
        }
        0
    }
    let ev = Arc::new(Event::new(true));
    let mut b = Block::new(q);
    let e = ev.clone();
    on_done(&mut b, move |_| e.set());
    assert_eq!(a.begin(&mut *b, 0, 0, provider), 0);
    assert_eq!(a.status(&mut *b, true), 0);
    assert!(ev.wait(2500));
    a.close(q);
}

// AsyncBlockTests::VerifyBeginAfterTerminate
pub fn verify_begin_after_terminate() {
    let a = api();
    let q = thread_pool_queue();
    assert_eq!(a.terminate(q, true, None), 0);
    let mut b = Block::new(q);
    assert_eq!(a.begin(&mut *b, 0, 0, nop_provider), E_ABORT);
    a.close(q);
}

// AsyncBlockTests::VerifyFailureInDoWork, plus the S_OK-without-complete case
// that AsyncLib.cpp WorkerCallback turns into E_UNEXPECTED.
pub fn verify_failure_in_do_work() {
    let a = api();
    let q = thread_pool_queue();
    unsafe extern "system" fn special(op: u32, d: *const Data) -> i32 {
        match op {
            OP_BEGIN => api().schedule((*d).block, 0),
            OP_DO_WORK => 0x8009_ABCDu32 as i32,
            _ => 0,
        }
    }
    unsafe extern "system" fn forgets(op: u32, d: *const Data) -> i32 {
        match op {
            OP_BEGIN => api().schedule((*d).block, 0),
            _ => 0,
        }
    }
    let mut b = Block::new(q);
    assert_eq!(a.begin(&mut *b, 0, 0, special), 0);
    assert_eq!(a.status(&mut *b, true), 0x8009_ABCDu32 as i32);
    let mut b = Block::new(q);
    assert_eq!(a.begin(&mut *b, 0, 0, forgets), 0);
    assert_eq!(a.status(&mut *b, true), E_UNEXPECTED, "DoWork returning S_OK without XAsyncComplete");
    a.close(q);
}

// AsyncBlockTests::VerifyDuplicateResultCallsFail
pub fn verify_duplicate_result_calls_fail() {
    let a = api();
    let q = thread_pool_queue();
    let f = Factorial::new(5, 0);
    let mut b = Block::new(q);
    assert_eq!(factorial_async(&f, &mut b, false), 0);
    assert_eq!(a.status(&mut *b, true), 0);
    let mut v = 0;
    assert_eq!(factorial_result(&mut *b, &mut v), 0);
    assert_eq!(factorial_result(&mut *b, &mut v), E_ILLEGAL_METHOD_CALL);
    a.close(q);
}

// AsyncBlockTests::VerifyDuplicatePendingResultCallsSucceed
pub fn verify_duplicate_pending_result_calls_succeed() {
    let a = api();
    let q = thread_pool_queue();
    struct Ctx {
        complete: Event,
        waiting: Event,
    }
    unsafe extern "system" fn provider(op: u32, d: *const Data) -> i32 {
        match op {
            OP_BEGIN => api().schedule((*d).block, 0),
            OP_DO_WORK => {
                let c = &*((*(*d).block).context as *const Ctx);
                c.waiting.set();
                c.complete.wait(INFINITE);
                api().complete((*d).block, 0, 0);
                0
            }
            _ => 0,
        }
    }
    let ctx = Ctx { complete: Event::new(true), waiting: Event::new(true) };
    let mut b = Block::new(q);
    b.context = &ctx as *const Ctx as usize;
    assert_eq!(a.begin(&mut *b, 0, 0, provider), 0);
    assert!(ctx.waiting.wait(2000));
    for _ in 0..10 {
        assert_eq!(a.result(&mut *b, 0, 0, core::ptr::null_mut(), core::ptr::null_mut()), E_PENDING);
    }
    ctx.complete.set();
    assert_eq!(a.status(&mut *b, true), 0);
    assert_eq!(a.result(&mut *b, 0, 0, core::ptr::null_mut(), core::ptr::null_mut()), 0);
    assert_eq!(a.result(&mut *b, 0, 0, core::ptr::null_mut(), core::ptr::null_mut()), 0, "a zero-payload result stays readable");
    a.close(q);
}

// AsyncBlockTests::VerifyNoWriteAfterGetStatusRace
pub fn verify_no_write_after_get_status_race() {
    let a = api();
    let q = thread_pool_queue();
    struct Ctx {
        m: Mutex<(usize, bool, bool, bool)>, // block, cancel requested, complete returned, stop
        cv: std::sync::Condvar,
    }
    unsafe extern "system" fn provider(op: u32, d: *const Data) -> i32 {
        if op == OP_CANCEL {
            let c = &*((*d).context as *const Ctx);
            c.m.lock().unwrap().1 = true;
            c.cv.notify_all();
        }
        0
    }
    let ctx = Arc::new(Ctx { m: Mutex::new((0, false, false, false)), cv: std::sync::Condvar::new() });
    let c = ctx.clone();
    let worker = std::thread::spawn(move || loop {
        let ctx = &*c;
        let block = {
            let mut g = ctx.cv.wait_while(ctx.m.lock().unwrap(), |g| !g.3 && !g.1).unwrap();
            if g.3 {
                return;
            }
            g.1 = false;
            g.0
        };
        api().complete(block as *mut Block, E_ABORT, 0);
        ctx.m.lock().unwrap().2 = true;
        ctx.cv.notify_all();
    });
    for i in 0..2000 {
        let mut b = Block::new(q);
        *ctx.m.lock().unwrap() = (&mut *b as *mut Block as usize, false, false, false);
        assert_eq!(a.begin(&mut *b, Arc::as_ptr(&ctx) as usize, 0, provider), 0);
        a.cancel(&mut *b);
        assert_eq!(a.status(&mut *b, true), E_ABORT);
        let snap = b.bytes();
        drop(ctx.cv.wait_while(ctx.m.lock().unwrap(), |g| !g.2).unwrap());
        assert_eq!(snap, b.bytes(), "iteration {i}: the async block was written after XAsyncGetStatus(wait) returned");
    }
    ctx.m.lock().unwrap().3 = true;
    ctx.cv.notify_all();
    worker.join().unwrap();
    a.close(q);
}

// AsyncLib.cpp XAsyncGetResult / XAsyncGetResultSize argument checks (no
// dedicated libHttpClient test): a null buffer is E_INVALIDARG, a short one is
// E_NOT_SUFFICIENT_BUFFER, and a failed call reports size 0.
pub fn verify_result_buffer_checks() {
    let a = api();
    let q = thread_pool_queue();
    let f = Factorial::new(5, 0);
    let mut b = Block::new(q);
    assert_eq!(factorial_async(&f, &mut b, false), 0);
    assert_eq!(a.status(&mut *b, true), 0);
    let mut size = 0usize;
    assert_eq!(a.result_size(&mut *b, &mut size), 0);
    assert_eq!(size, 4);
    let mut used = 0usize;
    assert_eq!(a.result(&mut *b, FACTORIAL_ID, 4, core::ptr::null_mut(), &mut used), E_INVALIDARG);

    let f = Factorial::new(5, 0);
    let mut b = Block::new(q);
    assert_eq!(factorial_async(&f, &mut b, false), 0);
    assert_eq!(a.status(&mut *b, true), 0);
    let mut v = 0u16;
    assert_eq!(a.result(&mut *b, FACTORIAL_ID, 2, &mut v as *mut u16 as *mut c_void, &mut used), E_NOT_SUFFICIENT_BUFFER);

    unsafe extern "system" fn failing(op: u32, d: *const Data) -> i32 {
        match op {
            OP_BEGIN => api().schedule((*d).block, 0),
            OP_DO_WORK => {
                api().complete((*d).block, E_FAIL, 16);
                0
            }
            _ => 0,
        }
    }
    let mut b = Block::new(q);
    assert_eq!(a.begin(&mut *b, 0, 0, failing), 0);
    assert_eq!(a.status(&mut *b, true), E_FAIL);
    let mut size = 99usize;
    assert_eq!(a.result_size(&mut *b, &mut size), E_FAIL);
    assert_eq!(size, 0);
    a.close(q);
}

// Every scenario, as one #[test] each.

macro_rules! xtq_tests {
    ($setup:path) => {
        xtq_tests!(@ $setup;
            verify_stock_queue, verify_composite_queue, verify_duplicate_queue_handle, verify_dispatch,
            verify_submitted_callback, verify_submit_callback_with_wait, verify_register_callback_submitted,
            verify_immediate_dispatch, verify_serialized_thread_pool_dispatch,
            verify_serialized_thread_pool_does_not_strand_items, verify_multithread_manual_dispatch,
            verify_register_with_auto_reset, verify_shared_port_queue_rundown_keeps_other_waiters,
            verify_shared_port_queue_rundown_drops_its_wait_handles, verify_queue_termination,
            verify_empty_queue_termination, verify_global_queue, verify_close_in_termination_for_thread_pool,
            verify_close_in_termination_for_manual, verify_wait_termination, verify_manual_dispatch_at_termination,
            verify_termination_of_composite_queue, verify_termination_of_base_queue_keeps_composites,
            verify_queue_monitor_has_correct_ports,
            verify_nested_queue_dispatch_order, verify_terminate_waits_for_async_in_flight,
            verify_single_queue_delayed_poll_strand, verify_composite_queue_delayed_callback_starvation,
            verify_termination_cancels_delayed_callbacks, verify_close_keeps_queued_callbacks,
            verify_dispatch_on_pool_port, verify_thread_pool_callbacks_can_block,
            verify_serialized_queues_do_not_keep_threads, verify_serialized_port_final_release,
            verify_serialized_port_survives_one_release, verify_serialized_idle_port_delivers_termination,
            verify_serialized_termination_during_callback,
            verify_simple_async_call, verify_multiple_calls, verify_distributed_async_call, verify_async_block_reuse,
            verify_cancellation, verify_cleanup_waits_for_work, verify_cleanup_waits_for_work_distributed,
            verify_run_async, verify_custom_queue, verify_cant_schedule_twice, verify_wait_for_completion,
            verify_periodic_pattern, verify_run_alot_async, verify_get_async_status_no_deadlock,
            verify_failed_begin_completes, verify_zero_payload_cleans_up_fast, verify_complete_in_begin,
            verify_begin_after_terminate, verify_failure_in_do_work, verify_duplicate_result_calls_fail,
            verify_duplicate_pending_result_calls_succeed, verify_no_write_after_get_status_race,
            verify_result_buffer_checks);
    };
    (@ $setup:path; $($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                if $setup(stringify!($name)) {
                    xtq::$name();
                }
            }
        )*
    };
}
