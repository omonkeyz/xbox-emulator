use super::*;
use tombox_xshader::hlsl::translate_user_data;
use tombox_xshader::user_data_abi::{self, RootArg, UserData, TB_USER_DATA_BYTES, TB_USER_DATA_DWORDS, TB_USER_DATA_REGISTER, TB_USER_DATA_SPACE};

fn reads(sgprs: [u32; 4]) -> Vec<u32> {
    let mut p = Prog::new();
    for (k, s) in sgprs.iter().enumerate() {
        p.raw(&[vop1(1, k as u32, *s)]);
    }
    p.words()
}

fn layout_root() -> RootSig {
    let t = RootParam { visibility: 0, kind: RootParamKind::Table(vec![RootRange { range_type: 0, num: 1, base_register: 0, space: 0, offset_in_table: u32::MAX }]) };
    let k = RootParam { visibility: 0, kind: RootParamKind::Constants { register: 0, space: 0, num32: 3 } };
    RootSig { version: 1, flags: 0, params: vec![t, k], num_static_samplers: 0, static_samplers: Vec::new() }
}

unsafe fn cbv_root(g: &Gpu) -> Result<*mut c_void, String> {
    let d3d = LoadLibraryA(b"d3d12.dll\0".as_ptr());
    let ser: unsafe extern "system" fn(*const u8, u32, *mut *mut c_void, *mut *mut c_void) -> i32 =
        core::mem::transmute(GetProcAddress(d3d, b"D3D12SerializeRootSignature\0".as_ptr()));
    let param: [u32; 8] = [2, 0, TB_USER_DATA_REGISTER, TB_USER_DATA_SPACE, 0, 0, 0, 0];
    let ps: Vec<u8> = param.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut desc = [0u8; 40];
    desc[0..4].copy_from_slice(&1u32.to_le_bytes());
    desc[8..16].copy_from_slice(&(ps.as_ptr() as u64).to_le_bytes());
    let mut blob: *mut c_void = core::ptr::null_mut();
    let mut err: *mut c_void = core::ptr::null_mut();
    if ser(desc.as_ptr(), 1, &mut blob, &mut err) < 0 {
        let msg = if err.is_null() { String::new() } else { std::ffi::CStr::from_ptr(vcall!(err, 3, fn() -> *const i8)).to_string_lossy().into_owned() };
        return Err(format!("serialize root signature: {msg}"));
    }
    let p = vcall!(blob, 3, fn() -> *const u8);
    let n = vcall!(blob, 4, fn() -> usize);
    let mut rs: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(g.device, 16, fn(u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32, 0, p, n, &d3d::IID_ROOTSIG, &mut rs);
    if hr < 0 {
        return Err(format!("CreateRootSignature {hr:#x}{}", g.messages()));
    }
    Ok(rs)
}

fn draw_user_data(words: &[u32], ud: &UserData) -> Result<[u32; 4], String> {
    let t = translate_user_data(&shader(words))?;
    if t.user_data_base.is_none() || !t.hlsl.contains("TbUserData") {
        return Err(format!("user-data fallback not selected\n{}", t.hlsl));
    }
    let code = dxc::compile(&t.hlsl, &dxc::profile_for(t.profile)).map_err(|e| format!("dxc: {e}\n{}", t.hlsl))?;
    let m = CTX.get_or_init(|| Mutex::new(unsafe { ctx_new() }));
    let mut c = m.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        let rs = cbv_root(&c.gpu)?;
        let up = c.gpu.buffer(TB_USER_DATA_BYTES as u64, 2, d3d::GENERIC_READ, false)?;
        Gpu::map_write(up, &user_data_abi::bytes(ud));
        let va = Gpu::gpu_va(up);
        let pre = |l: *mut c_void| {
            vcall!(l, 30, fn(*mut c_void) -> (), rs);
            vcall!(l, 38, fn(u32, u64) -> (), 0, va);
        };
        let r = draw_with(&mut c, &code, rs, &pre, &|_| {}).map_err(|e| format!("{e}\n{}", t.hlsl));
        d3d::release(up);
        d3d::release(rs);
        r
    }
}

#[test]
fn user_data_sgprs_read_the_bound_buffer() {
    let mut ud = [0u32; TB_USER_DATA_DWORDS];
    for (i, w) in ud.iter_mut().enumerate() {
        *w = 0xc0de_0000 | (i as u32 * 0x0101);
    }
    let out = draw_user_data(&reads([0, 8, 17, 31]), &ud).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(out, [ud[0], ud[8], ud[17], ud[31]]);
}

#[test]
fn user_data_follows_the_static_root_layout() {
    let words = reads([8, 9, 10, 11]);
    let mut with_root = shader(&words);
    with_root.root_sig = Some(layout_root());
    let fallback_with_root = translate_user_data(&with_root).unwrap_or_else(|e| panic!("{e}"));
    let static_path = translate(&with_root).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fallback_with_root.user_data_base, None);
    assert_eq!(fallback_with_root.hlsl, static_path.hlsl);
    let default_path = translate(&shader(&words)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(default_path.user_data_base, None);
    assert!(!default_path.hlsl.contains("TbUserData"), "{}", default_path.hlsl);
    let base = translate_user_data(&shader(&words)).unwrap_or_else(|e| panic!("{e}")).user_data_base.expect("user data base");
    assert_eq!(base, 8);
    let consts = [0x1111_0001, 0x2222_0002, 0x3333_0003];
    let ud = user_data_abi::pack(&layout_root(), base, &[RootArg::Table(0x5eed_0008), RootArg::Constants(&consts)]).unwrap_or_else(|e| panic!("{e}"));
    let out = draw_user_data(&words, &ud).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(out, [0x5eed_0008, consts[0], consts[1], consts[2]]);
}
