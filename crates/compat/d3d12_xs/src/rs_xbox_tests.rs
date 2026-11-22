use crate::rs_xbox::*;
use core::ffi::c_void;

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

const IID_DEVICE: Guid = Guid(0x189819f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
const IID_ROOT_SIGNATURE: Guid = Guid(0xc54a6b66, 0x72df, 0x4ee8, [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14]);
const IID_DESERIALIZER: Guid = Guid(0x34ab647b, 0x3cc8, 0x46ac, [0x84, 0x1b, 0xc0, 0x96, 0x56, 0x45, 0xc0, 0x46]);
const IID_VERSIONED_DESERIALIZER: Guid = Guid(0x7f91ce67, 0x090c, 0x4bb7, [0xb7, 0x8e, 0xed, 0x8f, 0xf2, 0xe3, 0x1d, 0xa0]);

const PARAM_UAV: u32 = 4;
const PARAM_CONSTANTS: u32 = 1;
const VERSION_1_0: u32 = 1;
const VERSION_1_1: u32 = 2;

unsafe fn vslot(o: *mut c_void, i: usize) -> usize {
    unsafe { *(*(o as *const *const usize)).add(i) }
}

unsafe fn release(o: *mut c_void) {
    if !o.is_null() {
        let r: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(vslot(o, 2)) };
        unsafe { r(o) };
    }
}

unsafe fn blob_bytes(b: *mut c_void) -> Vec<u8> {
    let ptr: unsafe extern "system" fn(*mut c_void) -> *const u8 = unsafe { core::mem::transmute(vslot(b, 3)) };
    let size: unsafe extern "system" fn(*mut c_void) -> usize = unsafe { core::mem::transmute(vslot(b, 4)) };
    unsafe { core::slice::from_raw_parts(ptr(b), size(b)) }.to_vec()
}

fn param(kind: u32, reg: u32, num32: u32) -> [u8; 32] {
    let mut p = [0u8; 32];
    p[0..4].copy_from_slice(&kind.to_le_bytes());
    p[8..12].copy_from_slice(&reg.to_le_bytes());
    p[16..20].copy_from_slice(&num32.to_le_bytes());
    p
}

fn xdxr_params() -> Vec<u8> {
    [param(PARAM_UAV, 0, 0), param(PARAM_UAV, 1, 0), param(PARAM_CONSTANTS, 0, 16)].concat()
}

fn versioned_desc(params: &[u8], n: u32, flags: u32) -> [u8; 48] {
    let mut d = [0u8; 48];
    d[0..4].copy_from_slice(&VERSION_1_1.to_le_bytes());
    d[8..12].copy_from_slice(&n.to_le_bytes());
    d[16..24].copy_from_slice(&(params.as_ptr() as u64).to_le_bytes());
    d[40..44].copy_from_slice(&flags.to_le_bytes());
    d
}

fn desc_1_0(params: &[u8], n: u32, flags: u32) -> [u8; 40] {
    let mut d = [0u8; 40];
    d[0..4].copy_from_slice(&n.to_le_bytes());
    d[8..16].copy_from_slice(&(params.as_ptr() as u64).to_le_bytes());
    d[32..36].copy_from_slice(&flags.to_le_bytes());
    d
}

fn serialize_v10(flags: u32) -> (i32, Vec<u8>) {
    let params = param(PARAM_CONSTANTS, 0, 4);
    let d = desc_1_0(&params, 1, flags);
    let (mut b, mut e): (*mut c_void, *mut c_void) = (core::ptr::null_mut(), core::ptr::null_mut());
    let hr = crate::D3D12SerializeRootSignature(d.as_ptr() as *const c_void, VERSION_1_0, &mut b, &mut e);
    let out = if hr >= 0 { unsafe { blob_bytes(b) } } else { Vec::new() };
    unsafe { release(b) };
    unsafe { release(e) };
    (hr, out)
}

fn serialize_xdxr(flags: u32) -> Vec<u8> {
    let params = xdxr_params();
    let d = versioned_desc(&params, 3, flags);
    serialize_versioned_desc(&d)
}

fn serialize_versioned_desc(d: &[u8; 48]) -> Vec<u8> {
    let (mut b, mut e): (*mut c_void, *mut c_void) = (core::ptr::null_mut(), core::ptr::null_mut());
    let hr = crate::D3D12SerializeVersionedRootSignature(d.as_ptr() as *const c_void, &mut b, &mut e);
    let out = if hr >= 0 && !b.is_null() { Some(unsafe { blob_bytes(b) }) } else { None };
    unsafe { release(b) };
    unsafe { release(e) };
    out.unwrap_or_else(|| panic!("versioned serialize hr={hr:#010x}"))
}

fn raw_rts0(flags: u32) -> Vec<u8> {
    [1u32, 0, 24, 0, 24, flags].iter().flat_map(|x| x.to_le_bytes()).collect()
}

#[test]
fn flag_values_match_the_console_compiler_and_driver() {
    assert_eq!(XBOX_FORCE_MEMORY_BASED_ABI, 0x0100_0000);
    assert_eq!(XBOX_RAYTRACING, XBOX_FORCE_MEMORY_BASED_ABI);
    assert_eq!(XBOX_FORCE_COMPUTE_2_SPILL_AREA, 0x0200_0000);
    assert_eq!(XBOX_INTERNAL_XDXR, 0x0300_0000);
    assert_eq!(XBOX_DRIVER_DISCARDED, 0x8000_0000);
    assert_eq!(XBOX_ROOT_FLAGS, 0x8300_0000);
    assert_eq!(CONSOLE_UNSUPPORTED & XBOX_ROOT_FLAGS, 0);
    assert_eq!(CONSOLE_UNSUPPORTED | XBOX_ROOT_FLAGS | 0xfff, u32::MAX);
}

#[test]
fn force_memory_based_abi_bit() {
    let x = XboxRoot(XBOX_FORCE_MEMORY_BASED_ABI | 0x1);
    assert!(x.memory_based_abi() && !x.compute_2_spill_area() && !x.internal_xdxr());
    assert_eq!(x.pc_flags(), 1);
    assert_eq!(x.xbox_bits(), XBOX_FORCE_MEMORY_BASED_ABI);
    assert_eq!(x.gfx_spill_base(), GFX_SPILL_BASE);
    assert!(x.console_supported());
}

#[test]
fn compute_2_spill_area_bit_moves_graphics_spills() {
    let x = XboxRoot(XBOX_FORCE_COMPUTE_2_SPILL_AREA);
    assert!(x.compute_2_spill_area() && !x.memory_based_abi() && !x.internal_xdxr());
    assert_eq!(x.pc_flags(), 0);
    assert_eq!(x.gfx_spill_base(), 0x700);
    assert_eq!(x.user_data(false, 15), UserData::Sgpr(15));
    assert_eq!(x.user_data(false, 16), UserData::Spill(0x700 + 64));
    assert_eq!(XboxRoot(0).user_data(false, 16), UserData::Spill(0x1c0 + 64));
    assert_eq!(x.user_data(true, 31), UserData::Sgpr(31));
    assert_eq!(x.user_data(true, 32), UserData::Spill(0x380 + 128));
    assert_eq!(XboxRoot(0).user_data(true, 32), x.user_data(true, 32));
}

#[test]
fn internal_xdxr_is_both_bits() {
    let x = XboxRoot(XBOX_INTERNAL_XDXR);
    assert!(x.internal_xdxr() && x.memory_based_abi() && x.compute_2_spill_area());
    assert!(!XboxRoot(XBOX_FORCE_MEMORY_BASED_ABI).internal_xdxr());
    assert!(!XboxRoot(XBOX_FORCE_COMPUTE_2_SPILL_AREA).internal_xdxr());
    assert_eq!(x.pc_flags(), 0);
}

#[test]
fn bit_31_is_accepted_and_discarded() {
    let x = XboxRoot(XBOX_DRIVER_DISCARDED | XBOX_FORCE_MEMORY_BASED_ABI | 0x40);
    assert!(x.console_supported());
    assert_eq!(x.driver_flags(), XBOX_FORCE_MEMORY_BASED_ABI | 0x40);
    assert_eq!(x.pc_flags(), 0x40);
}

#[test]
fn unsupported_bits_are_left_for_the_pc_runtime_to_reject() {
    let x = XboxRoot(0x1000 | XBOX_FORCE_MEMORY_BASED_ABI);
    assert!(!x.console_supported());
    assert_eq!(x.pc_flags(), 0x1000);
    let (hr, _) = serialize_v10(0x0400_0000);
    assert!(hr < 0, "PC accepted an unsupported root-signature flag bit");
}

#[test]
fn pc_blob_strips_each_xbox_bit_and_keeps_pc_flags() {
    for bits in [XBOX_FORCE_MEMORY_BASED_ABI, XBOX_FORCE_COMPUTE_2_SPILL_AREA, XBOX_INTERNAL_XDXR, XBOX_DRIVER_DISCARDED, XBOX_ROOT_FLAGS] {
        let raw = raw_rts0(bits | 0x21);
        let pc = pc_blob(&raw).unwrap();
        assert_eq!(blob_flags(&pc).unwrap().1, 0x21, "raw {bits:#x}");
        assert_eq!(pc.len(), raw.len());
        let dx = crate::device_wrap::wrap_root_signature(&raw);
        let mut hashed = dx.clone();
        hashed[4..20].fill(0xaa);
        let pc = pc_blob(&hashed).unwrap();
        assert_eq!(blob_flags(&pc).unwrap().1, 0x21, "dxbc {bits:#x}");
        assert_eq!(&pc[4..20], &[0u8; 16]);
        assert_eq!(&pc[20..], &{ let mut d = dx.clone(); set_blob_flags(&mut d, 0x21); d }[20..]);
    }
    assert!(pc_blob(&raw_rts0(0x21)).is_none());
    assert!(pc_blob(&[1, 2, 3]).is_none());
}

#[test]
fn serialize_v10_keeps_each_xbox_bit_in_the_blob() {
    let (hr, plain) = serialize_v10(0x1);
    assert!(hr >= 0, "PC-only flags hr={hr:#010x}");
    assert_eq!(blob_flags(&plain).unwrap().1, 0x1);
    for bits in [XBOX_FORCE_MEMORY_BASED_ABI, XBOX_FORCE_COMPUTE_2_SPILL_AREA, XBOX_INTERNAL_XDXR, XBOX_DRIVER_DISCARDED] {
        let (hr, blob) = serialize_v10(bits | 0x1);
        assert!(hr >= 0, "flags {bits:#x} hr={hr:#010x}");
        assert_eq!(blob_flags(&blob).unwrap().1, bits | 0x1);
    }
}

#[test]
fn versioned_serializer_round_trips_flags_through_legacy_deserializer() {
    for version in [VERSION_1_0, VERSION_1_1] {
        for flags in [1, 1 | XBOX_FORCE_MEMORY_BASED_ABI, 1 | XBOX_FORCE_COMPUTE_2_SPILL_AREA] {
            let mut desc = [0u8; 48];
            desc[..4].copy_from_slice(&version.to_le_bytes());
            desc[VERSIONED_DESC_FLAGS..VERSIONED_DESC_FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
            let blob = serialize_versioned_desc(&desc);
            let mut object = core::ptr::null_mut();
            let hr = crate::xs_exports::D3D12CreateRootSignatureDeserializer(
                blob.as_ptr().cast(),
                blob.len(),
                (&IID_DESERIALIZER as *const Guid).cast(),
                &mut object,
            );
            assert!(hr >= 0 && !object.is_null(), "version {version} flags {flags:#x} hr={hr:#010x}");
            let actual = unsafe {
                let get: unsafe extern "system" fn(*mut c_void) -> *const u8 =
                    core::mem::transmute(vslot(object, 3));
                let actual = core::ptr::read_unaligned(get(object).add(DESC_FLAGS).cast::<u32>());
                release(object);
                actual
            };
            assert_eq!(actual, flags, "version {version}");
        }
    }
}

#[test]
fn deserializers_report_the_console_flags() {
    let blob = serialize_xdxr(XBOX_INTERNAL_XDXR);
    let mut d: *mut c_void = core::ptr::null_mut();
    let hr = crate::xs_exports::D3D12CreateVersionedRootSignatureDeserializer(blob.as_ptr() as *const c_void, blob.len(), &IID_VERSIONED_DESERIALIZER as *const Guid as *const c_void, &mut d);
    assert!(hr >= 0 && !d.is_null(), "versioned deserializer hr={hr:#010x}");
    unsafe {
        let get: unsafe extern "system" fn(*mut c_void) -> *const u8 = core::mem::transmute(vslot(d, 4));
        let desc = get(d);
        assert_eq!(core::ptr::read_unaligned(desc as *const u32), VERSION_1_1);
        assert_eq!(core::ptr::read_unaligned(desc.add(8) as *const u32), 3);
        assert_eq!(core::ptr::read_unaligned(desc.add(VERSIONED_DESC_FLAGS) as *const u32), XBOX_INTERNAL_XDXR);
        release(d);
    }
    let (hr, v10) = serialize_v10(XBOX_FORCE_MEMORY_BASED_ABI | 0x1);
    assert!(hr >= 0);
    let mut d: *mut c_void = core::ptr::null_mut();
    let hr = crate::xs_exports::D3D12CreateRootSignatureDeserializer(v10.as_ptr() as *const c_void, v10.len(), &IID_DESERIALIZER as *const Guid as *const c_void, &mut d);
    assert!(hr >= 0 && !d.is_null(), "deserializer hr={hr:#010x}");
    unsafe {
        let get: unsafe extern "system" fn(*mut c_void) -> *const u8 = core::mem::transmute(vslot(d, 3));
        assert_eq!(core::ptr::read_unaligned(get(d).add(DESC_FLAGS) as *const u32), XBOX_FORCE_MEMORY_BASED_ABI | 0x1);
        release(d);
    }
}

unsafe fn device() -> *mut c_void {
    let p = crate::host_proc(b"D3D12CreateDevice\0");
    assert!(p != 0);
    let f: extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(p) };
    let mut d: *mut c_void = core::ptr::null_mut();
    assert!(f(core::ptr::null_mut(), 0xb000, &IID_DEVICE, &mut d) >= 0 && !d.is_null());
    d
}

unsafe fn create_rs(dev: *mut c_void, blob: &[u8]) -> (i32, *mut c_void) {
    let f: unsafe extern "system" fn(*mut c_void, u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(vslot(dev, 16)) };
    let mut rs: *mut c_void = core::ptr::null_mut();
    let hr = unsafe { f(dev, 1, blob.as_ptr(), blob.len(), &IID_ROOT_SIGNATURE, &mut rs) };
    (hr, rs)
}

#[test]
fn xdxr_root_signature_is_created_on_pc_after_the_strip() {
    let console = serialize_xdxr(XBOX_INTERNAL_XDXR);
    assert_eq!(crate::console_slots_mesh::console_root_flags(&console), Some(XBOX_INTERNAL_XDXR));
    unsafe {
        let dev = device();
        let (raw_hr, raw_rs) = create_rs(dev, &console);
        release(raw_rs);
        assert!(raw_hr < 0, "PC accepted the console-only bits; the strip would no longer be needed");
        let pc = crate::console_slots_mesh::pc_root_blob(&console).unwrap();
        assert_eq!(crate::console_slots_mesh::console_root_flags(&pc), Some(0));
        let (hr, rs) = create_rs(dev, &pc);
        assert!(hr >= 0 && !rs.is_null(), "stripped xdxr root signature hr={hr:#010x}");
        release(rs);
        for bits in [XBOX_FORCE_MEMORY_BASED_ABI, XBOX_FORCE_COMPUTE_2_SPILL_AREA, XBOX_DRIVER_DISCARDED] {
            let b = serialize_xdxr(bits);
            let (hr, rs) = create_rs(dev, &pc_blob(&b).unwrap());
            assert!(hr >= 0, "flag {bits:#x} hr={hr:#010x}");
            release(rs);
        }
        release(dev);
    }
}
