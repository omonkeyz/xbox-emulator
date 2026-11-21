#![allow(dead_code)]

use compat_common::record;
use core::ffi::c_void;
use std::sync::Arc;

pub const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
pub const SUB_ROOT_SIGNATURE: u32 = 0;
pub const SUB_VS: u32 = 1;
pub const SUB_PS: u32 = 2;
pub const SUB_DS: u32 = 3;
pub const SUB_GS: u32 = 5;
pub const SUB_CS: u32 = 6;
pub const SUB_AS: u32 = 24;
pub const SUB_MS: u32 = 25;
pub const PC_DEVICE2_CREATE_PIPELINE_STATE: usize = 47;
pub const PC_LIST6_DISPATCH_MESH: usize = 79;

#[repr(C)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);
pub const IID_DEVICE2: Guid = Guid(0x30baa41e, 0xb15b, 0x475c, [0xa0, 0xbb, 0x1a, 0xf5, 0xc5, 0xb6, 0x43, 0x28]);
pub const IID_LIST6: Guid = Guid(0xc3827890, 0xe548, 0x4cfa, [0x96, 0xcf, 0x56, 0x89, 0xa9, 0x37, 0x0f, 0x80]);

pub fn inner_layout(ty: u32) -> Option<(usize, usize)> {
    Some(match ty {
        0 => (8, 8),
        1..=6 | 24 | 25 => (8, 16),
        7 => (8, 32),
        8 => (4, 328),
        9 | 13 | 14 | 16 | 18 | 20 => (4, 4),
        10 => (4, 44),
        11 => (4, 52),
        21 => (4, 56),
        12 | 19 => (8, 16),
        15 => (4, 36),
        17 => (4, 8),
        22 => (8, 24),
        _ => return None,
    })
}

fn align(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

pub fn parse(stream: &[u8]) -> Result<Vec<(u32, usize)>, String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < stream.len() {
        let ty = u32::from_le_bytes(stream.get(at..at + 4).ok_or("truncated subobject type")?.try_into().unwrap());
        let (a, n) = inner_layout(ty).ok_or_else(|| format!("pipeline subobject type {ty} at {at:#x} unsupported"))?;
        let inner = at + align(4, a);
        if inner + n > stream.len() {
            return Err(format!("pipeline subobject type {ty} at {at:#x} runs past the stream end"));
        }
        if out.iter().any(|(t, _)| *t == ty) {
            return Err(format!("pipeline subobject type {ty} appears twice"));
        }
        out.push((ty, inner));
        at = align(inner + n, 8);
    }
    Ok(out)
}

fn bytecode(stream: &[u8], inner: usize) -> (u64, u64) {
    let p = u64::from_le_bytes(stream[inner..inner + 8].try_into().unwrap());
    let n = u64::from_le_bytes(stream[inner + 8..inner + 16].try_into().unwrap());
    (p, n)
}

pub fn validate(stream: &[u8], subs: &[(u32, usize)]) -> Result<(), String> {
    let has = |t: u32| subs.iter().find(|(x, _)| *x == t).map(|(_, o)| bytecode(stream, *o).0 != 0).unwrap_or(false);
    if has(SUB_MS) && has(SUB_VS) {
        return Err("When mesh shader is specified, vertex shader must be turned off".into());
    }
    if has(SUB_AS) && !has(SUB_MS) {
        return Err("amplification shader without a mesh shader".into());
    }
    Ok(())
}

pub const AS_ROOT_FLAG_ERR: &str = "When an amplification shader is used, the root signature must use D3D12XBOX_ROOT_SIGNATURE_FLAG_FORCE_MEMORY_BASED_ABI flag";

pub fn as_root_rule(stream: &[u8], subs: &[(u32, usize)], flags_of: &dyn Fn(u64) -> Option<u32>) -> Result<(), String> {
    let code = |t: u32| subs.iter().find(|(x, _)| *x == t).map(|(_, o)| bytecode(stream, *o)).filter(|(p, n)| *p != 0 && *n != 0);
    let Some((ap, an)) = code(SUB_AS) else { return Ok(()) };
    if code(SUB_MS).is_none() {
        return Ok(());
    }
    let rs = subs.iter().find(|(t, _)| *t == SUB_ROOT_SIGNATURE).map(|(_, o)| u64::from_le_bytes(stream[*o..*o + 8].try_into().unwrap())).unwrap_or(0);
    let flags = if rs != 0 {
        flags_of(rs).ok_or_else(|| format!("amplification PSO root signature {rs:#x} was not created through the console CreateRootSignature, its flags are unknown"))?
    } else {
        let blob = unsafe { core::slice::from_raw_parts(ap as *const u8, an as usize) };
        crate::console_slots_mesh::console_root_flags(blob).filter(|_| blob.get(0..4) == Some(b"DXBC")).ok_or("amplification PSO has no root signature subobject and its shader embeds none")?
    };
    if flags & crate::console_slots_mesh::FORCE_MEMORY_BASED_ABI == 0 {
        return Err(AS_ROOT_FLAG_ERR.into());
    }
    Ok(())
}

pub fn rt_formats(stream: &[u8], subs: &[(u32, usize)]) -> (u32, [u32; 8]) {
    let Some((_, o)) = subs.iter().find(|(t, _)| *t == 15) else { return (0, [0; 8]) };
    let mut f = [0u32; 8];
    for (k, v) in f.iter_mut().enumerate() {
        *v = u32::from_le_bytes(stream[o + 4 * k..o + 4 * k + 4].try_into().unwrap());
    }
    (u32::from_le_bytes(stream[o + 32..o + 36].try_into().unwrap()), f)
}

pub fn stage_rtfmt(ty: u32, (n, f): (u32, [u32; 8])) -> [u8; 8] {
    if ty == SUB_PS { crate::shader_xlat::export_formats(n, &f) } else { [0; 8] }
}

/// Translates every shader subobject with `xlat(type, blob, render targets, feeds)`, where
/// `feeds` is the pixel shader's blob for the stage that feeds it (MS, else GS, else DS,
/// else VS), so that stage can repeat the pixel shader's interpolation modifiers.
pub fn translate_stream(stream: &[u8], xlat: &dyn Fn(u32, &[u8], (u32, [u32; 8]), Option<&[u8]>) -> Result<Arc<Vec<u8>>, String>) -> Result<(Vec<u8>, Vec<Arc<Vec<u8>>>), String> {
    let subs = parse(stream)?;
    validate(stream, &subs)?;
    let rts = rt_formats(stream, &subs);
    let code_of = |t: u32| subs.iter().find(|(x, _)| *x == t).map(|(_, o)| bytecode(stream, *o)).filter(|(p, n)| *p != 0 && *n != 0);
    let feeder = [SUB_MS, SUB_GS, SUB_DS, SUB_VS].into_iter().find(|&t| code_of(t).is_some());
    let ps = code_of(SUB_PS).map(|(p, n)| unsafe { core::slice::from_raw_parts(p as *const u8, n as usize) });
    let mut out = stream.to_vec();
    let mut keep = Vec::new();
    for &(ty, inner) in &subs {
        if !matches!(ty, 1..=6 | SUB_AS | SUB_MS) {
            continue;
        }
        let (p, n) = bytecode(stream, inner);
        if p == 0 || n == 0 {
            continue;
        }
        let blob = unsafe { core::slice::from_raw_parts(p as *const u8, n as usize) };
        let feeds = if Some(ty) == feeder { ps } else { None };
        let code = xlat(ty, blob, rts, feeds).map_err(|e| format!("subobject {ty}: {e}"))?;
        out[inner..inner + 8].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
        out[inner + 8..inner + 16].copy_from_slice(&(code.len() as u64).to_le_bytes());
        keep.push(code);
    }
    Ok((out, keep))
}

pub unsafe fn create_pipeline_state(real: *mut c_void, desc: *const [u64; 2], riid: *const c_void, ppv: *mut *mut c_void) -> i32 {
    if desc.is_null() || real.is_null() || ppv.is_null() {
        return E_INVALIDARG;
    }
    let [size, ptr] = *desc;
    if ptr == 0 || size == 0 {
        return E_INVALIDARG;
    }
    let stream = core::slice::from_raw_parts(ptr as *const u8, size as usize);
    let rule = parse(stream).and_then(|s| as_root_rule(stream, &s, &|rs| crate::console_slots_mesh::root_flags(rs as usize)));
    if let Err(e) = rule {
        record("d3d12_xs", "CreatePipelineState", &format!("{e} -> E_INVALIDARG"));
        return E_INVALIDARG;
    }
    let root = parse(stream).ok().and_then(|s| s.iter().find(|(t, _)| *t == 0).map(|&(_, o)| bytecode(stream, o).0)).unwrap_or(0);
    let pc_centroid = crate::centroid_bind::tag_rtfmt(root, [0; 8])[0] & crate::centroid_bind::RTFMT_PC_CENTROID != 0;
    let xlat = |ty: u32, b: &[u8], (n, f): (u32, [u32; 8]), feeds: Option<&[u8]>| {
        let link = feeds.map(|ps| crate::shader_xlat::ps_link(ps, pc_centroid)).unwrap_or_else(crate::shader_xlat::no_link);
        if ty == SUB_MS && root != 0 && crate::shader_xlat::needs_external_root_data(b) {
            if let Some(data) = crate::console_slots_mesh::root_data(root as *mut c_void) {
                return crate::shader_xlat::translate_mesh_with_root(b, &data, &link).map_err(|e| e.to_string());
            }
        }
        let k = stage_rtfmt(ty, (n, f));
        let k = if ty == SUB_PS { crate::centroid_bind::tag_rtfmt(root, k) } else { k };
        crate::shader_xlat::translate_for_targets(b.as_ptr(), b.len(), k, &link).map_err(|e| e.to_string())
    };
    let (out, keep) = match translate_stream(stream, &xlat) {
        Ok(v) => v,
        Err(e) => {
            record("d3d12_xs", "CreatePipelineState", &format!("stream rejected: {e} -> E_INVALIDARG"));
            return E_INVALIDARG;
        }
    };
    let vt = *(real as *const *const usize);
    let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(*vt);
    let mut dev2: *mut c_void = core::ptr::null_mut();
    let hr = qi(real, &IID_DEVICE2, &mut dev2);
    if hr < 0 || dev2.is_null() {
        return hr;
    }
    let vt2 = *(dev2 as *const *const usize);
    let f: unsafe extern "system" fn(*mut c_void, *const [u64; 2], *const c_void, *mut *mut c_void) -> i32 = core::mem::transmute(*vt2.add(PC_DEVICE2_CREATE_PIPELINE_STATE));
    let d = [out.len() as u64, out.as_ptr() as u64];
    let hr = f(dev2, &d, riid, ppv);
    if hr >= 0 && !(*ppv).is_null() && parse(stream).is_ok_and(|s| s.iter().any(|(t, o)| *t == SUB_AS && bytecode(stream, *o).0 != 0)) {
        crate::console_slots_mesh::note_as_pso(*ppv as usize);
    }
    if hr >= 0 && !(*ppv).is_null() {
        crate::dbcount::note_stream_pso(*ppv as usize, &out, &keep);
    }
    let rel: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt2.add(2));
    rel(dev2);
    drop(keep);
    hr
}

pub fn dispatch_mesh_args_ok(_x: u32, y: u32, z: u32) -> bool {
    y == 1 && z == 1
}

pub unsafe fn dispatch_mesh(list6: *mut c_void, x: u32, y: u32, z: u32) {
    if !dispatch_mesh_args_ok(x, y, z) {
        record("d3d12_xs", "DispatchMesh", &format!("ThreadGroupCountY and Z should be 1 (got {x},{y},{z}): dispatch skipped"));
        return;
    }
    if list6.is_null() {
        return;
    }
    let vt = *(list6 as *const *const usize);
    let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(*vt);
    let mut l6: *mut c_void = core::ptr::null_mut();
    let hr = qi(list6, &IID_LIST6, &mut l6);
    if hr < 0 || l6.is_null() {
        record("d3d12_xs", "DispatchMesh", &format!("PC command list has no ID3D12GraphicsCommandList6 (hr={hr:#010x}): dispatch not executed"));
        return;
    }
    let vt6 = *(l6 as *const *const usize);
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, u32) = core::mem::transmute(*vt6.add(PC_LIST6_DISPATCH_MESH));
    f(l6, x, y, z);
    let rel: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt6.add(2));
    rel(l6);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(s: &mut Vec<u8>, ty: u32, body: &[u8]) {
        let (a, n) = inner_layout(ty).unwrap();
        assert_eq!(body.len(), n);
        s.extend_from_slice(&ty.to_le_bytes());
        while s.len() % a != 0 {
            s.push(0);
        }
        s.extend_from_slice(body);
        while s.len() % 8 != 0 {
            s.push(0);
        }
    }

    fn bc(b: &[u8]) -> Vec<u8> {
        let mut v = (b.as_ptr() as u64).to_le_bytes().to_vec();
        v.extend_from_slice(&(b.len() as u64).to_le_bytes());
        v
    }

    #[test]
    fn depth_stencil1_is_56_bytes_with_bounds_last() {
        let mut s = Vec::new();
        let mut ds1 = vec![0u8; 56];
        ds1[52] = 1;
        push(&mut s, 21, &ds1);
        push(&mut s, 17, &[4, 0, 0, 0, 0, 0, 0, 0]);
        let subs = parse(&s).unwrap();
        assert_eq!(subs, vec![(21, 4), (17, 68)]);
        assert_eq!(s[4 + 52], 1);
        assert_eq!(inner_layout(11), Some((4, 52)));
    }

    #[test]
    fn stream_layout_and_translation() {
        let ms = vec![1u8, 2, 3];
        let ps = vec![9u8; 5];
        let mut s = Vec::new();
        push(&mut s, SUB_ROOT_SIGNATURE, &0x1234u64.to_le_bytes());
        push(&mut s, SUB_MS, &bc(&ms));
        push(&mut s, SUB_PS, &bc(&ps));
        let mut rtf = [0u8; 36];
        rtf[0] = 10;
        rtf[32] = 1;
        push(&mut s, 15, &rtf);
        push(&mut s, 17, &[1, 0, 0, 0, 0, 0, 0, 0]);
        let subs = parse(&s).unwrap();
        assert_eq!(subs, vec![(0, 8), (25, 24), (2, 48), (15, 68), (17, 108)]);
        let xl = |ty: u32, b: &[u8], rts: (u32, [u32; 8]), feeds: Option<&[u8]>| {
            assert_eq!(rts, (1, [10, 0, 0, 0, 0, 0, 0, 0]), "subobject {ty}");
            // The mesh shader feeds the pixel shader and is handed its blob.
            assert_eq!(feeds, if ty == SUB_MS { Some(&ps[..]) } else { None }, "subobject {ty}");
            Ok(Arc::new(b.iter().map(|x| x + 100).collect::<Vec<u8>>()))
        };
        let (out, keep) = translate_stream(&s, &xl).unwrap();
        assert_eq!(keep.len(), 2);
        let (p, n) = bytecode(&out, 24);
        assert_eq!(unsafe { core::slice::from_raw_parts(p as *const u8, n as usize) }, &[101, 102, 103]);
        let (p, n) = bytecode(&out, 48);
        assert_eq!(unsafe { core::slice::from_raw_parts(p as *const u8, n as usize) }, &[109; 5]);
        assert_eq!(&out[..8], &s[..8]);
        assert_eq!(&out[64..], &s[64..]);
    }

    #[test]
    fn stream_rules() {
        let b = vec![1u8; 4];
        let mut s = Vec::new();
        push(&mut s, SUB_VS, &bc(&b));
        push(&mut s, SUB_MS, &bc(&b));
        let subs = parse(&s).unwrap();
        assert_eq!(validate(&s, &subs).unwrap_err(), "When mesh shader is specified, vertex shader must be turned off");
        let mut s = Vec::new();
        push(&mut s, SUB_VS, &[0u8; 16]);
        push(&mut s, SUB_MS, &bc(&b));
        assert!(validate(&s, &parse(&s).unwrap()).is_ok());
        let mut s = Vec::new();
        push(&mut s, SUB_AS, &bc(&b));
        assert!(validate(&s, &parse(&s).unwrap()).is_err());
        let mut s = Vec::new();
        push(&mut s, SUB_MS, &bc(&b));
        push(&mut s, SUB_MS, &bc(&b));
        assert!(parse(&s).unwrap_err().contains("twice"));
        let mut s = 23u32.to_le_bytes().to_vec();
        s.extend_from_slice(&[0u8; 12]);
        assert!(parse(&s).unwrap_err().contains("type 23"));
        let mut s = Vec::new();
        push(&mut s, SUB_MS, &bc(&b));
        s.truncate(20);
        assert!(parse(&s).unwrap_err().contains("past the stream end"));
        let fail = |_: u32, _: &[u8], _: (u32, [u32; 8]), _: Option<&[u8]>| Err::<Arc<Vec<u8>>, String>("translate: x".into());
        let mut s = Vec::new();
        push(&mut s, SUB_MS, &bc(&b));
        assert!(translate_stream(&s, &fail).unwrap_err().contains("subobject 25"));
    }

    #[test]
    fn only_the_pixel_stage_gets_target_formats() {
        let rts = (2u32, [10u32, 26, 0, 0, 0, 0, 0, 0]);
        assert_eq!(stage_rtfmt(SUB_MS, rts), [0; 8]);
        assert_eq!(stage_rtfmt(SUB_AS, rts), [0; 8]);
        assert_eq!(stage_rtfmt(SUB_PS, rts), crate::shader_xlat::export_formats(2, &rts.1));
    }

    #[test]
    fn amplification_needs_force_memory_based_abi_root_signature() {
        let b = vec![1u8; 4];
        let known = |rs: u64| match rs {
            0x10 => Some(0x0100_0001),
            0x20 => Some(0x1),
            _ => None,
        };
        let stream = |rs: u64, with_as: bool| {
            let mut s = Vec::new();
            push(&mut s, SUB_ROOT_SIGNATURE, &rs.to_le_bytes());
            if with_as {
                push(&mut s, SUB_AS, &bc(&b));
            }
            push(&mut s, SUB_MS, &bc(&b));
            s
        };
        let check = |s: &Vec<u8>| as_root_rule(s, &parse(s).unwrap(), &known);
        assert!(check(&stream(0x10, true)).is_ok());
        assert_eq!(check(&stream(0x20, true)).unwrap_err(), AS_ROOT_FLAG_ERR);
        assert!(check(&stream(0x20, false)).is_ok());
        assert!(check(&stream(0x30, true)).unwrap_err().contains("flags are unknown"));
        let rts = |flags: u32| {
            let body: Vec<u8> = [1u32, 0, 24, 0, 24, flags].iter().flat_map(|x| x.to_le_bytes()).collect();
            crate::device_wrap::wrap_root_signature(&body)
        };
        for (flags, ok) in [(0x0100_0000u32, true), (0, false)] {
            let asb = rts(flags);
            let mut s = Vec::new();
            push(&mut s, SUB_AS, &bc(&asb));
            push(&mut s, SUB_MS, &bc(&b));
            assert_eq!(as_root_rule(&s, &parse(&s).unwrap(), &known).is_ok(), ok);
        }
        let mut s = Vec::new();
        push(&mut s, SUB_AS, &bc(&b));
        push(&mut s, SUB_MS, &bc(&b));
        assert!(as_root_rule(&s, &parse(&s).unwrap(), &known).unwrap_err().contains("embeds none"));
    }

    #[test]
    fn dispatch_mesh_console_rule() {
        assert!(dispatch_mesh_args_ok(7, 1, 1));
        assert!(!dispatch_mesh_args_ok(1, 2, 1));
        assert!(!dispatch_mesh_args_ok(1, 1, 2));
    }
}
