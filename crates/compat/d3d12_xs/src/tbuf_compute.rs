use core::ffi::c_void;
use std::collections::BTreeMap;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tombox_xshader::hlsl::TbufClass;

use crate::device_wrap::Guid;
use crate::tbuf_bindings::{Classes, Layout};

const IID_PSO: Guid = Guid {
    d1: 0x765a_30f3,
    d2: 0xf624,
    d3: 0x4c6f,
    d4: [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45],
};
const IID_IUNKNOWN: Guid = Guid {
    d1: 0,
    d2: 0,
    d3: 0,
    d4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46],
};
const PLAN_GUID: Guid = Guid {
    d1: 0x9ad1_209d,
    d2: 0x8c08,
    d3: 0x41df,
    d4: [0x93, 0x9a, 0x5b, 0x4d, 0x2e, 0x64, 0x37, 0xef],
};
const MAX_VARIANTS: usize = 16;
const DISPATCH: usize = 14;
const DISPATCH_X: usize = crate::console_slots::CL_DISPATCH_X;

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("TOMBOX_TBUF_COMPUTE").as_deref() == Ok("1"))
}

unsafe fn slot(object: *mut c_void, index: usize) -> usize {
    unsafe { *(*(object as *const *const usize)).add(index) }
}

struct OwnedPso(usize);

impl Drop for OwnedPso {
    fn drop(&mut self) {
        let pso = self.0 as *mut c_void;
        let release: unsafe extern "system" fn(*mut c_void) -> u32 =
            unsafe { core::mem::transmute(slot(pso, 2)) };
        unsafe { release(pso) };
    }
}

type Variant = Arc<OnceLock<Option<OwnedPso>>>;

#[repr(C)]
struct Plan {
    vt: &'static PlanVtable,
    refs: AtomicU32,
    disabled: AtomicBool,
    device: usize,
    root: usize,
    root_blob: Arc<[u8]>,
    shader: Vec<u8>,
    layout: Layout,
    desc: [u8; crate::pso_serial::COMPUTE_DESC_SIZE],
    variants: Mutex<BTreeMap<Classes, Variant>>,
}

#[repr(C)]
struct PlanVtable {
    query: unsafe extern "system" fn(*mut Plan, *const Guid, *mut *mut c_void) -> i32,
    add: unsafe extern "system" fn(*mut Plan) -> u32,
    release: unsafe extern "system" fn(*mut Plan) -> u32,
}

static PLAN_VT: PlanVtable = PlanVtable {
    query,
    add,
    release,
};

unsafe extern "system" fn query(plan: *mut Plan, iid: *const Guid, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return 0x8000_4003u32 as i32;
    }
    unsafe { *out = null_mut() };
    if iid.is_null() || unsafe { *iid != IID_IUNKNOWN } {
        return 0x8000_4002u32 as i32;
    }
    unsafe {
        add(plan);
        *out = plan.cast();
    }
    0
}

unsafe extern "system" fn add(plan: *mut Plan) -> u32 {
    unsafe { (*plan).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn release(plan: *mut Plan) -> u32 {
    let left = unsafe { (*plan).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
    if left == 0 {
        drop(unsafe { Box::from_raw(plan) });
    }
    left
}

pub unsafe fn register(
    real: *mut c_void,
    pso: *mut c_void,
    iid: *const Guid,
    desc: &[u8],
    shader: Option<&[u8]>,
) {
    if !enabled() || pso.is_null() || iid.is_null() || unsafe { *iid != IID_PSO } {
        return;
    }
    let prepared = std::panic::catch_unwind(|| {
        let shader = shader?;
        let root = u64::from_le_bytes(desc.get(..8)?.try_into().ok()?) as usize;
        let (_, root_blob, _) = crate::pso_lib::root_info(root)?;
        let ex = tombox_xshader::extract(shader).ok()?;
        if ex.stage != tombox_xshader::Stage::Compute || !ex.has_root_sig {
            return None;
        }
        let translated = tombox_xshader::hlsl::translate(&ex).ok()?;
        let layout = Layout::compute(&root_blob, &translated.tbuf_unresolved)?;
        let mut desc: [u8; crate::pso_serial::COMPUTE_DESC_SIZE] = desc.try_into().ok()?;
        desc[8..24].fill(0);
        desc[32..48].fill(0);
        Some(Plan {
            vt: &PLAN_VT,
            refs: AtomicU32::new(1),
            disabled: AtomicBool::new(false),
            device: real as usize,
            root,
            root_blob,
            shader: shader.to_vec(),
            layout,
            desc,
            variants: Mutex::new(BTreeMap::new()),
        })
    });
    let candidate = prepared.ok().flatten();
    static ATTACH: Mutex<()> = Mutex::new(());
    let _guard = ATTACH.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(prev) = unsafe { plan_of(pso) } {
        let prev = unsafe { &*prev.0 };
        if !candidate.as_ref().is_some_and(|plan| {
            plan.device == prev.device && plan.desc == prev.desc && plan.shader == prev.shader
        }) {
            prev.disabled.store(true, Ordering::Release);
        }
        return;
    }
    let Some(plan) = candidate else { return };
    let plan = Box::into_raw(Box::new(plan));
    let set: unsafe extern "system" fn(*mut c_void, *const Guid, *mut c_void) -> i32 =
        unsafe { core::mem::transmute(slot(pso, 5)) };
    unsafe {
        set(pso, &PLAN_GUID, plan.cast());
        release(plan);
    }
}

struct PlanRef(*mut Plan);

impl Drop for PlanRef {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

unsafe fn plan_of(pso: *mut c_void) -> Option<PlanRef> {
    let get: unsafe extern "system" fn(*mut c_void, *const Guid, *mut u32, *mut c_void) -> i32 =
        unsafe { core::mem::transmute(slot(pso, 3)) };
    let mut plan: *mut Plan = null_mut();
    let mut size = core::mem::size_of_val(&plan) as u32;
    let hr = unsafe {
        get(
            pso,
            &PLAN_GUID,
            &mut size,
            (&mut plan as *mut *mut Plan).cast(),
        )
    };
    (hr >= 0 && !plan.is_null()).then(|| PlanRef(plan))
}

#[cfg(test)]
pub(crate) unsafe fn cached_variants_for_test(pso: usize) -> Option<usize> {
    let handle = unsafe { plan_of(pso as *mut c_void) }?;
    let plan = unsafe { &*handle.0 };
    let variants = plan.variants.lock().unwrap_or_else(|e| e.into_inner());
    Some(
        variants
            .values()
            .filter(|cell| matches!(cell.get(), Some(Some(_))))
            .count(),
    )
}

fn variant_cell(plan: &Plan, classes: &Classes) -> Option<Variant> {
    let mut variants = plan.variants.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(cell) = variants.get(classes) {
        return Some(cell.clone());
    }
    if variants.len() >= MAX_VARIANTS {
        return None;
    }
    let cell = Arc::new(OnceLock::new());
    variants.insert(classes.clone(), cell.clone());
    Some(cell)
}

unsafe fn build_variant(
    real: *mut c_void,
    plan: &Plan,
    classes: &Classes,
) -> Result<OwnedPso, String> {
    let code = crate::shader_xlat::compile_tbuf_compute(&plan.shader, &plan.root_blob, classes)?;
    let mut desc = plan.desc;
    desc[8..16].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
    desc[16..24].copy_from_slice(&(code.len() as u64).to_le_bytes());
    let mut pso = null_mut();
    let create: unsafe extern "system" fn(
        *mut c_void,
        *const u8,
        *const Guid,
        *mut *mut c_void,
    ) -> i32 = unsafe { core::mem::transmute(slot(real, 11)) };
    let hr = unsafe { create(real, desc.as_ptr(), &IID_PSO, &mut pso) };
    if hr < 0 || pso.is_null() {
        return Err(format!("CreateComputePipelineState returned {hr:#010x}"));
    }
    Ok(OwnedPso(pso as usize))
}

unsafe fn with_variant(list: *mut c_void, dispatch: impl FnOnce()) {
    let select_variant = || {
        if unsafe { crate::fill::list_type(list) } != crate::fill::LIST_TYPE_COMPUTE {
            return None;
        }
        let base = crate::cl2_state::bound_pso(list)?;
        let resolved = crate::pso_async::real_of(base);
        let base = if resolved != 0 { resolved } else { base } as *mut c_void;
        if base.is_null() {
            return None;
        }
        let handle = unsafe { plan_of(base) }?;
        let plan = unsafe { &*handle.0 };
        if plan.disabled.load(Ordering::Acquire) {
            return None;
        }
        let classes = crate::cl2_state::compute_tbuf_classes(list, plan.root, &plan.layout)?;
        if classes.values().all(|&class| class == TbufClass::Float) {
            return None;
        }
        let cell = variant_cell(plan, &classes)?;
        let pso = cell
            .get_or_init(|| {
                match unsafe { build_variant(plan.device as *mut c_void, plan, &classes) } {
                    Ok(pso) => {
                        compat_common::record(
                            "d3d12_xs",
                            "tbuf_compute",
                            &format!("Built compute variant for {classes:?}"),
                        );
                        Some(pso)
                    }
                    Err(err) => {
                        compat_common::record(
                            "d3d12_xs",
                            "tbuf_compute",
                            &format!("Compute specialization unavailable: {err}"),
                        );
                        None
                    }
                }
            })
            .as_ref()?
            .0 as *mut c_void;
        Some((handle, base, pso))
    };
    let Some((_handle, base, pso)) = select_variant() else {
        dispatch();
        return;
    };
    let set: unsafe extern "system" fn(*mut c_void, *mut c_void) =
        unsafe { core::mem::transmute(crate::cmdlist_wrap::pc_method(list, 25)) };
    unsafe { set(list, pso) };
    dispatch();
    unsafe { set(list, base) };
}

static NEXT: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

unsafe extern "system" fn dispatch(list: *mut c_void, x: u32, y: u32, z: u32) {
    let next: unsafe extern "system" fn(*mut c_void, u32, u32, u32) =
        unsafe { core::mem::transmute(NEXT[0].load(Ordering::Relaxed)) };
    unsafe { with_variant(list, || next(list, x, y, z)) };
}

unsafe extern "system" fn dispatch_x(list: *mut c_void, x: u32, y: u32, z: u32, flags: u32) {
    let next: unsafe extern "system" fn(*mut c_void, u32, u32, u32, u32) =
        unsafe { core::mem::transmute(NEXT[1].load(Ordering::Relaxed)) };
    unsafe { with_variant(list, || next(list, x, y, z, flags)) };
}

pub fn install(vt: &[AtomicUsize]) {
    if !enabled() {
        return;
    }
    for (idx, (slot, hook)) in [
        (DISPATCH, dispatch as *const () as usize),
        (DISPATCH_X, dispatch_x as *const () as usize),
    ]
    .into_iter()
    .enumerate()
    {
        let prev = vt[slot].load(Ordering::Relaxed);
        if prev != 0 {
            NEXT[idx].store(prev, Ordering::Relaxed);
            vt[slot].store(hook, Ordering::Relaxed);
        }
    }
}
