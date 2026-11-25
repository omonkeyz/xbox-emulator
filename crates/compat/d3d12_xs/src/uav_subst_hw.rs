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

unsafe fn prepare(g: &Gpu) -> bool {
    if crate::uav_subst::FORCE_NO_TYPED_UAV.load(core::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    let support = unsafe { crate::uav_subst::host_support1(g.dev as *mut c_void, crate::uav_subst::R9G9B9E5_SHAREDEXP) }.expect("CheckFeatureSupport(FORMAT_SUPPORT, R9G9B9E5)");
    let who = if g.warp { "WARP" } else { "hardware" };
    if support & crate::uav_subst::SUPPORT1_TYPED_UAV != 0 {
        crate::uav_subst::FORCE_NO_TYPED_UAV.store(true, core::sync::atomic::Ordering::Relaxed);
        eprintln!("{who} binds R9G9B9E5 as a typed UAV natively (Support1={support:#x}); substitution forced for this test process");
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
        let forced = prepare(g);
        let real = g.dev;
        let heap: [u32; 5] = [HEAP_DEFAULT, 0, 0, 1, 1];
        let d = tex_desc(67, FLAG_UAV);
        if !forced {
            let (hr, _) = committed(real, &heap, &d, ST_UAV);
            assert_eq!(hr, 0x8007_0057u32 as i32, "the host accepted R9G9B9E5 + ALLOW_UAV");
        }
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
        let console_pitch = (W * 4).next_multiple_of(256);
        let console_fp = buffer(g.dev, HEAP_READBACK, (console_pitch * H) as u64, 0, ST_COPY_DEST);
        let host_fp = buffer(g.dev, HEAP_READBACK, (pitch * H) as u64, 0, ST_COPY_DEST);
        core::ptr::write_bytes(console_fp.cpu, 0xcd, console_pitch * H);
        core::ptr::write_bytes(host_fp.cpu, 0xcd, pitch * H);
        let (refused, converted) = (crate::uav_subst::refused_copies(), crate::uav_subst::converted_copies());
        let x = title_list(g.dev);
        let b = transition(tex as usize, ST_NON_PIXEL_SRV, ST_COPY_SOURCE);
        call!(x, X_BARRIER, (u32, *const u64) -> (), 1, b.as_ptr());
        let sub = sub_loc(tex as usize, 0);
        let console = fp_loc(console_fp.res, 0, 67, [W as u32, H as u32, 1], console_pitch);
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), console.as_ptr(), 0, 0, 0, sub.as_ptr(), core::ptr::null());
        let host = fp_loc(host_fp.res, 0, 10, [W as u32, H as u32, 1], pitch);
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), host.as_ptr(), 0, 0, 0, sub.as_ptr(), core::ptr::null());
        run(g, x);
        assert_eq!(crate::uav_subst::refused_copies(), refused + 1, "the RGBA16F-footprint copy out of an R9G9B9E5 texture has no console meaning and is refused");
        assert_eq!(crate::uav_subst::converted_copies(), converted + 1, "the R9G9B9E5-footprint copy is converted");
        assert!(core::slice::from_raw_parts(host_fp.cpu, pitch * H).iter().all(|&b| b == 0xcd), "the RGBA16F-footprint copy was not refused");
        for (i, t) in back.iter().enumerate() {
            let got = u32::from_le_bytes(core::slice::from_raw_parts(console_fp.cpu.add((i / W) * console_pitch + (i % W) * 4), 4).try_into().unwrap());
            assert_eq!(got, spec_rgb9e5([t[0], t[1], t[2]]), "texel {i}: copy into the console layout");
        }
        eprintln!("{n} texels verified on {}: {} exact RGB9E5, {} within one RGB9E5 quantum; console-layout copy bit-exact, RGBA16F-footprint copy refused", if g.warp { "WARP" } else { "hardware" }, n / 2, n - n / 2);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_rgb9e5_placed_textures_are_substituted_and_cpu_visible_heaps_are_not() {
    let g = gpu();
    unsafe {
        let forced = prepare(g);
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
        if forced {
            assert!(hr >= 0 && !c.is_null() && crate::uav_subst::lookup(c).is_none() && format_of(c) == 67, "a CPU-visible R9G9B9E5 UAV texture must stay unsubstituted (hr={hr:#010x})");
        } else {
            assert!(hr < 0 && c.is_null(), "a CPU-visible R9G9B9E5 UAV texture was created (hr={hr:#010x}); it must stay unsubstituted and rejected by the host");
        }
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_uav_subst_view_hook_cost_per_call() {
    let g = gpu();
    unsafe {
        prepare(g);
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

fn spec_rgb9e5(c: [f32; 3]) -> u32 {
    const N: i32 = 9;
    const B: i32 = 15;
    const E_MAX: i32 = 31;
    let sharedexp_max = ((1 << N) - 1) as f64 / (1 << N) as f64 * 2f64.powi(E_MAX - B);
    let c = c.map(|x| if x.is_nan() { 0.0 } else { (x as f64).max(0.0).min(sharedexp_max) });
    let max_c = c[0].max(c[1]).max(c[2]);
    let floor_log2 = |x: f64| ((x.to_bits() >> 52) & 0x7ff) as i32 - 1023;
    let exp_shared_p = if max_c > 0.0 { floor_log2(max_c).max(-B - 1) } else { -B - 1 } + 1 + B;
    let scale = |e: i32| 2f64.powi(e - B - N);
    let max_s = (max_c / scale(exp_shared_p) + 0.5).floor();
    let exp_shared = if max_s == (1 << N) as f64 { exp_shared_p + 1 } else { exp_shared_p };
    let s = c.map(|x| (x / scale(exp_shared) + 0.5).floor() as u32);
    s[0] | s[1] << 9 | s[2] << 18 | (exp_shared as u32) << 27
}

#[test]
fn spec_reference_known_values() {
    assert_eq!(spec_rgb9e5([1.0, 0.0, 0.0]), 256 | 16 << 27);
    assert_eq!(spec_rgb9e5([65408.0, 0.0, 0.0]), 511 | 31 << 27);
    assert_eq!(spec_rgb9e5([f32::INFINITY, f32::NAN, -1.0]), 511 | 31 << 27);
    assert_eq!(spec_rgb9e5([0.0, -0.0, f32::NAN]), 0);
    assert_eq!(spec_rgb9e5([2f32.powi(-24), 0.0, 0.0]), 1);
    assert_eq!(spec_rgb9e5([1.0 - 2f32.powi(-10), 0.0, 0.0]), 256 | 16 << 27);
    assert_eq!(spec_rgb9e5([1.0, 3.0 * 2f32.powi(-9), 0.0]), 256 | 2 << 9 | 16 << 27);
    for e in 0..32 {
        for m in [1u32, 2, 255, 256, 511] {
            let v = m | m << 9 | m << 18 | e << 27;
            let back = spec_rgb9e5(crate::uav_subst::rgb9e5_decode(v));
            assert_eq!(crate::uav_subst::rgb9e5_decode(back), crate::uav_subst::rgb9e5_decode(v), "{v:#010x}");
        }
    }
}

fn fp_loc(res: usize, offset: u64, format: u32, size: [u32; 3], pitch: usize) -> [u8; 0x30] {
    let mut loc = [0u8; 0x30];
    loc[0..8].copy_from_slice(&(res as u64).to_le_bytes());
    loc[8..12].copy_from_slice(&1u32.to_le_bytes());
    loc[0x10..0x18].copy_from_slice(&offset.to_le_bytes());
    loc[0x18..0x1c].copy_from_slice(&format.to_le_bytes());
    loc[0x1c..0x20].copy_from_slice(&size[0].to_le_bytes());
    loc[0x20..0x24].copy_from_slice(&size[1].to_le_bytes());
    loc[0x24..0x28].copy_from_slice(&size[2].to_le_bytes());
    loc[0x28..0x2c].copy_from_slice(&(pitch as u32).to_le_bytes());
    loc
}

fn sub_loc(res: usize, sub: u32) -> [u8; 0x30] {
    let mut loc = [0u8; 0x30];
    loc[0..8].copy_from_slice(&(res as u64).to_le_bytes());
    loc[0x10..0x14].copy_from_slice(&sub.to_le_bytes());
    loc
}

unsafe fn run_on(q: usize, x: *mut c_void) {
    assert!(call!(x, X_CLOSE, () -> i32) >= 0, "list failed to close");
    let f = fence(gpu().dev);
    submit(q, x, f, 1);
    assert!(reached(f, 1, Duration::from_secs(10)), "list did not finish");
}

unsafe fn run(g: &Gpu, x: *mut c_void) {
    unsafe { run_on(g.a, x) }
}

fn tex2d(w: usize, h: usize, format: u32, flags: u32) -> [u8; 0x38] {
    let mut d = tex_desc(format, flags);
    d[0x10..0x18].copy_from_slice(&(w as u64).to_le_bytes());
    d[0x18..0x1c].copy_from_slice(&(h as u32).to_le_bytes());
    d
}

unsafe fn substituted(w: usize, h: usize) -> *mut c_void {
    let (hr, t) = unsafe { committed(wrapped(), &[HEAP_DEFAULT, 0, 0, 1, 1], &tex2d(w, h, 67, FLAG_UAV), ST_COMMON) };
    assert!(hr >= 0 && !t.is_null(), "substituted texture hr={hr:#010x}");
    assert_eq!((unsafe { format_of(t) }, unsafe { crate::uav_subst::lookup(t) }), (10, Some(crate::uav_subst::TABLE[0])));
    t
}

unsafe fn plain_rgb9e5(w: usize, h: usize) -> *mut c_void {
    let (hr, t) = unsafe { committed(gpu().dev, &[HEAP_DEFAULT, 0, 0, 1, 1], &tex2d(w, h, 67, 0), ST_COMMON) };
    assert!(hr >= 0 && !t.is_null(), "plain R9G9B9E5 texture hr={hr:#010x}");
    t
}

unsafe fn fill(g: &Gpu, tex: *mut c_void, format: u32, w: usize, h: usize, bpp: usize, texels: &[u8]) {
    let pitch = (w * bpp).next_multiple_of(256);
    let up = unsafe { buffer(g.dev, HEAP_UPLOAD, (pitch * h) as u64, 0, ST_GENERIC_READ) };
    for y in 0..h {
        unsafe { core::ptr::copy_nonoverlapping(texels[y * w * bpp..].as_ptr(), up.cpu.add(y * pitch), w * bpp) };
    }
    let l = unsafe { raw_list(g.dev) };
    let (dst, src) = (sub_loc(tex as usize, 0), fp_loc(up.res, 0, format, [w as u32, h as u32, 1], pitch));
    call!(l, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), dst.as_ptr(), 0, 0, 0, src.as_ptr(), core::ptr::null());
    unsafe { run(g, l) };
}

unsafe fn read(g: &Gpu, tex: *mut c_void, format: u32, w: usize, h: usize, bpp: usize) -> Vec<u8> {
    let pitch = (w * bpp).next_multiple_of(256);
    let rb = unsafe { buffer(g.dev, HEAP_READBACK, (pitch * h) as u64, 0, ST_COPY_DEST) };
    let l = unsafe { raw_list(g.dev) };
    let (dst, src) = (fp_loc(rb.res, 0, format, [w as u32, h as u32, 1], pitch), sub_loc(tex as usize, 0));
    call!(l, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), dst.as_ptr(), 0, 0, 0, src.as_ptr(), core::ptr::null());
    unsafe { run(g, l) };
    (0..h).flat_map(|y| unsafe { core::slice::from_raw_parts(rb.cpu.add(y * pitch), w * bpp) }.to_vec()).collect()
}

fn words(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn halves(b: &[u8]) -> Vec<[u16; 4]> {
    b.chunks_exact(8).map(|c| core::array::from_fn(|k| u16::from_le_bytes([c[2 * k], c[2 * k + 1]]))).collect()
}

fn f16_texels(n: usize) -> Vec<[u16; 4]> {
    (0..n as u32).map(|i| [i as u16, i.wrapping_mul(40503).wrapping_add(7) as u16, i.wrapping_mul(25173).wrapping_add(13849) as u16, 0x3c00]).collect()
}

fn encoded(t: [u16; 4]) -> u32 {
    spec_rgb9e5([t[0], t[1], t[2]].map(crate::uav_subst::f16_to_f32))
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_rgb9e5_copies_out_of_a_substituted_texture_are_encoded_bit_exact() {
    let g = gpu();
    unsafe {
        prepare(g);
        let (w, h) = (256usize, 256usize);
        let texels = f16_texels(w * h);
        let s = substituted(w, h);
        fill(g, s, 10, w, h, 8, &texels.iter().flat_map(|t| t.iter().flat_map(|v| v.to_le_bytes())).collect::<Vec<u8>>());
        let pitch = w * 4;
        let whole = buffer(g.dev, HEAP_READBACK, (pitch * h) as u64, 0, ST_COPY_DEST);
        let (bw, bh, bpitch, boff) = (64usize, 48usize, 256usize, 512u64);
        let boxed = buffer(g.dev, HEAP_READBACK, boff + (bpitch * bh) as u64, 0, ST_COPY_DEST);
        core::ptr::write_bytes(boxed.cpu, 0xcd, boff as usize + bpitch * bh);
        let ts: Vec<*mut c_void> = (0..3).map(|_| plain_rgb9e5(w, h)).collect();
        let (refused, converted) = (crate::uav_subst::refused_copies(), crate::uav_subst::converted_copies());
        let x = title_list(g.dev);
        let src = sub_loc(s as usize, 0);
        let dst = fp_loc(whole.res, 0, 67, [w as u32, h as u32, 1], pitch);
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), dst.as_ptr(), 0, 0, 0, src.as_ptr(), core::ptr::null());
        let dst = fp_loc(boxed.res, boff, 67, [bw as u32, bh as u32, 1], bpitch);
        let sbox: [u32; 6] = [16, 8, 0, 48, 40, 1];
        call!(x, 83, (*const u8, u32, u32, u32, *const u8, *const u32, u32) -> (), dst.as_ptr(), 5, 3, 0, src.as_ptr(), sbox.as_ptr(), 0);
        let dst = sub_loc(ts[0] as usize, 0);
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), dst.as_ptr(), 0, 0, 0, src.as_ptr(), core::ptr::null());
        call!(x, 17, (*mut c_void, *mut c_void) -> (), ts[1], s);
        call!(x, 84, (*mut c_void, *mut c_void, u32) -> (), ts[2], s, 0);
        run(g, x);
        assert_eq!((crate::uav_subst::refused_copies(), crate::uav_subst::converted_copies()), (refused, converted + 5), "five copies converted, none refused");
        let want: Vec<u32> = texels.iter().map(|&t| encoded(t)).collect();
        let got = words(core::slice::from_raw_parts(whole.cpu, pitch * h));
        let bad: Vec<usize> = (0..w * h).filter(|&i| got[i] != want[i]).collect();
        assert!(bad.is_empty(), "{} of {} texels differ from the spec encoding; first: texel {} f16 {:04x?} got {:#010x} want {:#010x}", bad.len(), w * h, bad[0], texels[bad[0]], got[bad[0]], want[bad[0]]);
        let bytes = core::slice::from_raw_parts(boxed.cpu, boff as usize + bpitch * bh);
        for y in 0..bh {
            for x in 0..bw {
                let at = boff as usize + y * bpitch + x * 4;
                let v = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                let inside = (5..37).contains(&x) && (3..35).contains(&y);
                if inside {
                    assert_eq!(v, want[(y - 3 + 8) * w + (x - 5 + 16)], "boxed copy texel ({x},{y})");
                } else {
                    assert_eq!(v, 0xcdcd_cdcd, "boxed copy wrote outside its box at ({x},{y})");
                }
            }
        }
        assert!(bytes[..boff as usize].iter().all(|&b| b == 0xcd), "boxed copy wrote before the footprint offset");
        for (k, t) in ts.iter().enumerate() {
            assert_eq!(words(&read(g, *t, 67, w, h, 4)), want, "R9G9B9E5 texture {k} (CopyTextureRegion, CopyResource, CopyResourceX)");
        }
        eprintln!("{} {} texels (every f16 value in each channel) encoded bit-exact against the spec reference; boxed footprint copy, texture copy, CopyResource and CopyResourceX checked", if g.warp { "WARP" } else { "hardware" }, w * h);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_rgb9e5_copies_into_a_substituted_texture_are_decoded_exact() {
    let g = gpu();
    unsafe {
        prepare(g);
        let (w, h) = (128usize, 128usize);
        let src_words: Vec<u32> = (0..(w * h) as u32).map(|i| (i % 512) | ((i * 7 + 3) % 512) << 9 | ((i * 13 + 5) % 512) << 18 | (i / 512) << 27).collect();
        let want: Vec<[u16; 4]> = src_words.iter().map(|&v| crate::uav_subst::rgb9e5_to_rgba16f(v)).collect();
        for (v, t) in src_words.iter().zip(&want) {
            let exact = crate::uav_subst::rgb9e5_decode(*v).map(crate::uav_subst::f32_to_f16);
            assert_eq!(&t[..3], &exact[..], "{v:#010x}");
        }
        let pitch = w * 4;
        let up = buffer(g.dev, HEAP_UPLOAD, (pitch * h) as u64, 0, ST_GENERIC_READ);
        core::ptr::copy_nonoverlapping(src_words.as_ptr() as *const u8, up.cpu, pitch * h);
        let ss: Vec<*mut c_void> = (0..4).map(|_| substituted(w, h)).collect();
        let marker = [0x1234u16, 0x5678, 0x9abc, 0xdef0];
        fill(g, ss[1], 10, w, h, 8, &(0..w * h).flat_map(|_| marker.iter().flat_map(|v| v.to_le_bytes())).collect::<Vec<u8>>());
        let t = plain_rgb9e5(w, h);
        fill(g, t, 67, w, h, 4, core::slice::from_raw_parts(src_words.as_ptr() as *const u8, pitch * h));
        let (refused, converted) = (crate::uav_subst::refused_copies(), crate::uav_subst::converted_copies());
        let x = title_list(g.dev);
        let fp = fp_loc(up.res, 0, 67, [w as u32, h as u32, 1], pitch);
        let d0 = sub_loc(ss[0] as usize, 0);
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), d0.as_ptr(), 0, 0, 0, fp.as_ptr(), core::ptr::null());
        let d1 = sub_loc(ss[1] as usize, 0);
        let sbox: [u32; 6] = [10, 20, 0, 74, 52, 1];
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const u32) -> (), d1.as_ptr(), 3, 4, 0, fp.as_ptr(), sbox.as_ptr());
        let (d2, st) = (sub_loc(ss[2] as usize, 0), sub_loc(t as usize, 0));
        call!(x, 83, (*const u8, u32, u32, u32, *const u8, *const c_void, u32) -> (), d2.as_ptr(), 0, 0, 0, st.as_ptr(), core::ptr::null(), 0);
        call!(x, 17, (*mut c_void, *mut c_void) -> (), ss[3], t);
        run(g, x);
        assert_eq!((crate::uav_subst::refused_copies(), crate::uav_subst::converted_copies()), (refused, converted + 4), "four copies converted, none refused");
        for (k, s) in ss.iter().enumerate() {
            let got = halves(&read(g, *s, 10, w, h, 8));
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let expect = if k != 1 {
                        want[i]
                    } else if (3..67).contains(&x) && (4..36).contains(&y) {
                        want[(y - 4 + 20) * w + (x - 3 + 10)]
                    } else {
                        marker
                    };
                    assert_eq!(got[i], expect, "substituted texture {k} texel ({x},{y})");
                }
            }
        }
        eprintln!("{} {} texels (every RGB9E5 mantissa and exponent) decoded exactly into RGBA16F from a footprint, a boxed footprint, a texture (slot 83) and CopyResource", if g.warp { "WARP" } else { "hardware" }, w * h);
    }
}

#[test]
#[ignore = "needs a D3D12 device; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_rgb9e5_conversions_on_a_copy_queue_list_are_refused() {
    const IID_QUEUE_LOCAL: Guid = Guid { d1: 0x0ec8_70a6, d2: 0x5d7e, d3: 0x4c22, d4: [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed] };
    const IID_ALLOCATOR_LOCAL: Guid = Guid { d1: 0x6102_dee4, d2: 0xaf59, d3: 0x4b09, d4: [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24] };
    const IID_LIST_LOCAL: Guid = Guid { d1: 0x5b16_0d0f, d2: 0xac1b, d3: 0x4185, d4: [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55] };
    let g = gpu();
    unsafe {
        prepare(g);
        let (w, h) = (64usize, 16usize);
        let s = substituted(w, h);
        let qd: [u32; 4] = [3, 0, 0, 0];
        let mut q: *mut c_void = core::ptr::null_mut();
        assert!(call!(g.dev, 8, (*const u32, *const Guid, *mut *mut c_void) -> i32, qd.as_ptr(), &IID_QUEUE_LOCAL, &mut q) >= 0);
        crate::sys_ts::note_queue(q, vs(q, 16), vs(q, 17));
        crate::present::apply_queue_vtable(q);
        let mut a: *mut c_void = core::ptr::null_mut();
        assert!(call!(g.dev, 9, (u32, *const Guid, *mut *mut c_void) -> i32, 3, &IID_ALLOCATOR_LOCAL, &mut a) >= 0);
        let mut l: *mut c_void = core::ptr::null_mut();
        assert!(call!(g.dev, 12, (u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32, 0, 3, a, core::ptr::null_mut(), &IID_LIST_LOCAL, &mut l) >= 0);
        let x = crate::cmdlist_wrap::wrap(l);
        let rb = buffer(g.dev, HEAP_READBACK, (256 * h) as u64, 0, ST_COPY_DEST);
        core::ptr::write_bytes(rb.cpu, 0xcd, 256 * h);
        let (refused, converted) = (crate::uav_subst::refused_copies(), crate::uav_subst::converted_copies());
        let (dst, src) = (fp_loc(rb.res, 0, 67, [w as u32, h as u32, 1], 256), sub_loc(s as usize, 0));
        call!(x, 16, (*const u8, u32, u32, u32, *const u8, *const c_void) -> (), dst.as_ptr(), 0, 0, 0, src.as_ptr(), core::ptr::null());
        run_on(q as usize, x);
        assert_eq!((crate::uav_subst::refused_copies(), crate::uav_subst::converted_copies()), (refused + 1, converted), "a copy-queue list cannot run the compute pass, so the copy is refused");
        assert!(core::slice::from_raw_parts(rb.cpu, 256 * h).iter().all(|&b| b == 0xcd), "the refused copy wrote data");
    }
}

#[test]
#[ignore = "needs a D3D12 device and xg_xs.dll; run: cargo test -p d3d12_xs --lib hw_ -- --ignored --test-threads=1 (TOMBOX_HW_ADAPTER=warp for WARP)"]
fn hw_rgb9e5_title_memory_is_mirrored_into_a_substituted_texture() {
    extern "system" {
        fn VirtualAlloc(addr: *mut c_void, len: usize, ty: u32, protect: u32) -> *mut c_void;
    }
    let g = gpu();
    unsafe {
        prepare(g);
        let (w, h) = (128usize, 64usize);
        let desc = tex2d(w, h, 67, FLAG_UAV);
        let (total, bpe, offs) = crate::upload::test_offsets(&desc).unwrap_or_else(|e| panic!("XG layout for R9G9B9E5: {e}"));
        assert_eq!((bpe, offs.len()), (4, w * h));
        let n = ((total as usize) + 0xffff) & !0xffff;
        let mem = VirtualAlloc(core::ptr::null_mut(), n, 0x3000 | 0x20_0000, 0x04) as usize;
        assert_ne!(mem, 0);
        crate::cpu_watch::register_test_region(mem, n);
        let put = |seed: u32| {
            for (i, &o) in offs.iter().enumerate() {
                let v = (i as u32).wrapping_mul(0x9e37_79b9).wrapping_add(seed) | 1;
                *((mem + o as usize) as *mut u32) = v;
            }
        };
        let want = || -> Vec<[u16; 4]> { offs.iter().map(|&o| crate::uav_subst::rgb9e5_to_rgba16f(*((mem + o as usize) as *const u32))).collect() };
        put(0x1111);
        let mut tex: *mut c_void = core::ptr::null_mut();
        let hr = call!(wrapped(), D_PLACED_X, (u64, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32, mem as u64, desc.as_ptr(), ST_COMMON, core::ptr::null(), &IID_RESOURCE, &mut tex);
        assert!(hr >= 0 && !tex.is_null(), "CreatePlacedResourceX hr={hr:#010x}");
        assert_eq!(crate::uav_subst::lookup(tex), Some(crate::uav_subst::TABLE[0]));
        let pump_until = |what: &str, done: &dyn Fn() -> bool| {
            let t = Instant::now();
            while !done() {
                assert!(t.elapsed() < Duration::from_secs(10), "upload pump never reached: {what}");
                crate::upload::pump(g.a as *mut c_void);
                std::thread::sleep(Duration::from_millis(2));
            }
            crate::upload::pump(g.a as *mut c_void);
        };
        pump_until("first upload", &|| crate::upload::test_ww(tex).sent);
        let s0 = crate::upload::test_ww(tex);
        assert_eq!(halves(&read(g, tex, 10, w, h, 8)), want(), "first upload of console RGB9E5 texels");
        put(0x7777);
        pump_until("re-upload after a CPU rewrite", &|| crate::upload::test_ww(tex).uploaded_subs > s0.uploaded_subs);
        assert_eq!(halves(&read(g, tex, 10, w, h, 8)), want(), "re-upload after a CPU rewrite");
        assert_eq!(crate::upload::test_ww(tex).lost, s0.lost, "no CPU write may be dropped");
        call!(tex, 2, () -> u32);
        eprintln!("{} {} title texels widened from RGB9E5 to RGBA16F on upload, twice", if g.warp { "WARP" } else { "hardware" }, w * h);
    }
}
