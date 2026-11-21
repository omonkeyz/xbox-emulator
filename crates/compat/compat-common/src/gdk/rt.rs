use super::xasync::{ProviderData, XASYNC};
use super::{log, setting, Iface, This, E_ABORT, E_INVALIDARG, E_NOT_SUFFICIENT_BUFFER, E_PENDING, S_OK};
use core::ffi::c_void;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub type Job = Box<dyn FnOnce(bool) + Send>;
type TaskCb = unsafe extern "system" fn(*mut c_void, bool);
type AsyncWork = unsafe extern "system" fn(*mut c_void) -> i32;
type Provider = unsafe extern "system" fn(u32, *const ProviderData) -> i32;
type Completion = unsafe extern "system" fn(*mut c_void);
type TerminatedCb = unsafe extern "system" fn(*mut c_void);

pub const MANUAL: u32 = 0;
pub const THREAD_POOL: u32 = 1;
pub const SERIALIZED: u32 = 2;
pub const IMMEDIATE: u32 = 3;

pub fn active() -> bool {
    static A: OnceLock<bool> = OnceLock::new();
    *A.get_or_init(|| {
        let forced = setting("gdk_xasync").map(|s| s.to_ascii_lowercase());
        let a = match forced.as_deref() {
            Some("real") => unsafe { !real_xasync_ok() },
            _ => true,
        };
        log("xasync", if a { "XAsync/XTaskQueue: emulated implementation" } else { "XAsync/XTaskQueue: Gaming Runtime implementation" });
        a
    })
}

unsafe fn real_xasync_ok() -> bool {
    let Some(q) = super::real_query() else { return false };
    let mut out: *mut c_void = core::ptr::null_mut();
    q(&XASYNC, &XASYNC, &mut out) >= 0 && !out.is_null()
}

struct Pool {
    q: Mutex<VecDeque<Job>>,
    cv: Condvar,
    started: AtomicUsize,
}

fn pool() -> &'static Pool {
    static P: OnceLock<Pool> = OnceLock::new();
    let p = P.get_or_init(|| Pool { q: Mutex::new(VecDeque::new()), cv: Condvar::new(), started: AtomicUsize::new(0) });
    let want = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    while p.started.load(Ordering::Acquire) < want {
        if p.started.fetch_add(1, Ordering::AcqRel) >= want {
            break;
        }
        std::thread::spawn(|| {
            let p = pool();
            loop {
                let job = {
                    let mut q = p.q.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if let Some(j) = q.pop_front() {
                            break j;
                        }
                        q = p.cv.wait(q).unwrap_or_else(|e| e.into_inner());
                    }
                };
                job(false);
            }
        });
    }
    p
}

pub struct Port {
    mode: u32,
    items: Mutex<VecDeque<Job>>,
    cv: Condvar,
    terminated: AtomicBool,
    serial_started: AtomicBool,
    users: AtomicUsize,
}

impl Port {
    fn new(mode: u32) -> Arc<Port> {
        Arc::new(Port { mode, items: Mutex::new(VecDeque::new()), cv: Condvar::new(), terminated: AtomicBool::new(false), serial_started: AtomicBool::new(false), users: AtomicUsize::new(0) })
    }

    fn submit(self: &Arc<Port>, job: Job) {
        if self.terminated.load(Ordering::Acquire) {
            job(true);
            return;
        }
        match self.mode {
            IMMEDIATE => job(false),
            THREAD_POOL => {
                let p = pool();
                p.q.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
                p.cv.notify_one();
            }
            SERIALIZED => {
                self.items.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
                self.cv.notify_all();
                self.ensure_serial();
            }
            _ => {
                self.items.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
                self.cv.notify_all();
            }
        }
    }

    fn ensure_serial(self: &Arc<Port>) {
        if self.serial_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || loop {
            let job = {
                let mut q = me.items.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(j) = q.pop_front() {
                        break Some(j);
                    }
                    if me.users.load(Ordering::Acquire) == 0 {
                        // Termination can still enqueue completion markers. Reset
                        // under the queue lock so a later marker can start a worker.
                        me.serial_started.store(false, Ordering::Release);
                        break None;
                    }
                    q = me.cv.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            let Some(job) = job else { return };
            job(me.terminated.load(Ordering::Acquire));
        });
    }

    fn submit_marker(self: &Arc<Port>, job: Job) {
        match self.mode {
            IMMEDIATE => job(false),
            THREAD_POOL => {
                let p = pool();
                p.q.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
                p.cv.notify_one();
            }
            _ => {
                self.items.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
                self.cv.notify_all();
                if self.mode == SERIALIZED {
                    self.ensure_serial();
                } else if self.users.load(Ordering::Acquire) == 0 {
                    let drained: Vec<Job> = self.items.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
                    for j in drained {
                        j(true);
                    }
                }
            }
        }
    }

    fn dispatch(&self, timeout_ms: u32) -> bool {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let job = {
            let mut q = self.items.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(j) = q.pop_front() {
                    break Some(j);
                }
                let now = Instant::now();
                if timeout_ms == 0 || now >= deadline {
                    break None;
                }
                let wait = if timeout_ms == u32::MAX { Duration::from_millis(100) } else { deadline - now };
                q = self.cv.wait_timeout(q, wait).unwrap_or_else(|e| e.into_inner()).0;
            }
        };
        match job {
            Some(j) => {
                j(self.terminated.load(Ordering::Acquire));
                true
            }
            None => false,
        }
    }

    fn release(&self) {
        if self.users.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.cancel_all();
        }
    }

    fn cancel_all(&self) {
        self.terminated.store(true, Ordering::Release);
        let drained: Vec<Job> = self.items.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        for j in drained {
            j(true);
        }
        self.cv.notify_all();
    }
}

pub struct Queue {
    magic: u64,
    work: Arc<Port>,
    completion: Arc<Port>,
    refs: AtomicUsize,
    process: bool,
    owns: bool,
    ended: Arc<AtomicBool>,
}

const QMAGIC: u64 = 0x5442_5851_5545_5545;

fn port_handles() -> &'static Mutex<HashMap<usize, Arc<Port>>> {
    static M: OnceLock<Mutex<HashMap<usize, Arc<Port>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn port_handle(p: &Arc<Port>) -> usize {
    let k = Arc::as_ptr(p) as usize;
    port_handles().lock().unwrap_or_else(|e| e.into_inner()).entry(k).or_insert_with(|| p.clone());
    k
}

fn new_queue(work: Arc<Port>, completion: Arc<Port>, process: bool, owns: bool) -> usize {
    work.users.fetch_add(1, Ordering::AcqRel);
    completion.users.fetch_add(1, Ordering::AcqRel);
    Box::into_raw(Box::new(Queue { magic: QMAGIC, work, completion, refs: AtomicUsize::new(1), process, owns, ended: Arc::new(AtomicBool::new(false)) })) as usize
}

fn process_queue() -> usize {
    static P: AtomicUsize = AtomicUsize::new(0);
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| P.store(new_queue(Port::new(THREAD_POOL), Port::new(THREAD_POOL), true, true), Ordering::Release));
    let o = OVERRIDE.load(Ordering::Acquire);
    if o != 0 { o } else { P.load(Ordering::Acquire) }
}

static OVERRIDE: AtomicUsize = AtomicUsize::new(0);

unsafe fn queue<'a>(h: usize) -> Option<&'a Queue> {
    let h = if h == 0 { process_queue() } else { h };
    let q = (h as *const Queue).as_ref()?;
    if q.magic == QMAGIC { Some(q) } else { None }
}

unsafe fn port_of(h: usize, port: u32) -> Option<Arc<Port>> {
    let q = queue(h)?;
    Some(if port == 0 { q.work.clone() } else { q.completion.clone() })
}

unsafe fn queued(h: usize, port: u32, job: Job) -> Option<(Arc<Port>, Job)> {
    let q = queue(h)?;
    let p = if port == 0 { q.work.clone() } else { q.completion.clone() };
    if q.ended.load(Ordering::Acquire) {
        job(true);
        return Some((p, Box::new(|_| {})));
    }
    let ended = q.ended.clone();
    Some((p, Box::new(move |canceled| job(canceled || ended.load(Ordering::Acquire)))))
}

pub unsafe fn submit(queue_h: usize, port: u32, job: Job) -> i32 {
    match queued(queue_h, port, job) {
        Some((p, j)) => {
            p.submit(j);
            S_OK
        }
        None => E_INVALIDARG,
    }
}

fn timers() -> &'static (Mutex<Vec<(Instant, usize, Arc<Port>, Option<Job>)>>, Condvar) {
    static T: OnceLock<(Mutex<Vec<(Instant, usize, Arc<Port>, Option<Job>)>>, Condvar)> = OnceLock::new();
    static START: std::sync::Once = std::sync::Once::new();
    let t = T.get_or_init(|| (Mutex::new(Vec::new()), Condvar::new()));
    START.call_once(|| {
        std::thread::spawn(|| {
            let (m, cv) = timers();
            loop {
                let mut due: Vec<(Arc<Port>, Job)> = Vec::new();
                {
                    let mut v = m.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        let now = Instant::now();
                        let mut i = 0;
                        while i < v.len() {
                            if v[i].0 <= now {
                                let (_, _, p, j) = v.swap_remove(i);
                                if let Some(j) = j {
                                    due.push((p, j));
                                }
                            } else {
                                i += 1;
                            }
                        }
                        if !due.is_empty() {
                            break;
                        }
                        let next = v.iter().map(|x| x.0).min();
                        v = match next {
                            Some(n) => cv.wait_timeout(v, n.saturating_duration_since(Instant::now())).unwrap_or_else(|e| e.into_inner()).0,
                            None => cv.wait(v).unwrap_or_else(|e| e.into_inner()),
                        };
                    }
                }
                for (p, j) in due {
                    p.submit(j);
                }
            }
        });
    });
    t
}

pub unsafe fn submit_delayed(queue_h: usize, port: u32, delay_ms: u32, job: Job) -> i32 {
    let Some((p, job)) = queued(queue_h, port, job) else { return E_INVALIDARG };
    if delay_ms == 0 {
        p.submit(job);
        return S_OK;
    }
    let (m, cv) = timers();
    m.lock().unwrap_or_else(|e| e.into_inner()).push((Instant::now() + Duration::from_millis(delay_ms as u64), queue_h, p, Some(job)));
    cv.notify_all();
    S_OK
}

struct SendPtr(usize);
unsafe impl Send for SendPtr {}

fn callback_job(ctx: *mut c_void, cb: TaskCb) -> Job {
    let c = SendPtr(ctx as usize);
    Box::new(move |canceled| unsafe { cb(c.0 as *mut c_void, canceled) })
}

unsafe extern "system" fn q_create(_t: This, work: u32, completion: u32, out: *mut usize) -> i32 {
    if out.is_null() || work > 3 || completion > 3 {
        return E_INVALIDARG;
    }
    *out = new_queue(Port::new(work), Port::new(completion), false, true);
    S_OK
}

unsafe extern "system" fn q_get_port(_t: This, q: usize, port: u32, out: *mut usize) -> i32 {
    if out.is_null() || port > 1 {
        return E_INVALIDARG;
    }
    match port_of(q, port) {
        Some(p) => {
            *out = port_handle(&p);
            S_OK
        }
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn q_create_composite(_t: This, work: usize, completion: usize, out: *mut usize) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = port_handles().lock().unwrap_or_else(|e| e.into_inner());
    let (Some(w), Some(c)) = (m.get(&work).cloned(), m.get(&completion).cloned()) else { return E_INVALIDARG };
    drop(m);
    *out = new_queue(w, c, false, false);
    S_OK
}

unsafe extern "system" fn q_duplicate(_t: This, q: usize, out: *mut usize) -> i32 {
    let Some(qq) = queue(q) else { return E_INVALIDARG };
    if out.is_null() {
        return E_INVALIDARG;
    }
    qq.refs.fetch_add(1, Ordering::AcqRel);
    *out = if q == 0 { process_queue() } else { q };
    S_OK
}

unsafe extern "system" fn q_dispatch(_t: This, q: usize, port: u32, timeout: u32) -> bool {
    match port_of(q, port) {
        Some(p) if p.mode == MANUAL => p.dispatch(timeout),
        _ => false,
    }
}

unsafe extern "system" fn q_close(_t: This, q: usize) {
    if let Some(qq) = queue(q) {
        if !qq.process && qq.refs.fetch_sub(1, Ordering::AcqRel) == 1 {
            qq.work.release();
            qq.completion.release();
        }
    }
}

unsafe extern "system" fn q_submit(_t: This, q: usize, port: u32, ctx: *mut c_void, cb: Option<TaskCb>) -> i32 {
    let Some(cb) = cb else { return E_INVALIDARG };
    submit(q, port, callback_job(ctx, cb))
}

unsafe extern "system" fn q_submit_delayed(_t: This, q: usize, port: u32, delay: u32, ctx: *mut c_void, cb: Option<TaskCb>) -> i32 {
    let Some(cb) = cb else { return E_INVALIDARG };
    submit_delayed(q, port, delay, callback_job(ctx, cb))
}

unsafe extern "system" fn q_terminate(_t: This, q: usize, wait: bool, ctx: *mut c_void, cb: Option<TerminatedCb>) -> i32 {
    let Some(qq) = queue(q) else { return E_INVALIDARG };
    if qq.process {
        return E_INVALIDARG;
    }
    qq.ended.store(true, Ordering::Release);
    if qq.owns {
        qq.work.cancel_all();
        qq.completion.cancel_all();
    }
    let c = SendPtr(ctx as usize);
    let done = move || {
        if let Some(cb) = cb {
            cb(c.0 as *mut c_void);
        }
    };
    if wait {
        done();
        return S_OK;
    }
    let completion = qq.completion.clone();
    let deliver: Job = Box::new(move |_| done());
    let forward: Job = Box::new(move |_| completion.submit_marker(deliver));
    qq.work.submit_marker(forward);
    S_OK
}

unsafe extern "system" fn q_get_process(_t: This, out: *mut usize) -> bool {
    if out.is_null() {
        return false;
    }
    let p = process_queue();
    if let Some(q) = queue(p) {
        q.refs.fetch_add(1, Ordering::AcqRel);
    }
    *out = p;
    true
}

thread_local! {
    static TIME_SENSITIVE: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "system" fn thread_set_time_sensitive(_t: This, v: bool) -> i32 {
    TIME_SENSITIVE.with(|c| c.set(v));
    S_OK
}

unsafe extern "system" fn thread_is_time_sensitive(_t: This) -> bool {
    TIME_SENSITIVE.with(|c| c.get())
}

unsafe extern "system" fn thread_assert_not_time_sensitive(_t: This) {
    if TIME_SENSITIVE.with(|c| c.get()) {
        log("XThreadAssertNotTimeSensitive", "a time-sensitive thread called a blocking API");
    }
}

#[repr(C)]
pub struct Block {
    pub queue: usize,
    pub context: *mut c_void,
    pub callback: Option<Completion>,
    pub internal: [u8; 32],
}

const BMAGIC: u32 = 0x4158_4254;

struct AState {
    provider: Provider,
    ctx: usize,
    identity: usize,
    status: Mutex<(i32, usize)>,
    cv: Condvar,
    cleaned: AtomicBool,
    queue: usize,
    canceled: AtomicBool,
    data: Box<core::cell::UnsafeCell<ProviderData>>,
}

unsafe impl Send for AState {}
unsafe impl Sync for AState {}

fn states() -> &'static Mutex<HashMap<usize, Arc<AState>>> {
    static M: OnceLock<Mutex<HashMap<usize, Arc<AState>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn state(block: usize) -> Option<Arc<AState>> {
    states().lock().unwrap_or_else(|e| e.into_inner()).get(&block).cloned()
}

unsafe fn mark(block: *mut Block, hr: i32, live: bool) {
    let i = &mut (*block).internal;
    i[0..4].copy_from_slice(&BMAGIC.to_le_bytes());
    i[4..8].copy_from_slice(&hr.to_le_bytes());
    i[8] = live as u8;
    i[9] = 0;
}

unsafe fn marked(block: *const Block) -> Option<i32> {
    let i = &(*block).internal;
    if u32::from_le_bytes(i[0..4].try_into().ok()?) == BMAGIC {
        Some(i32::from_le_bytes(i[4..8].try_into().ok()?))
    } else {
        None
    }
}

unsafe fn call(s: &AState, op: u32, block: *mut Block, size: usize, buf: *mut c_void) -> i32 {
    let d = s.data.get();
    (*d).block = block as *mut c_void;
    (*d).buffer_size = size;
    (*d).buffer = buf as *mut u8;
    (*d).context = s.ctx as *mut c_void;
    (s.provider)(op, d)
}

unsafe fn cleanup(s: &AState, block: *mut Block) {
    if !s.cleaned.swap(true, Ordering::AcqRel) {
        call(s, 4, block, 0, core::ptr::null_mut());
    }
}

pub unsafe fn begin(block: *mut Block, ctx: *mut c_void, identity: usize, _name: *const u8, provider: Provider) -> i32 {
    if block.is_null() {
        return E_INVALIDARG;
    }
    if let Some(s) = state(block as usize) {
        if s.status.lock().unwrap_or_else(|e| e.into_inner()).0 == E_PENDING {
            return E_INVALIDARG;
        }
    }
    let s = Arc::new(AState {
        provider,
        ctx: ctx as usize,
        identity,
        status: Mutex::new((E_PENDING, 0)),
        cv: Condvar::new(),
        cleaned: AtomicBool::new(false),
        queue: (*block).queue,
        canceled: AtomicBool::new(false),
        data: Box::new(core::cell::UnsafeCell::new(ProviderData { block: block as *mut c_void, buffer_size: 0, buffer: core::ptr::null_mut(), context: ctx })),
    });
    states().lock().unwrap_or_else(|e| e.into_inner()).insert(block as usize, s.clone());
    mark(block, E_PENDING, true);
    let hr = call(&s, 0, block, 0, core::ptr::null_mut());
    if hr < 0 {
        states().lock().unwrap_or_else(|e| e.into_inner()).remove(&(block as usize));
        mark(block, hr, false);
        return hr;
    }
    S_OK
}

pub unsafe fn schedule(block: *mut Block, delay: u32) -> i32 {
    let Some(s) = state(block as usize) else { return E_INVALIDARG };
    let b = SendPtr(block as usize);
    let job: Job = Box::new(move |canceled| {
        let block = b.0 as *mut Block;
        let Some(s) = state(block as usize) else { return };
        if canceled || s.canceled.load(Ordering::Acquire) {
            complete(block, E_ABORT, 0);
            return;
        }
        let hr = call(&s, 1, block, 0, core::ptr::null_mut());
        if hr != E_PENDING {
            complete(block, hr, 0);
        }
    });
    submit_delayed(s.queue, 0, delay, job)
}

pub unsafe fn complete(block: *mut Block, hr: i32, size: usize) {
    if hr == E_PENDING {
        return;
    }
    let Some(s) = state(block as usize) else { return };
    {
        let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
        if st.0 != E_PENDING {
            return;
        }
        *st = (hr, size);
    }
    let keep = hr >= 0 && size > 0;
    if !keep {
        cleanup(&s, block);
    }
    mark(block, hr, keep);
    let notify = {
        let s = s.clone();
        let b = SendPtr(block as usize);
        move || {
            let block = b.0 as *mut Block;
            if !keep {
                states().lock().unwrap_or_else(|e| e.into_inner()).remove(&(block as usize));
            }
            s.cv.notify_all();
        }
    };
    match (*block).callback {
        Some(cb) => {
            let b = SendPtr(block as usize);
            let job: Job = Box::new(move |_| {
                cb(b.0 as *mut c_void);
                notify();
            });
            if submit(s.queue, 1, job) < 0 {
                s.cv.notify_all();
            }
        }
        None => notify(),
    }
}

pub unsafe fn get_status(block: *mut Block, wait: bool) -> i32 {
    if block.is_null() {
        return E_INVALIDARG;
    }
    match state(block as usize) {
        Some(s) => {
            let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
            while wait && st.0 == E_PENDING {
                st = s.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            st.0
        }
        None => marked(block).unwrap_or(E_INVALIDARG),
    }
}

pub unsafe fn get_result_size(block: *mut Block, out: *mut usize) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match state(block as usize) {
        Some(s) => {
            let st = *s.status.lock().unwrap_or_else(|e| e.into_inner());
            if st.0 < 0 || st.0 == E_PENDING {
                return st.0;
            }
            *out = st.1;
            S_OK
        }
        None => {
            let hr = marked(block).unwrap_or(E_INVALIDARG);
            if hr >= 0 {
                *out = 0;
            }
            hr
        }
    }
}

pub unsafe fn get_result(block: *mut Block, identity: usize, size: usize, buf: *mut c_void, used: *mut usize) -> i32 {
    let Some(s) = state(block as usize) else {
        let hr = marked(block).unwrap_or(E_INVALIDARG);
        if hr >= 0 && (*block).internal[9] == 1 {
            return 0x8000_000Eu32 as i32;
        }
        if hr >= 0 && !used.is_null() {
            *used = 0;
        }
        return hr;
    };
    if identity != 0 && s.identity != 0 && identity != s.identity {
        return E_INVALIDARG;
    }
    let (hr, need) = *s.status.lock().unwrap_or_else(|e| e.into_inner());
    if hr < 0 || hr == E_PENDING {
        return hr;
    }
    if size < need || (buf.is_null() && need > 0) {
        return E_NOT_SUFFICIENT_BUFFER;
    }
    let r = if need > 0 { call(&s, 2, block, need, buf) } else { S_OK };
    if !used.is_null() {
        *used = need;
    }
    cleanup(&s, block);
    states().lock().unwrap_or_else(|e| e.into_inner()).remove(&(block as usize));
    mark(block, hr, false);
    (*block).internal[9] = (need > 0 && r >= 0) as u8;
    r
}

pub unsafe fn cancel(block: *mut Block) {
    let Some(s) = state(block as usize) else { return };
    if s.status.lock().unwrap_or_else(|e| e.into_inner()).0 != E_PENDING {
        return;
    }
    s.canceled.store(true, Ordering::Release);
    call(&s, 3, block, 0, core::ptr::null_mut());
    complete(block, E_ABORT, 0);
}

unsafe extern "system" fn run_provider(op: u32, d: *const ProviderData) -> i32 {
    if op == 1 {
        let w: AsyncWork = core::mem::transmute((*d).context);
        return w((*d).block);
    }
    S_OK
}

unsafe extern "system" fn a_get_status(_t: This, b: *mut Block, wait: bool) -> i32 {
    get_status(b, wait)
}
unsafe extern "system" fn a_get_result_size(_t: This, b: *mut Block, out: *mut usize) -> i32 {
    get_result_size(b, out)
}
unsafe extern "system" fn a_cancel(_t: This, b: *mut Block) {
    cancel(b)
}
unsafe extern "system" fn a_run(_t: This, b: *mut Block, work: Option<AsyncWork>) -> i32 {
    let Some(w) = work else { return E_INVALIDARG };
    let hr = begin(b, w as *mut c_void, a_run as usize, core::ptr::null(), run_provider);
    if hr < 0 {
        return hr;
    }
    schedule(b, 0)
}
unsafe extern "system" fn a_begin(_t: This, b: *mut Block, ctx: *mut c_void, identity: usize, name: *const u8, p: Option<Provider>) -> i32 {
    let Some(p) = p else { return E_INVALIDARG };
    begin(b, ctx, identity, name, p)
}
unsafe extern "system" fn a_schedule(_t: This, b: *mut Block, delay: u32) -> i32 {
    schedule(b, delay)
}
unsafe extern "system" fn a_complete(_t: This, b: *mut Block, hr: i32, size: usize) {
    complete(b, hr, size)
}
unsafe extern "system" fn a_get_result(_t: This, b: *mut Block, identity: usize, size: usize, buf: *mut c_void, used: *mut usize) -> i32 {
    get_result(b, identity, size, buf, used)
}

pub fn interfaces() -> Vec<Iface> {
    vec![Iface {
        clsid: XASYNC,
        iid: XASYNC,
        name: "XAsync/XTaskQueue/XThread(073b7dcb, emulated)",
        slots: vec![
            (3, a_get_status as usize),
            (4, a_get_result_size as usize),
            (5, a_cancel as usize),
            (6, a_run as usize),
            (7, a_begin as usize),
            (9, a_schedule as usize),
            (10, a_complete as usize),
            (11, a_get_result as usize),
            (12, q_create as usize),
            (13, q_create_composite as usize),
            (14, q_get_port as usize),
            (15, q_duplicate as usize),
            (16, q_dispatch as usize),
            (17, q_close as usize),
            (18, q_submit as usize),
            (19, q_submit_delayed as usize),
            (22, q_terminate as usize),
            (25, q_get_process as usize),
            (27, thread_set_time_sensitive as usize),
            (29, thread_assert_not_time_sensitive as usize),
            (30, thread_is_time_sensitive as usize),
        ],
    }]
}

#[cfg(test)]
mod port_tests {
    use super::*;
    use std::sync::mpsc;

    fn running_serial_port(users: usize) -> Arc<Port> {
        let port = Port::new(SERIALIZED);
        port.users.store(users, Ordering::Release);
        let (tx, rx) = mpsc::channel();
        port.submit(Box::new(move |canceled| tx.send(canceled).unwrap()));
        assert!(!rx.recv_timeout(Duration::from_secs(2)).unwrap());
        port
    }

    fn assert_worker_stopped(port: &Arc<Port>) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Arc::strong_count(port) != 1 {
            assert!(Instant::now() < deadline, "serialized worker still holds its port");
            std::thread::yield_now();
        }
        assert!(!port.serial_started.load(Ordering::Acquire));
    }

    #[test]
    fn serialized_worker_exits_after_last_user_releases_the_port() {
        let port = running_serial_port(1);
        port.release();
        assert_worker_stopped(&port);
        assert!(port.terminated.load(Ordering::Acquire));
    }

    #[test]
    fn serialized_worker_survives_release_of_one_shared_user() {
        let port = running_serial_port(2);
        port.release();
        assert!(!port.terminated.load(Ordering::Acquire));
        let (tx, rx) = mpsc::channel();
        port.submit(Box::new(move |canceled| tx.send(canceled).unwrap()));
        assert!(!rx.recv_timeout(Duration::from_secs(2)).unwrap());
        port.release();
        assert_worker_stopped(&port);
    }

    #[test]
    fn serialized_completion_marker_restarts_a_released_port_worker() {
        let port = running_serial_port(1);
        port.release();
        assert_worker_stopped(&port);
        let (tx, rx) = mpsc::channel();
        port.submit_marker(Box::new(move |_| tx.send(()).unwrap()));
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_worker_stopped(&port);
    }

    #[test]
    fn terminating_a_live_serialized_port_still_delivers_its_marker() {
        let port = Port::new(SERIALIZED);
        port.users.store(1, Ordering::Release);
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        port.submit(Box::new(move |_| {
            started_tx.send(()).unwrap();
            finish_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let (cancel_tx, cancel_rx) = mpsc::channel();
        port.submit(Box::new(move |canceled| cancel_tx.send(canceled).unwrap()));
        port.cancel_all();
        assert!(cancel_rx.recv_timeout(Duration::from_secs(2)).unwrap());

        let (marker_tx, marker_rx) = mpsc::channel();
        port.submit_marker(Box::new(move |_| marker_tx.send(()).unwrap()));
        assert!(matches!(marker_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        finish_tx.send(()).unwrap();
        marker_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        port.release();
        assert_worker_stopped(&port);
    }
}
