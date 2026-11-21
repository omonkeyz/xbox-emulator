use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock, RwLock};
use compat_common::{live, record};

static PREV_RESET: AtomicUsize = AtomicUsize::new(0);
static PREV_SET: AtomicUsize = AtomicUsize::new(0);
static DEVICE: AtomicUsize = AtomicUsize::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);
static SEEN: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
static MAP: RwLock<Option<HashMap<usize, usize>>> = RwLock::new(None);
static EDGE: live::Edge = live::Edge::new();

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
const IID_DEVICE: Guid = Guid(0x189819f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
const IID_PSO: Guid = Guid(0x765a30f3, 0xf624, 0x4c6f, [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45]);

const GFX_STAGES: [(usize, &str); 5] = [(0x08, "vs"), (0x18, "ps"), (0x28, "ds"), (0x38, "hs"), (0x48, "gs")];

pub fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| live::flag("TOMBOX_SHADER_HOTRELOAD", "shader_hotreload", false))
}

unsafe fn vt(obj: *mut c_void) -> *const usize {
    *(obj as *const *const usize)
}

fn mapped(pso: *mut c_void) -> *mut c_void {
    if !ACTIVE.load(Ordering::Relaxed) || pso.is_null() {
        return pso;
    }
    let g = MAP.read().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|m| m.get(&(pso as usize)).copied()).map(|p| p as *mut c_void).unwrap_or(pso)
}

unsafe fn note(list: *mut c_void, pso: *mut c_void) {
    if pso.is_null() {
        return;
    }
    if DEVICE.load(Ordering::Relaxed) == 0 && !list.is_null() {
        let f: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(crate::cmdlist_wrap::real_method(7));
        let mut d = core::ptr::null_mut();
        if f(list, &IID_DEVICE, &mut d) >= 0 && !d.is_null() {
            if DEVICE.compare_exchange(0, d as usize, Ordering::AcqRel, Ordering::Relaxed).is_err() {
                let rel: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt(d).add(2));
                rel(d);
            }
        }
    }
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashSet::new).insert(pso as usize);
}

unsafe extern "system" fn h_reset(this: *mut c_void, alloc: *mut c_void, pso: *mut c_void) -> i32 {
    note(this, pso);
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32 = core::mem::transmute(PREV_RESET.load(Ordering::Relaxed));
    f(this, alloc, mapped(pso))
}

unsafe extern "system" fn h_set_pso(this: *mut c_void, pso: *mut c_void) {
    note(this, pso);
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void) = core::mem::transmute(PREV_SET.load(Ordering::Relaxed));
    f(this, mapped(pso))
}

pub unsafe fn install(xbox_vt: &[AtomicUsize]) {
    if !enabled() {
        return;
    }
    PREV_RESET.store(xbox_vt[10].load(Ordering::Relaxed), Ordering::Relaxed);
    xbox_vt[10].store(h_reset as usize, Ordering::Relaxed);
    PREV_SET.store(xbox_vt[25].load(Ordering::Relaxed), Ordering::Relaxed);
    xbox_vt[25].store(h_set_pso as usize, Ordering::Relaxed);
    record("d3d12_xs", "hot_reload", "shader hot reload armed: set shader_reload=<all|override|hash,hash..> in live.cfg (change the value or shader_reload_seq to fire again), shader_reload=off restores the original PSOs");
}

fn rd64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

fn wr64(d: &mut [u8], o: usize, v: u64) {
    d[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn has_override(hash: u64) -> bool {
    std::env::temp_dir().join("tombox_shader_override").join(format!("{hash:016x}.hlsl")).exists()
}

enum Want {
    All,
    Overrides,
    Hashes(Vec<u64>),
}

fn want_of(v: &str) -> Want {
    let hs: Vec<u64> = v
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|w| w.trim().trim_start_matches("0x"))
        .filter(|w| w.len() == 16)
        .filter_map(|w| u64::from_str_radix(w, 16).ok())
        .collect();
    if !hs.is_empty() {
        Want::Hashes(hs)
    } else if v.trim().eq_ignore_ascii_case("override") {
        Want::Overrides
    } else {
        Want::All
    }
}

struct Stage {
    off: usize,
    name: &'static str,
    hash: u64,
    blob: std::sync::Arc<Vec<u8>>,
}

unsafe fn rebuild(dev: *mut c_void, pso: usize, want: &Want, keep: &mut Vec<std::sync::Arc<Vec<u8>>>) -> Result<Option<usize>, String> {
    let gfx = crate::device_wrap::gfx_pso_desc(pso);
    let (compute, mut desc) = match &gfx {
        Some((d, _)) => (false, d.clone()),
        None => match crate::device_x::compute_desc(pso) {
            Some(d) => (true, d),
            None => return Ok(None),
        },
    };
    let offs: Vec<(usize, &'static str)> = if compute { vec![(0x08, "cs")] } else { GFX_STAGES.to_vec() };
    let mut stages = Vec::new();
    for (off, name) in offs {
        let code = rd64(&desc, off) as usize;
        if code == 0 {
            continue;
        }
        let Some((hash, blob)) = crate::shader_xlat::origin(code) else { return Ok(None) };
        stages.push(Stage { off, name, hash, blob });
    }
    let hit = match want {
        Want::All => !stages.is_empty(),
        Want::Overrides => stages.iter().any(|s| has_override(s.hash)),
        Want::Hashes(h) => stages.iter().any(|s| h.contains(&s.hash)),
    };
    if !hit {
        return Ok(None);
    }
    // As at creation: the stage feeding the pixel shader repeats its interpolation modifiers.
    let feeder = ["gs", "ds", "vs"].into_iter().find(|n| stages.iter().any(|s| s.name == *n));
    let pc_centroid = crate::centroid_bind::tag_rtfmt(u64::from_le_bytes(desc[0..8].try_into().unwrap()), [0; 8])[0] & crate::centroid_bind::RTFMT_PC_CENTROID != 0;
    let ps_link = stages.iter().find(|s| s.name == "ps").map(|s| crate::shader_xlat::ps_link(&s.blob, pc_centroid)).unwrap_or_else(crate::shader_xlat::no_link);
    for s in &stages {
        let rtfmt = if s.name == "ps" {
            let mut f = [0u32; 8];
            for (i, x) in f.iter_mut().enumerate() {
                *x = u32::from_le_bytes(desc[0x244 + i * 4..0x248 + i * 4].try_into().unwrap());
            }
            crate::centroid_bind::tag_rtfmt(u64::from_le_bytes(desc[0..8].try_into().unwrap()), crate::shader_xlat::export_formats(u32::from_le_bytes(desc[0x240..0x244].try_into().unwrap()), &f))
        } else {
            [0u8; 8]
        };
        let link = if Some(s.name) == feeder { ps_link.clone() } else { crate::shader_xlat::no_link() };
        let code = crate::shader_xlat::retranslate(&s.blob, s.hash, rtfmt, &link).map_err(|e| format!("{} {:016x}: {}", s.name, s.hash, e.lines().next().unwrap_or("")))?;
        wr64(&mut desc, s.off, code.as_ptr() as u64);
        wr64(&mut desc, s.off + 8, code.len() as u64);
        keep.push(code);
    }
    let (slot, cached) = if compute { (11, 0x20) } else { (10, 0x278) };
    wr64(&mut desc, cached, 0);
    wr64(&mut desc, cached + 8, 0);
    let f: unsafe extern "system" fn(*mut c_void, *const u8, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(*vt(dev).add(slot));
    let mut out = core::ptr::null_mut();
    let hr = f(dev, desc.as_ptr(), &IID_PSO, &mut out);
    if hr < 0 || out.is_null() {
        return Err(format!("Create{}PipelineState hr={hr:#010x}", if compute { "Compute" } else { "Graphics" }));
    }
    let start = keep.len() - stages.len();
    if compute {
        crate::device_x::register_compute(out, &desc, keep[start].clone());
    } else {
        crate::device_wrap::register_gfx_pso(out, desc, keep[start..].to_vec());
    }
    Ok(Some(out as usize))
}

fn run(value: String) {
    let dev = DEVICE.load(Ordering::Relaxed) as *mut c_void;
    if value.trim().eq_ignore_ascii_case("off") {
        ACTIVE.store(false, Ordering::Relaxed);
        let n = MAP.write().unwrap_or_else(|e| e.into_inner()).take().map(|m| m.len()).unwrap_or(0);
        record("d3d12_xs", "hot_reload", &format!("shader_reload=off: {n} replacement PSOs dropped, original PSOs in use"));
        BUSY.store(false, Ordering::SeqCst);
        return;
    }
    if dev.is_null() {
        record("d3d12_xs", "hot_reload", "no command list has set a PSO yet; nothing to reload");
        BUSY.store(false, Ordering::SeqCst);
        return;
    }
    let want = want_of(&value);
    let seen: Vec<usize> = SEEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.iter().copied().collect()).unwrap_or_default();
    let t0 = std::time::Instant::now();
    let (mut ok, mut failed, mut skipped) = (0usize, 0usize, 0usize);
    let mut keep = Vec::new();
    let mut fresh = HashMap::new();
    for p in seen {
        match unsafe { rebuild(dev, p, &want, &mut keep) } {
            Ok(Some(n)) => {
                fresh.insert(p, n);
                ok += 1;
            }
            Ok(None) => skipped += 1,
            Err(e) => {
                failed += 1;
                if failed <= 16 {
                    record("d3d12_xs", "hot_reload", &format!("pso {p:#x} kept: {e}"));
                }
            }
        }
    }
    drop(keep);
    {
        let mut g = MAP.write().unwrap_or_else(|e| e.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        m.extend(fresh);
        ACTIVE.store(!m.is_empty(), Ordering::Relaxed);
    }
    record("d3d12_xs", "hot_reload", &format!("shader_reload={value}: {ok} PSOs rebuilt, {failed} failed, {skipped} not matching, {:.0} ms; existing PSOs now draw with the new shaders", t0.elapsed().as_secs_f64() * 1000.0));
    BUSY.store(false, Ordering::SeqCst);
}

pub fn on_present() {
    if !enabled() {
        return;
    }
    let Some(v) = EDGE.poll("shader_reload") else { return };
    if BUSY.swap(true, Ordering::SeqCst) {
        record("d3d12_xs", "hot_reload", "reload already running; trigger ignored");
        return;
    }
    let _ = std::thread::Builder::new().name("tombox-shader-reload".into()).spawn(move || run(v));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_values_parse() {
        assert!(matches!(want_of("all"), Want::All));
        assert!(matches!(want_of("1"), Want::All));
        assert!(matches!(want_of("override"), Want::Overrides));
        match want_of("251ed6ba5631cd6d, 0xd4a501eafe4beee0") {
            Want::Hashes(h) => assert_eq!(h, vec![0x251ed6ba5631cd6d, 0xd4a501eafe4beee0]),
            _ => panic!(),
        }
    }
}
