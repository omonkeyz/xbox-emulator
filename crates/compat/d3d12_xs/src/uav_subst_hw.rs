use super::*;

const IID_DESCRIPTOR_HEAP: Guid = Guid { d1: 0x8efb_471d, d2: 0x616c, d3: 0x4f49, d4: [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51] };
const IID_HEAP: Guid = Guid { d1: 0x6b3b_2502, d2: 0x6e51, d3: 0x45b3, d4: [0x90, 0xee, 0x98, 0x84, 0x26, 0x5e, 0x8d, 0xf3] };
const ST_NON_PIXEL_SRV: u32 = 0x40;
const PC_SET_DESCRIPTOR_HEAPS: usize = 28;
const PC_SET_COMPUTE_ROOT_TABLE: usize = 31;
const PC_SET_COMPUTE_ROOT_SRV: usize = 39;
const D_CREATE_HEAP: usize = 28;
const D_PLACED: usize = 29;
const D_PLACED_X: usize = 44;
const D_COMMITTED: usize = 27;
const D_CREATE_SRV: usize = 18;
const D_CREATE_UAV: usize = 19;
const W: usize = 64;
const H: usize = 16;

fn wrapped() -> usize {
    static D: OnceLock<usize> = OnceLock::new();
    *D.get_or_init(|| crate::device_wrap::wrap_device(gpu().dev as *mut c_void) as usize)
}

fn tex_desc(format: u32, flags: u32) -> [u8; 0x38] {
    let mut d = [0u8; 0x38];
    d[0..4].copy_from_slice(&3u32.to_le_bytes());
    d[0x10..0x18].copy_from_slice(&(W as u64).to_le_bytes());
    d[0x18..0x1c].copy_from_slice(&(H as u32).to_le_bytes());
    d[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
    d[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
    d[0x20..0x24].copy_from_slice(&format.to_le_bytes());
    d[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
    d[0x30..0x34].copy_from_slice(&flags.to_le_bytes());
    d
}

fn transition(res: usize, before: u32, after: u32) -> [u64; 4] {
    [0, res as u64, 0xffff_ffff | (before as u64) << 32, after as u64]
}

unsafe fn native(g: &Gpu) -> bool {
    let support = unsafe { crate::uav_subst::host_support1(g.dev as *mut c_void, crate::uav_subst::R9G9B9E5_SHAREDEXP) }.expect("CheckFeatureSupport(FORMAT_SUPPORT, R9G9B9E5)");
    let who = if g.warp { "WARP" } else { "hardware" };
    if support & crate::uav_subst::SUPPORT1_TYPED_UAV != 0 {
        eprintln!("skipped: this adapter ({who}) binds R9G9B9E5 as a typed UAV natively (Support1={support:#x})");
        return true;
    }
    eprintln!("{who}: R9G9B9E5 Support1={support:#x}, no typed UAV");
    false
}

unsafe fn committed(dev: usize, heap: &[u32; 5], d: &[u8; 0x38], state: u32) -> (i32, *mut c_void) {
    let mut r: *mut c_void = core::ptr::null_mut();
    let hr = call!(dev, D_COMMITTED, (*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
        heap.as_ptr(), 0, d.as_ptr(), state, core::ptr::null(), &IID_RESOURCE, &mut r);
    (hr, r)
}

unsafe fn format_of(res: *mut c_void) -> u32 {
    let mut got = [0u8; 0x38];
    call!(res, 10, (*mut u8) -> *mut u8, got.as_mut_ptr());
    u32::from_le_bytes(got[0x20..0x24].try_into().unwrap())
}

unsafe fn descriptor_heap(dev: usize, n: u32, shader_visible: bool) -> (*mut c_void, usize, u64, usize) {
    let hd: [u32; 4] = [0, n, shader_visible as u32, 0];
    let mut dh: *mut c_void = core::ptr::null_mut();
    assert!(call!(dev, 14, (*const u32, *const Guid, *mut *mut c_void) -> i32, hd.as_ptr(), &IID_DESCRIPTOR_HEAP, &mut dh) >= 0);
    let mut cpu0 = 0usize;
    call!(dh, 9, (*mut usize) -> *mut usize, &mut cpu0);
    let mut gpu0 = 0u64;
    if shader_visible {
        call!(dh, 10, (*mut u64) -> *mut u64, &mut gpu0);
    }
    (dh, cpu0, gpu0, call!(dev, 15, (u32) -> u32, 0) as usize)
}

fn view(format: u32, dim: u32, mapping: u32) -> [u8; 40] {
    let mut v = [0u8; 40];
    v[0..4].copy_from_slice(&format.to_le_bytes());
    v[4..8].copy_from_slice(&dim.to_le_bytes());
    v[8..12].copy_from_slice(&mapping.to_le_bytes());
    if mapping != 0 {
        v[20..24].copy_from_slice(&1u32.to_le_bytes());
    }
    v
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_rgb9e5_typed_uav_texture_is_substituted() {
    use crate::uav_subst::{f16_to_f32, f32_to_f16, rgb9e5_decode, rgb9e5_encode, rgb9e5_step};
    let g = gpu();
    unsafe {
        if native(g) {
            return;
        }
        let real = g.dev;
        let heap: [u32; 5] = [HEAP_DEFAULT, 0, 0, 1, 1];
        let d = tex_desc(67, FLAG_UAV);
        let (hr, _) = committed(real, &heap, &d, ST_UAV);
        assert_eq!(hr, 0x8007_0057u32 as i32, "the host accepted R9G9B9E5 + ALLOW_UAV");
        let dev = wrapped();
        let mut tex: *mut c_void = core::ptr::null_mut();
        let hr = call!(dev, crate::console_slots_dev2::SLOT_COMMITTED_X, (*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
            heap.as_ptr(), 0, d.as_ptr(), ST_UAV, core::ptr::null(), &IID_RESOURCE, &mut tex);
        assert!(hr >= 0 && !tex.is_null(), "d3d12_xs CreateCommittedResourceX hr={hr:#010x}");
        assert_eq!(format_of(tex), 10, "host texture is not RGBA16F");
        assert_eq!(crate::uav_subst::lookup(tex), Some(crate::uav_subst::TABLE[0]));

        let (dh, cpu0, gpu0, inc) = descriptor_heap(real, 6, true);
        call!(dev, D_CREATE_UAV, (*mut c_void, *mut c_void, *const u8, usize) -> (), tex, core::ptr::null_mut(), view(67, 4, 0).as_ptr(), cpu0);
        call!(dev, D_CREATE_SRV, (*mut c_void, *const u8, usize) -> (), tex, view(67, 4, 0x1688).as_ptr(), cpu0 + inc);
        call!(real, D_CREATE_UAV, (*mut c_void, *mut c_void, *const u8, usize) -> (), core::ptr::null_mut(), core::ptr::null_mut(), view(10, 4, 0).as_ptr(), cpu0 + 2 * inc);
        call!(real, D_CREATE_SRV, (*mut c_void, *const u8, usize) -> (), core::ptr::null_mut(), view(10, 4, 0x1688).as_ptr(), cpu0 + 3 * inc);
        call!(dev, D_CREATE_SRV, (*mut c_void, *const u8, usize) -> (), tex, core::ptr::null::<u8>(), cpu0 + 4 * inc);
        call!(real, D_CREATE_SRV, (*mut c_void, *const u8, usize) -> (), tex, view(0, 4, 0x1688).as_ptr(), cpu0 + 5 * inc);

        let n = W * H;
        let mut seed = 0x1234_5678u32;
        let mut next = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); seed };
        let mut input = vec![[0f32; 4]; n];
        for (i, t) in input.iter_mut().enumerate() {
            let rgb = if i < n / 2 {
                rgb9e5_decode(next())
            } else {
                [0; 3].map(|_| {
                    let u = next();
                    (2f32.powi((u >> 24) as i32 % 41 - 25) * (1.0 + (u & 0xffff) as f32 / 65536.0)).min(65408.0)
                })
            };
            *t = [rgb[0], rgb[1], rgb[2], 0.25];
        }
        input[n / 2] = [0.0, 0.0, 0.0, 0.25];
        input[n / 2 + 1] = [65408.0, 1.0, 3.0e-7, 0.25];
        let src = buffer(g.dev, HEAP_UPLOAD, (n * 16) as u64, 0, ST_GENERIC_READ);
        core::ptr::copy_nonoverlapping(input.as_ptr() as *const u8, src.cpu, n * 16);
        let outs: Vec<Buf> = (0..3).map(|_| buffer(g.dev, HEAP_DEFAULT, (n * 16) as u64, FLAG_UAV, ST_UAV)).collect();
        let rbs: Vec<Buf> = (0..3).map(|_| buffer(g.dev, HEAP_READBACK, (n * 16) as u64, 0, ST_COPY_DEST)).collect();

        let rs_decl = "#define RS \"DescriptorTable(UAV(u0)), DescriptorTable(SRV(t0)), SRV(t1), UAV(u1)\"\n\
            RWTexture2D<float4> tex_out : register(u0);\nTexture2D<float4> tex_in : register(t0);\n\
            ByteAddressBuffer inp : register(t1);\nRWByteAddressBuffer res : register(u1);\n";
        let write = format!("{rs_decl}[RootSignature(RS)] [numthreads(8, 8, 1)]\nvoid main(uint3 id : SV_DispatchThreadID) {{ uint i = id.y * {W} + id.x; tex_out[id.xy] = asfloat(inp.Load4(i * 16)); }}\n");
        let read = format!("{rs_decl}[RootSignature(RS)] [numthreads(8, 8, 1)]\nvoid main(uint3 id : SV_DispatchThreadID) {{ uint i = id.y * {W} + id.x; res.Store4(i * 16, asuint(tex_in.Load(int3(id.xy, 0)))); }}\n");
        let (rs, pso_w) = compute_pso(g.dev, &write, None);
        let (_, pso_r) = compute_pso(g.dev, &read, Some(rs));
        let l = raw_list(g.dev);
        let heaps = [dh];
        call!(l, PC_SET_DESCRIPTOR_HEAPS, (u32, *const *mut c_void) -> (), 1, heaps.as_ptr());
        call!(l, PC_SET_COMPUTE_ROOT_SIGNATURE, (*mut c_void) -> (), rs);
        call!(l, PC_SET_COMPUTE_ROOT_TABLE, (u32, u64) -> (), 0, gpu0);
        call!(l, PC_SET_COMPUTE_ROOT_TABLE, (u32, u64) -> (), 1, gpu0 + 3 * inc as u64);
        call!(l, PC_SET_COMPUTE_ROOT_SRV, (u32, u64) -> (), 2, src.va);
        call!(l, PC_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 3, outs[0].va);
        call!(l, X_SET_PSO, (*mut c_void) -> (), pso_w);
        call!(l, X_DISPATCH, (u32, u32, u32) -> (), (W / 8) as u32, (H / 8) as u32, 1);
        let b = transition(tex as usize, ST_UAV, ST_NON_PIXEL_SRV);
        call!(l, X_BARRIER, (u32, *const u64) -> (), 1, b.as_ptr());
        call!(l, X_SET_PSO, (*mut c_void) -> (), pso_r);
        call!(l, PC_SET_COMPUTE_ROOT_TABLE, (u32, u64) -> (), 0, gpu0 + 2 * inc as u64);
        for (k, srv) in [1u64, 4, 5].into_iter().enumerate() {
            call!(l, PC_SET_COMPUTE_ROOT_TABLE, (u32, u64) -> (), 1, gpu0 + srv * inc as u64);
            call!(l, PC_SET_COMPUTE_ROOT_UAV, (u32, u64) -> (), 3, outs[k].va);
            call!(l, X_DISPATCH, (u32, u32, u32) -> (), (W / 8) as u32, (H / 8) as u32, 1);
        }
        for (o, rb) in outs.iter().zip(&rbs) {
            let b = transition(o.res, ST_UAV, ST_COPY_SOURCE);
            call!(l, X_BARRIER, (u32, *const u64) -> (), 1, b.as_ptr());
            call!(l, X_COPY_BUFFER_REGION, (usize, u64, usize, u64, u64) -> (), rb.res, 0, o.res, 0, (n * 16) as u64);
        }
        assert!(call!(l, X_CLOSE, () -> i32) >= 0, "compute list failed to close");
        let f = fence(g.dev);
        submit(g.a, l, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)), "compute work did not finish");
        let back = core::slice::from_raw_parts(rbs[0].cpu as *const [f32; 4], n);
        for (k, what) in [(1, "null-desc SRV"), (2, "UNKNOWN-format SRV")] {
            let other = core::slice::from_raw_parts(rbs[k].cpu as *const [f32; 4], n);
            assert_eq!(other, back, "{what} reads differ from the R9G9B9E5 SRV");
        }
        for (i, (want, got)) in input.iter().zip(back).enumerate() {
            let rgb = [want[0], want[1], want[2]];
            assert_eq!(got[3], 1.0, "texel {i}: SRV alpha of an RGB9E5 view must read 1.0, got {}", got[3]);
            if i < n / 2 {
                assert_eq!(&got[..3], &rgb, "texel {i}: RGB9E5 value not stored exactly");
                continue;
            }
            let q = rgb9e5_decode(rgb9e5_encode(rgb));
            let step = rgb9e5_step(rgb);
            for c in 0..3 {
                let h = f32_to_f16(rgb[c]);
                let (lo, hi) = (f16_to_f32(h.saturating_sub(1)).min(f16_to_f32(h)), f16_to_f32(h + 1));
                assert!(got[c] >= lo && got[c] <= hi, "texel {i}.{c}: {} is not an f16 neighbour of {}", got[c], rgb[c]);
                assert!((got[c] - q[c]).abs() <= step, "texel {i}.{c}: {} vs RGB9E5 {} (quantum {step})", got[c], q[c]);
            }
        }

        let pitch = (W * 8).next_multiple_of(256);
        let console_fp = buffer(g.dev, HEAP_READBACK, (pitch * H) as u64, 0, ST_COPY_DEST);
        let host_fp = buffer(g.dev, HEAP_READBACK, (pitch * H) as u64, 0, ST_COPY_DEST);
        core::ptr::write_bytes(console_fp.cpu, 0xcd, pitch * H);
        core::ptr::write_bytes(host_fp.cpu, 0xcd, pitch * H);
        let footprint = |res: usize, fmt: u32, row: usize| {
            let mut loc = [0u8; 0x30];
            loc[0..8].copy_from_slice(&(res as u64).to_le_bytes());
            loc[8..12].copy_from_slice(&1u32.to_le_bytes());
            loc[0x18..0x1c].copy_from_slice(&fmt.to_le_bytes());
            loc[0x1c..0x20].copy_from_slice(&(W as u32).to_le_bytes());
            loc[0x20..0x24].copy_from_slice(&(H as u32).to_le_bytes());
            loc[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
            loc[0x28..0x2c].copy_from_slice(&(row as u32).to_le_bytes());
            loc
        };
        let mut sub = [0u8; 0x30];
        sub[0..8].copy_from_slice(&(tex as u64).to_le_bytes());
        let refused = crate::uav_subst::refused_copies();
        let x = title_list(g.dev);
        let b = transition(tex as usize, ST_NON_PIXEL_SRV, ST_COPY_SOURCE);
        call!(x, X_BARRIER, (u32, *const u64) -> (), 1, b.as_ptr());
        let console = footprint(console_fp.res, 67, (W * 4).next_multiple_of(256));
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), console.as_ptr(), 0, 0, 0, sub.as_ptr(), core::ptr::null());
        let host = footprint(host_fp.res, 10, pitch);
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), host.as_ptr(), 0, 0, 0, sub.as_ptr(), core::ptr::null());
        assert!(call!(x, X_CLOSE, () -> i32) >= 0, "copy list failed to close");
        let f = fence(g.dev);
        submit(g.a, x, f, 1);
        assert!(reached(f, 1, Duration::from_secs(5)), "copy work did not finish");
        assert_eq!(crate::uav_subst::refused_copies(), refused + 2, "both copies out of the substituted texture are refused");
        assert!(core::slice::from_raw_parts(console_fp.cpu, pitch * H).iter().all(|&b| b == 0xcd), "the R9G9B9E5-footprint copy was not refused");
        assert!(core::slice::from_raw_parts(host_fp.cpu, pitch * H).iter().all(|&b| b == 0xcd), "the RGBA16F-footprint copy was not refused");
        eprintln!("{n} texels verified on {}: {} exact RGB9E5, {} within one RGB9E5 quantum; copies refused", if g.warp { "WARP" } else { "hardware" }, n / 2, n - n / 2);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_rgb9e5_placed_textures_are_substituted_and_cpu_visible_heaps_are_not() {
    let g = gpu();
    unsafe {
        if native(g) {
            return;
        }
        let dev = wrapped();
        let d = tex_desc(67, FLAG_UAV);
        let mut hd = [0u8; 48];
        hd[0..8].copy_from_slice(&0x40_0000u64.to_le_bytes());
        hd[8..12].copy_from_slice(&HEAP_DEFAULT.to_le_bytes());
        hd[20..24].copy_from_slice(&1u32.to_le_bytes());
        hd[24..28].copy_from_slice(&1u32.to_le_bytes());
        hd[40..44].copy_from_slice(&0x44u32.to_le_bytes());
        let mut heap: *mut c_void = core::ptr::null_mut();
        assert!(call!(g.dev, D_CREATE_HEAP, (*const u8, *const Guid, *mut *mut c_void) -> i32, hd.as_ptr(), &IID_HEAP, &mut heap) >= 0);
        let mut r: *mut c_void = core::ptr::null_mut();
        let hr = call!(dev, D_PLACED, (*mut c_void, u64, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
            heap, 0x1_0000, d.as_ptr(), ST_UAV, core::ptr::null(), &IID_RESOURCE, &mut r);
        assert!(hr >= 0 && !r.is_null(), "slot 29 hr={hr:#010x}");
        assert_eq!((format_of(r), crate::uav_subst::lookup(r)), (10, Some(crate::uav_subst::TABLE[0])), "slot 29");

        let mut x: *mut c_void = core::ptr::null_mut();
        let va = 0x7320_0000_0000u64;
        let hr = call!(dev, D_PLACED_X, (u64, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
            va, d.as_ptr(), ST_UAV, core::ptr::null(), &IID_RESOURCE, &mut x);
        assert!(hr >= 0 && !x.is_null(), "slot 44 hr={hr:#010x}");
        assert_eq!((format_of(x), crate::uav_subst::lookup(x)), (10, Some(crate::uav_subst::TABLE[0])), "slot 44");
        assert_eq!(crate::placed::lookup(x).map(|p| (p.va, u32::from_le_bytes(p.desc[0x20..0x24].try_into().unwrap()))), Some((va, 67)), "slot 44 keeps the console desc for title memory");

        let cpu_heap: [u32; 5] = [4, 3, 1, 1, 1];
        let (hr, c) = committed(dev, &cpu_heap, &d, ST_COMMON);
        assert!(hr < 0 && c.is_null(), "a CPU-visible R9G9B9E5 UAV texture was created (hr={hr:#010x}); it must stay unsubstituted and rejected by the host");
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_uav_subst_view_hook_cost_per_call() {
    let g = gpu();
    unsafe {
        if native(g) {
            return;
        }
        let dev = wrapped();
        let (hr, tex) = committed(dev, &[HEAP_DEFAULT, 0, 0, 1, 1], &tex_desc(67, FLAG_UAV), ST_UAV);
        assert!(hr >= 0 && !tex.is_null(), "substituted texture hr={hr:#010x}");
        let real = g.dev as *mut c_void;
        let (hr, plain) = committed(g.dev, &[HEAP_DEFAULT, 0, 0, 1, 1], &tex_desc(28, FLAG_UAV), ST_UAV);
        assert!(hr >= 0, "plain texture hr={hr:#010x}");
        let (_dh, cpu0, _, _) = descriptor_heap(g.dev, 1, false);
        type Srv = unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, usize);
        type Uav = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u8, usize);
        let srv_hooked: Srv = core::mem::transmute(vs(real, D_CREATE_SRV));
        let uav_hooked: Uav = core::mem::transmute(vs(real, D_CREATE_UAV));
        let srv_plain: Srv = core::mem::transmute(crate::uav_subst::unhooked(real, crate::uav_subst::View::Srv).expect("SRV creation is not hooked"));
        let uav_plain: Uav = core::mem::transmute(crate::uav_subst::unhooked(real, crate::uav_subst::View::Uav).expect("UAV creation is not hooked"));
        assert_ne!(srv_hooked as usize, srv_plain as usize);
        let srv = view(28, 4, 0x1688);
        let uav = view(28, 4, 0);
        const CALLS: u32 = 100_000;
        const ROUNDS: usize = 9;
        let time = |f: &dyn Fn()| {
            let t = Instant::now();
            for _ in 0..CALLS {
                f();
            }
            t.elapsed().as_nanos() as f64 / CALLS as f64
        };
        let cases: [(&str, Box<dyn Fn()>, Box<dyn Fn()>); 3] = [
            ("SRV, format 28", Box::new(|| srv_hooked(real, plain, srv.as_ptr(), cpu0)), Box::new(|| srv_plain(real, plain, srv.as_ptr(), cpu0))),
            ("UAV, format 28", Box::new(|| uav_hooked(real, plain, core::ptr::null_mut(), uav.as_ptr(), cpu0)), Box::new(|| uav_plain(real, plain, core::ptr::null_mut(), uav.as_ptr(), cpu0))),
            ("SRV, null desc", Box::new(|| srv_hooked(real, plain, core::ptr::null(), cpu0)), Box::new(|| srv_plain(real, plain, core::ptr::null(), cpu0))),
        ];
        for (what, hooked, unhooked) in &cases {
            let (mut h, mut u) = (f64::MAX, f64::MAX);
            for _ in 0..ROUNDS {
                h = h.min(time(hooked.as_ref()));
                u = u.min(time(unhooked.as_ref()));
            }
            eprintln!("{} {what}: hooked {h:.1} ns/call, unhooked {u:.1} ns/call, difference {:.1} ns (best of {ROUNDS} rounds of {CALLS} calls)", if g.warp { "WARP" } else { "hardware" }, h - u);
        }
    }
}
